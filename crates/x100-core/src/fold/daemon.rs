//! The fold of the daemon's log: what belongs to no project.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::records::JobRecord;
use super::{Changes, Corrupt, Fail, FoldError, Record, expected_after, on, unknown};
use crate::credential::Credential;
use crate::event::{DaemonEvent, Envelope, Seq};
use crate::harness::{CoolOff, Harness, HarnessModel, Machine};
use crate::id::{CredentialRef, HarnessId, JobId, MachineId, ProjectId};
use crate::job::{self, DaemonJob, JobEvent};
use crate::settings::DaemonSettings;
use crate::time::Timestamp;

/// The parts of the interface the daemon's events change. Credentials are the Credentials part of
/// Settings, and projects are the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonPart {
    Machines,
    Harnesses,
    Credentials,
    Projects,
    Settings,
    Jobs,
}

/// A harness and model cooling off after a failure that looked like the provider's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoolOffRecord {
    pub cool_off: CoolOff,
    /// The job that ends it.
    pub ends: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRecord {
    /// The reference, the name and the scope. Never the value.
    pub credential: Credential,
    pub added: Timestamp,
    pub replaced: Option<Timestamp>,
}

/// A project on the daemon's list. Its own log holds the rest, the sidebar's live state included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub name: String,
    pub path: String,
    pub added: Timestamp,
}

/// Everything the daemon holds outside any project, folded from its log.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DaemonState {
    seq: Option<Seq>,
    machines: BTreeMap<MachineId, Machine>,
    harnesses: BTreeMap<HarnessId, Harness>,
    /// At most one per harness and model, ordered by it.
    cool_offs: Vec<CoolOffRecord>,
    credentials: BTreeMap<CredentialRef, CredentialRecord>,
    projects: BTreeMap<ProjectId, ProjectEntry>,
    settings: DaemonSettings,
    jobs: BTreeMap<JobId, JobRecord<DaemonJob>>,
}

type Ch = Changes<DaemonPart>;

impl DaemonState {
    pub fn fold<'a>(
        log: impl IntoIterator<Item = &'a Envelope<DaemonEvent>>,
    ) -> Result<Self, FoldError> {
        let mut state = Self::default();
        for e in log {
            state.apply(e)?;
        }
        Ok(state)
    }

    /// Applies the next event of the daemon's log and says what it changed.
    pub fn apply(&mut self, e: &Envelope<DaemonEvent>) -> Result<Changes<DaemonPart>, FoldError> {
        let expected = expected_after(self.seq);
        if e.seq != expected {
            return Err(FoldError {
                seq: e.seq,
                record: None,
                cause: Corrupt::OutOfOrder { expected },
            });
        }
        let mut ch = Changes::new();
        self.event(e.at, &e.event, &mut ch)
            .map_err(|(record, cause)| FoldError {
                seq: e.seq,
                record,
                cause,
            })?;
        self.seq = Some(e.seq);
        Ok(ch)
    }

    fn event(&mut self, at: Timestamp, event: &DaemonEvent, ch: &mut Ch) -> Result<(), Fail> {
        use DaemonEvent as D;
        match event {
            D::MachineAdded(m) => {
                insert_new(&mut self.machines, m.id, m.clone(), Record::Machine(m.id))?;
                ch.part(DaemonPart::Machines).record(Record::Machine(m.id));
            }
            D::MachineCapSet { machine, cap } => {
                let record = Record::Machine(*machine);
                self.machines.get_mut(machine).ok_or(unknown(record))?.cap = *cap;
                ch.part(DaemonPart::Machines).record(record);
            }
            D::HarnessAdded(h) => {
                insert_new(&mut self.harnesses, h.id, h.clone(), Record::Harness(h.id))?;
                ch.part(DaemonPart::Harnesses).record(Record::Harness(h.id));
            }
            D::HarnessUpdated(h) => {
                let record = Record::Harness(h.id);
                *self.harnesses.get_mut(&h.id).ok_or(unknown(record))? = h.clone();
                ch.part(DaemonPart::Harnesses).record(record);
            }
            D::CoolOffStarted { cool_off, ends } => {
                let record = Record::Harness(cool_off.of.harness);
                if !self.harnesses.contains_key(&cool_off.of.harness) {
                    return Err(unknown(record));
                }
                let new = CoolOffRecord {
                    cool_off: cool_off.clone(),
                    ends: *ends,
                };
                // A later failure while cooling off restarts the cool-off.
                match self.cool_off_at(&cool_off.of) {
                    Ok(i) => self.cool_offs[i] = new,
                    Err(i) => self.cool_offs.insert(i, new),
                }
                ch.part(DaemonPart::Harnesses).record(record);
            }
            D::CoolOffEnded(of) => {
                let record = Record::Harness(of.harness);
                let i = self.cool_off_at(of).map_err(|_| unknown(record))?;
                self.cool_offs.remove(i);
                ch.part(DaemonPart::Harnesses).record(record);
            }
            D::CredentialAdded(c) => {
                let record = Record::Credential(c.reference);
                let new = CredentialRecord {
                    credential: c.clone(),
                    added: at,
                    replaced: None,
                };
                insert_new(&mut self.credentials, c.reference, new, record)?;
                ch.part(DaemonPart::Credentials).record(record);
            }
            D::CredentialReplaced { credential } => {
                self.credential(*credential, ch)?.replaced = Some(at);
            }
            D::CredentialScopeSet { credential, scope } => {
                self.credential(*credential, ch)?.credential.scope = scope.clone();
            }
            D::CredentialRemoved { credential } => {
                let record = Record::Credential(*credential);
                self.credentials.remove(credential).ok_or(unknown(record))?;
                ch.part(DaemonPart::Credentials).record(record);
            }
            D::ProjectAdded {
                project,
                name,
                path,
            } => {
                let record = Record::Project(*project);
                let entry = ProjectEntry {
                    name: name.clone(),
                    path: path.clone(),
                    added: at,
                };
                insert_new(&mut self.projects, *project, entry, record)?;
                ch.part(DaemonPart::Projects).record(record);
            }
            D::SettingChanged(change) => {
                self.settings.set(*change);
                ch.part(DaemonPart::Settings);
            }
            D::JobScheduled(j) => {
                let record = Record::Job(j.id);
                insert_new(&mut self.jobs, j.id, JobRecord::new(j.clone(), at), record)?;
                ch.part(DaemonPart::Jobs).record(record);
            }
            D::Job { job, event } => {
                let record = Record::Job(*job);
                let rec = self.jobs.get_mut(job).ok_or(unknown(record))?;
                rec.state = job::next(&rec.state, event).map_err(on(record))?;
                match event {
                    JobEvent::Fired { at } => {
                        rec.fired += 1;
                        rec.last_fired = Some(*at);
                    }
                    JobEvent::Cancelled { by } => rec.cancelled_by = Some(*by),
                }
                ch.part(DaemonPart::Jobs).record(record);
            }
        }
        Ok(())
    }

    fn cool_off_at(&self, of: &HarnessModel) -> Result<usize, usize> {
        self.cool_offs.binary_search_by(|c| c.cool_off.of.cmp(of))
    }

    fn credential(
        &mut self,
        id: CredentialRef,
        ch: &mut Ch,
    ) -> Result<&mut CredentialRecord, Fail> {
        let record = Record::Credential(id);
        ch.part(DaemonPart::Credentials).record(record);
        self.credentials.get_mut(&id).ok_or(unknown(record))
    }

    /// The last event applied, from which a client follows the log.
    pub fn seq(&self) -> Option<Seq> {
        self.seq
    }

    pub fn machines(&self) -> impl Iterator<Item = &Machine> {
        self.machines.values()
    }

    pub fn harnesses(&self) -> impl Iterator<Item = &Harness> {
        self.harnesses.values()
    }

    pub fn harness(&self, id: HarnessId) -> Option<&Harness> {
        self.harnesses.get(&id)
    }

    /// The cool-offs in force.
    pub fn cool_offs(&self) -> &[CoolOffRecord] {
        &self.cool_offs
    }

    pub fn cooling_off(&self, of: &HarnessModel) -> Option<&CoolOffRecord> {
        self.cool_off_at(of).ok().map(|i| &self.cool_offs[i])
    }

    pub fn credentials(&self) -> impl Iterator<Item = &CredentialRecord> {
        self.credentials.values()
    }

    pub fn projects(&self) -> impl Iterator<Item = (ProjectId, &ProjectEntry)> {
        self.projects.iter().map(|(id, p)| (*id, p))
    }

    pub fn settings(&self) -> &DaemonSettings {
        &self.settings
    }

    pub fn jobs(&self) -> impl Iterator<Item = &JobRecord<DaemonJob>> {
        self.jobs.values()
    }
}

fn insert_new<K: Ord, V>(
    map: &mut BTreeMap<K, V>,
    key: K,
    value: V,
    record: Record,
) -> Result<(), Fail> {
    if map.contains_key(&key) {
        return Err((Some(record), Corrupt::Duplicate));
    }
    map.insert(key, value);
    Ok(())
}
