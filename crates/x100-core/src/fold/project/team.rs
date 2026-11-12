//! Events about the team: agents, their drivers, processes and spend.

use super::{Ch, ProjectPart, ProjectState, Stamp};
use crate::agent::{self, Agent, AgentEvent, AgentState, Position};
use crate::driver::{self, Driver, DriverEvent, Purpose};
use crate::event::ProjectEvent;
use crate::fold::records::{AgentRecord, DriverRecord, RunningProcess, Spent};
use crate::fold::{Corrupt, Fail, Record, on, unknown};
use crate::id::{AgentId, DriverId};
use crate::ledger::{Charge, ChargeSource, Payer};
use crate::party::Party;
use crate::process::{Process, ProcessName};
use crate::team::{TeamError, TeamOp};
use crate::time::Timestamp;
use crate::wait::WaitOn;

impl ProjectState {
    pub(super) fn agent_added(&mut self, s: Stamp, a: &Agent, ch: &mut Ch) -> Result<(), Fail> {
        let op = TeamOp::Join {
            agent: a.id,
            position: a.position.clone(),
        };
        self.change_team(&op, Record::Agent(a.id))?;
        self.agents
            .insert(a.id, AgentRecord::new(a.id, &a.role, &a.charter, s.at));
        ch.part(ProjectPart::Team)
            .part(ProjectPart::Sidebar)
            .record(Record::Agent(a.id));
        if matches!(a.position, Position::Lead { .. }) {
            ch.part(ProjectPart::Budget).part(ProjectPart::Settings);
        }
        Ok(())
    }

    pub(super) fn driver_started(&mut self, s: Stamp, d: &Driver, ch: &mut Ch) -> Result<(), Fail> {
        let id = d.id;
        if self.drivers.contains_key(&id) {
            return Err((Some(Record::Driver(id)), Corrupt::Duplicate));
        }
        let team = &self.project_mut()?.team;
        if team.is_retired(d.agent) {
            return Err((
                Some(Record::Agent(d.agent)),
                TeamError::Retired(d.agent).into(),
            ));
        }
        // The half of "every driver is compatible when it starts" that the log alone can
        // show. Allow-lists, cool-offs and budgets are the router's to check.
        let range = team
            .effort(d.agent)
            .ok_or(unknown(Record::Agent(d.agent)))?;
        if !range.contains(d.effort) {
            let cause = Corrupt::OutOfRange {
                effort: d.effort,
                range,
            };
            return Err((Some(Record::Driver(id)), cause));
        }
        if d.purpose == Purpose::Work
            && let Some(other) = self.acting_driver(d.agent)
        {
            return Err((Some(Record::Driver(id)), Corrupt::SecondDriver(other)));
        }
        self.agent_mut(d.agent)?.drivers.push(id);
        self.drivers.insert(
            id,
            DriverRecord {
                driver: d.clone(),
                state: driver::DriverState::INITIAL,
                started: s.at,
                ended: None,
                turns: 0,
                context: None,
                spent: Spent::default(),
            },
        );
        ch.part(ProjectPart::Team)
            .record(Record::Driver(id))
            .record(Record::Agent(d.agent));
        Ok(())
    }

    pub(super) fn process_started(
        &mut self,
        s: Stamp,
        p: &Process,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Agent(p.agent);
        let running = RunningProcess {
            command: p.command.clone(),
            started: s.at,
        };
        let processes = &mut self.agent_mut(p.agent)?.processes;
        if processes.contains_key(&p.name) {
            return Err((Some(record), Corrupt::Duplicate));
        }
        processes.insert(p.name.clone(), running);
        ch.part(ProjectPart::Team).record(record);
        Ok(())
    }

    pub(super) fn process_stopped(
        &mut self,
        agent: &AgentId,
        name: &ProcessName,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Agent(*agent);
        self.agent_mut(*agent)?
            .processes
            .remove(name)
            .ok_or(unknown(record))?;
        ch.part(ProjectPart::Team).record(record);
        Ok(())
    }

    pub(super) fn agent_event(
        &mut self,
        id: AgentId,
        event: &AgentEvent,
        whole: &ProjectEvent,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Agent(id);
        let rec = self.agents.get(&id).ok_or(unknown(record))?;
        let next = agent::next(&rec.state, event).map_err(on(record))?;
        let was_working = rec.state == AgentState::Working;
        if let Some(op) = TeamOp::from_event(whole) {
            self.change_team(&op, record)?;
        }
        let rec = self.agents.get_mut(&id).ok_or(unknown(record))?;
        match event {
            AgentEvent::Delivered { item, what } => {
                rec.inbox.deliver(*item).map_err(on(record))?;
                rec.items.insert(*item, what.clone());
            }
            AgentEvent::WaitOpened(wait) => {
                if rec.waits.contains_key(&wait.id) {
                    return Err((Some(record), Corrupt::Duplicate));
                }
                if let WaitOn::Reply { message, from } = wait.on {
                    let m = self
                        .messages
                        .get_mut(&message)
                        .ok_or(unknown(Record::Message(message)))?;
                    m.awaited = true;
                    ch.record(Record::Message(message));
                    if from == Party::Builder {
                        ch.part(ProjectPart::Chat);
                    }
                }
                rec.waits.insert(wait.id, wait.clone());
            }
            AgentEvent::WaitEnded { wait, .. } => {
                rec.waits.remove(wait).ok_or(unknown(record))?;
            }
            AgentEvent::EffortSet(_) => {
                ch.part(ProjectPart::Settings);
            }
            AgentEvent::BudgetSet(_) | AgentEvent::Promoted { .. } => {
                ch.part(ProjectPart::Settings).part(ProjectPart::Budget);
            }
            _ => {}
        }
        if was_working != (next == AgentState::Working) || next == AgentState::Retired {
            ch.part(ProjectPart::Sidebar);
        }
        rec.state = next;
        ch.part(ProjectPart::Team).record(record);
        Ok(())
    }

    pub(super) fn driver_event(
        &mut self,
        at: Timestamp,
        id: DriverId,
        event: &DriverEvent,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Driver(id);
        let rec = self.drivers.get(&id).ok_or(unknown(record))?;
        let next = driver::next(&rec.state, event).map_err(on(record))?;
        let agent = rec.driver.agent;
        // A lab driver runs an arm of a trial, not the agent's work, so the agent's inbox is not
        // its business.
        if rec.driver.purpose == Purpose::Work {
            let a = self.agent_mut(agent)?;
            match event {
                DriverEvent::TurnStarted { given } => {
                    a.inbox.give(given, id).map_err(on(record))?
                }
                DriverEvent::TurnEnded => {
                    for item in a.inbox.turn_completed(id) {
                        a.items.remove(&item);
                    }
                }
                DriverEvent::Failed(_) | DriverEvent::Stopped(_) => {
                    a.inbox.turn_lost(id);
                }
                _ => {}
            }
        }
        let rec = self.drivers.get_mut(&id).ok_or(unknown(record))?;
        match event {
            DriverEvent::TurnStarted { .. } => rec.turns += 1,
            DriverEvent::ContextReported(fill) => rec.context = Some(*fill),
            _ => {}
        }
        if !next.is_live() {
            rec.ended = Some(at);
        }
        rec.state = next;
        ch.part(ProjectPart::Team)
            .record(record)
            .record(Record::Agent(agent));
        Ok(())
    }

    pub(super) fn charged(&mut self, c: &Charge, ch: &mut Ch) -> Result<(), Fail> {
        if let ChargeSource::Turn { driver, agent } = c.source {
            self.agent_mut(agent)?;
            let d = self
                .drivers
                .get_mut(&driver)
                .ok_or(unknown(Record::Driver(driver)))?;
            d.spent.add(c.cost, c.usage);
            if c.payer != Payer::Lab {
                self.agent_mut(agent)?.spent.add(c.cost, c.usage);
            }
            ch.part(ProjectPart::Team)
                .record(Record::Driver(driver))
                .record(Record::Agent(agent));
        }
        self.spend.charge(c.payer, c.cost);
        self.usage += c.usage;
        ch.part(ProjectPart::Budget);
        Ok(())
    }
}
