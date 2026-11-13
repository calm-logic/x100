//! Reflection's tools, `read_log` and `propose_change`, as the folded state alone decides them:
//! the setting chooses between committing at once and a trial, a correction cites the log, and a
//! team change is the events the builder's approval would append.

use x100_core::agent::{AgentEvent, AgentState};
use x100_core::effort::EffortRange;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::memory::NoteName;
use x100_core::money::Money;
use x100_core::party::Actor;
use x100_core::project::SettingChange;
use x100_core::team::{LeadSpec, TeamChange};
use x100_core::testing::deploy_skill;
use x100_core::tool::{Proposal, ToolCall};
use x100_core::trial::{EvalItem, EvalSet, TrialChange};

use super::{Scene, scene};
use crate::tools::MemoryPlan;

fn edit() -> ToolCall {
    ToolCall::ProposeChange(Proposal::Trial {
        change: TrialChange::EditSkill {
            skill: deploy_skill(),
            body: "---\nname: deploy\ndescription: d\n---\n\nBetter.\n".into(),
        },
        eval: EvalSet {
            items: vec![EvalItem::Probe {
                question: "q".into(),
                answer: "a".into(),
            }],
            runs_per_arm: 3,
        },
    })
}

fn trials_on(s: &mut Scene) {
    let seq = s.state.seq().unwrap().0 + 1;
    let e = Envelope {
        seq: Seq(seq),
        at: s.f.log.last().unwrap().at,
        by: Actor::Builder,
        event: ProjectEvent::SettingChanged(SettingChange::TrialBeforeCommit(true)),
    };
    s.state.apply(&e).unwrap();
}

#[test]
fn only_the_ceo_reflects_and_reads_within_the_log() {
    let s = scene();
    let last = s.state.seq().unwrap().0;
    let read = |from: u64, to: Option<u64>| ToolCall::ReadLog {
        from: Seq(from),
        to: to.map(Seq),
    };
    assert!(s.refused(s.f.tech, &read(1, None)).contains("only the CEO"));
    assert!(s.refused(s.f.tech, &edit()).contains("only the CEO"));
    let plan = s.call(s.f.ceo, &read(5, Some(last + 50))).unwrap();
    assert_eq!(
        plan.memory,
        Some(MemoryPlan::ReadLog {
            from: Seq(5),
            to: last
        })
    );
    assert!(
        s.refused(s.f.ceo, &read(0, None))
            .contains("runs from event 1")
    );
    assert!(
        s.refused(s.f.ceo, &read(last + 1, None))
            .contains("runs from event 1")
    );
    assert!(s.refused(s.f.ceo, &read(9, Some(4))).contains("before"));
}

#[test]
fn the_setting_decides_between_committing_at_once_and_a_trial() {
    let mut s = scene();
    let off = s.call(s.f.ceo, &edit()).unwrap();
    assert!(off.trial.is_none());
    assert!(matches!(off.memory, Some(MemoryPlan::Commit { .. })));
    trials_on(&mut s);
    let on = s.call(s.f.ceo, &edit()).unwrap();
    assert!(on.memory.is_none() && on.trial.is_some());
    // Whatever the setting, a compaction must keep some note.
    let empty = ToolCall::ProposeChange(Proposal::Trial {
        change: TrialChange::CompactMemory {
            agent: s.f.tech,
            notes: vec![],
        },
        eval: EvalSet {
            items: vec![],
            runs_per_arm: 3,
        },
    });
    assert!(s.refused(s.f.ceo, &empty).contains("cannot keep none"));
}

#[test]
fn a_correction_cites_events_in_the_log_and_is_never_a_trial() {
    let mut s = scene();
    trials_on(&mut s);
    let correct = |sources: Vec<Seq>| {
        ToolCall::ProposeChange(Proposal::Correction {
            agent: s.f.tech,
            note: NoteName("Deploys".into()),
            body: "Deploys go through make ship.".into(),
            sources,
        })
    };
    let plan = s.call(s.f.ceo, &correct(vec![Seq(3)])).unwrap();
    assert!(plan.trial.is_none(), "corrections are never put on trial");
    assert_eq!(
        plan.memory,
        Some(MemoryPlan::Correct {
            agent: s.f.tech,
            note: NoteName("deploys.md".into()),
            body: "Deploys go through make ship.".into(),
            sources: vec![Seq(3)],
        })
    );
    assert!(
        s.refused(s.f.ceo, &correct(vec![]))
            .contains("cites the events")
    );
    let beyond = Seq(s.state.seq().unwrap().0 + 1);
    assert!(
        s.refused(s.f.ceo, &correct(vec![beyond]))
            .contains("not in the log")
    );
}

#[test]
fn a_team_change_is_the_events_its_approval_appends() {
    let s = scene();
    let team = |changes: Vec<TeamChange>, telemetry: &str| {
        ToolCall::ProposeChange(Proposal::Team {
            changes,
            telemetry: telemetry.into(),
            team_toml: String::new(),
        })
    };
    let lead = TeamChange::AddLead(LeadSpec {
        role: "Ops".into(),
        charter: "Keeps it running.".into(),
        effort: EffortRange::FULL,
        budget: Money::from_dollars(1),
    });
    let telemetry = "Deploys failed 4 of 6 times in the log (#120, #188); nobody owns them.";
    let after = s.folds(s.f.ceo, &team(vec![lead.clone()], telemetry));
    assert!(
        after.agents().any(|a| a.role == "Ops"),
        "the new lead joins once the events are appended"
    );
    assert!(
        s.refused(s.f.ceo, &team(vec![lead], "  "))
            .contains("telemetry")
    );
    assert!(
        s.refused(s.f.ceo, &team(vec![], telemetry))
            .contains("at least one")
    );
    // An agent at work is not retired from under its task.
    let busy = s
        .state
        .agents()
        .find(|a| a.id != s.f.ceo && !a.open_tasks.is_empty())
        .unwrap()
        .id;
    let retire = team(vec![TeamChange::Retire { agent: busy }], telemetry);
    assert!(s.refused(s.f.ceo, &retire).contains("at work"));
    let ceo = team(vec![TeamChange::Retire { agent: s.f.ceo }], telemetry);
    assert!(!s.refused(s.f.ceo, &ceo).is_empty());
    // An idle agent is retired.
    let idle = s
        .state
        .agents()
        .find(|a| {
            a.state == AgentState::Idle
                && a.open_tasks.is_empty()
                && a.id != s.f.ceo
                && s.state.children(a.id).is_empty()
        })
        .expect("Fernleaf has an idle agent with nobody under it");
    let plan = s
        .call(
            s.f.ceo,
            &team(vec![TeamChange::Retire { agent: idle.id }], telemetry),
        )
        .unwrap();
    assert!(plan.events.iter().any(|(_, e)| matches!(
        e,
        ProjectEvent::Agent {
            event: AgentEvent::Retired,
            ..
        }
    )));
}
