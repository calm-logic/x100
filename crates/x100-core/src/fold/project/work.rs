//! Events about the work: tasks, messages, jobs, actions, specs, files, memories, skills and
//! trials.

use super::{Ch, ProjectPart, ProjectState, Stamp};
use crate::action::{self, Action, ActionEvent};
use crate::file::FileRef;
use crate::fold::records::{
    ActionRecord, ChatEntry, ChatItem, FileRecord, JobRecord, LoadedSkill, MessageRecord,
    SkillRecord, SpecRecord, TaskRecord, TrialRecord,
};
use crate::fold::{Corrupt, Fail, Record, on, unknown};
use crate::git::CommitId;
use crate::id::{ActionId, AgentId, FileId, JobId, TaskId, TrialId};
use crate::job::{self, Job, JobEvent, ProjectJob};
use crate::memory::{NoteName, SkillRef, SkillScope, SkillVersion};
use crate::message::Route;
use crate::party::Party;
use crate::spec::SpecRevision;
use crate::task::{self, Opening, Task, TaskEvent, TaskState, Verdict};
use crate::time::Timestamp;
use crate::trial::{self, Trial, TrialEvent, TrialState};

impl ProjectState {
    pub(super) fn job_scheduled(
        &mut self,
        s: Stamp,
        j: &Job<ProjectJob>,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        if self.jobs.contains_key(&j.id) {
            return Err((Some(Record::Job(j.id)), Corrupt::Duplicate));
        }
        self.jobs.insert(j.id, JobRecord::new(j.clone(), s.at));
        job_changed(&j.does, j.id, ch);
        Ok(())
    }

    pub(super) fn job_event(
        &mut self,
        job: &JobId,
        event: &JobEvent,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let rec = self.jobs.get_mut(job).ok_or(unknown(Record::Job(*job)))?;
        rec.state = job::next(&rec.state, event).map_err(on(Record::Job(*job)))?;
        match event {
            JobEvent::Fired { at } => {
                rec.fired += 1;
                rec.last_fired = Some(*at);
            }
            JobEvent::Cancelled { by } => rec.cancelled_by = Some(*by),
        }
        job_changed(&rec.job.does, *job, ch);
        Ok(())
    }

    pub(super) fn action_proposed(
        &mut self,
        s: Stamp,
        a: &Action,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        if self.actions.contains_key(&a.id) {
            return Err((Some(Record::Action(a.id)), Corrupt::Duplicate));
        }
        self.agent_mut(a.agent)?;
        self.actions.insert(
            a.id,
            ActionRecord {
                action: a.clone(),
                state: action::ActionState::INITIAL,
                proposed: s.at,
                classification: None,
                decided: None,
                error: None,
            },
        );
        ch.record(Record::Action(a.id));
        Ok(())
    }

    pub(super) fn spec_written(
        &mut self,
        s: Stamp,
        spec: &SpecRevision,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        self.agent_mut(spec.author)?;
        self.specs.insert(
            spec.kind,
            SpecRecord {
                spec: spec.clone(),
                at: s.at,
            },
        );
        ch.part(ProjectPart::Specs).record(Record::Spec(spec.kind));
        Ok(())
    }

    pub(super) fn file_added(&mut self, s: Stamp, f: &FileRef, ch: &mut Ch) -> Result<(), Fail> {
        if self.files.contains_key(&f.id) {
            return Err((Some(Record::File(f.id)), Corrupt::Duplicate));
        }
        self.files.insert(
            f.id,
            FileRecord {
                file: f.clone(),
                added: s.at,
            },
        );
        file_changed(f.id, f.attached_to.is_some(), ch);
        Ok(())
    }

    pub(super) fn file_removed(&mut self, file: &FileId, ch: &mut Ch) -> Result<(), Fail> {
        let removed = self
            .files
            .remove(file)
            .ok_or(unknown(Record::File(*file)))?;
        file_changed(*file, removed.file.attached_to.is_some(), ch);
        Ok(())
    }

    pub(super) fn memory_written(
        &mut self,
        agent: &AgentId,
        note: &NoteName,
        commit: &CommitId,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        self.agent_mut(*agent)?
            .memories
            .insert(note.clone(), commit.clone());
        ch.part(ProjectPart::Team).record(Record::Agent(*agent));
        Ok(())
    }

    pub(super) fn skill_loaded(
        &mut self,
        agent: &AgentId,
        skill: &SkillRef,
        version: &SkillVersion,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let skills = &mut self.agent_mut(*agent)?.skills;
        let loaded = LoadedSkill {
            skill: skill.clone(),
            version: *version,
        };
        match skills.iter_mut().find(|l| l.skill == *skill) {
            Some(l) => *l = loaded,
            None => skills.push(loaded),
        }
        ch.part(ProjectPart::Team).record(Record::Agent(*agent));
        Ok(())
    }

    pub(super) fn trial_proposed(&mut self, s: Stamp, t: &Trial, ch: &mut Ch) -> Result<(), Fail> {
        if self.trials.contains_key(&t.id) {
            return Err((Some(Record::Trial(t.id)), Corrupt::Duplicate));
        }
        self.trials.insert(
            t.id,
            TrialRecord {
                trial: t.clone(),
                state: TrialState::INITIAL,
                proposed: s.at,
                runs: Vec::new(),
                commit: None,
                why: None,
            },
        );
        ch.part(ProjectPart::Team).record(Record::Trial(t.id));
        Ok(())
    }

    pub(super) fn trial_event(
        &mut self,
        trial: &TrialId,
        event: &TrialEvent,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let id = Record::Trial(*trial);
        let rec = self.trials.get_mut(trial).ok_or(unknown(id))?;
        rec.state = trial::next(&rec.state, event).map_err(on(id))?;
        match event {
            TrialEvent::RunFinished(score) => rec.runs.push(score.clone()),
            TrialEvent::Committed { commit } => rec.commit = Some(commit.clone()),
            TrialEvent::Discarded { why } | TrialEvent::Stopped { why } => {
                rec.why = Some(why.clone())
            }
            _ => {}
        }
        ch.part(ProjectPart::Team).record(id);
        Ok(())
    }

    pub(super) fn skill_committed(
        &mut self,
        skill: &SkillRef,
        version: &SkillVersion,
        commit: &CommitId,
        trial: &Option<TrialId>,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        if let Some(trial) = trial {
            self.committed_trial(*trial)?;
        }
        let record = SkillRecord {
            skill: skill.clone(),
            version: *version,
            commit: commit.clone(),
            trial: *trial,
        };
        if let SkillScope::Agent(owner) = skill.scope {
            self.agent_mut(owner)?;
        }
        match self.skills.binary_search_by(|r| r.skill.cmp(skill)) {
            Ok(i) => self.skills[i] = record,
            Err(i) => self.skills.insert(i, record),
        }
        let part = ch.part(ProjectPart::Team);
        if let Some(trial) = trial {
            part.record(Record::Trial(*trial));
        }
        if let SkillScope::Agent(owner) = skill.scope {
            part.record(Record::Agent(owner));
        }
        Ok(())
    }

    pub(super) fn memory_compacted(
        &mut self,
        agent: &AgentId,
        trial: &Option<TrialId>,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        if let Some(trial) = trial {
            self.committed_trial(*trial)?;
        }
        self.agent_mut(*agent)?;
        let part = ch.part(ProjectPart::Team);
        if let Some(trial) = trial {
            part.record(Record::Trial(*trial));
        }
        part.record(Record::Agent(*agent));
        Ok(())
    }

    /// Reading a note changes nothing in the state; the event is the record, for reflection.
    pub(super) fn note_read(&mut self, agent: &AgentId) -> Result<(), Fail> {
        self.agent_mut(*agent).map(drop)
    }

    pub(super) fn task_opened(
        &mut self,
        at: Timestamp,
        task: &Task,
        opening: &Opening,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Task(task.id);
        if self.tasks.contains_key(&task.id) {
            return Err((Some(record), Corrupt::Duplicate));
        }
        let state = TaskState::initial(opening);
        if let TaskState::InProgress(a) = &state {
            self.agent_mut(a.assignee)?.open_tasks.push(task.id);
            ch.part(ProjectPart::Team).record(Record::Agent(a.assignee));
        }
        self.tasks.insert(
            task.id,
            TaskRecord {
                task: task.clone(),
                state,
                opened: at,
                // Tasks are never removed, so this counts every task opened before it.
                seq: self.tasks.len(),
                progress: None,
                sent_back: None,
                conflict: None,
                merged: None,
            },
        );
        ch.part(ProjectPart::Tasks).record(record);
        Ok(())
    }

    pub(super) fn task_event(
        &mut self,
        s: Stamp,
        id: TaskId,
        event: &TaskEvent,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Task(id);
        let rec = self.tasks.get(&id).ok_or(unknown(record))?;
        let next = task::next(&rec.state, event).map_err(on(record))?;
        let before = in_progress(&rec.state);
        let after = in_progress(&next);
        if before != after
            && let Some(assignee) = before.or(after)
        {
            let open = &mut self.agent_mut(assignee)?.open_tasks;
            open.retain(|t| *t != id);
            if after.is_some() {
                open.push(id);
            }
            ch.part(ProjectPart::Team).record(Record::Agent(assignee));
        }
        let rec = self.tasks.get_mut(&id).ok_or(unknown(record))?;
        match event {
            TaskEvent::ProgressNoted(p) => rec.progress = Some(p.clone()),
            TaskEvent::Reported(report) => {
                rec.sent_back = None;
                rec.conflict = None;
                if next
                    .assignment()
                    .is_some_and(|a| a.delegator == Party::Builder)
                {
                    self.chat.push(ChatEntry {
                        seq: s.seq,
                        at: s.at,
                        item: ChatItem::Report {
                            task: id,
                            report: report.clone(),
                        },
                    });
                    ch.part(ProjectPart::Chat);
                }
            }
            TaskEvent::Reviewed(Verdict::SendBack { note, .. }) => {
                rec.sent_back = Some(note.clone());
            }
            TaskEvent::MergeFailed { conflict } => rec.conflict = Some(conflict.clone()),
            TaskEvent::Merged { commit } => rec.merged = Some(commit.clone()),
            _ => {}
        }
        rec.state = next;
        ch.part(ProjectPart::Tasks)
            .part(ProjectPart::Team)
            .record(record);
        Ok(())
    }

    pub(super) fn message_sent(
        &mut self,
        s: Stamp,
        m: &crate::message::Message,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Message(m.id);
        if self.messages.contains_key(&m.id) {
            return Err((Some(record), Corrupt::Duplicate));
        }
        if let Route::Between { from, to } = m.route {
            self.agent_mut(from)?;
            self.agent_mut(to)?;
        }
        if let Some(to) = m.in_reply_to {
            let original = self
                .messages
                .get_mut(&to)
                .ok_or(unknown(Record::Message(to)))?;
            original.reply.get_or_insert(m.id);
            ch.record(Record::Message(to));
        }
        self.messages.insert(
            m.id,
            MessageRecord {
                message: m.clone(),
                at: s.at,
                awaited: false,
                reply: None,
            },
        );
        if !matches!(m.route, Route::Between { .. }) {
            self.chat.push(ChatEntry {
                seq: s.seq,
                at: s.at,
                item: ChatItem::Message(m.id),
            });
            ch.part(ProjectPart::Chat);
        }
        ch.record(record);
        Ok(())
    }

    pub(super) fn action_event(
        &mut self,
        at: Timestamp,
        id: ActionId,
        event: &ActionEvent,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        let record = Record::Action(id);
        let rec = self.actions.get_mut(&id).ok_or(unknown(record))?;
        rec.state = action::next(&rec.state, event).map_err(on(record))?;
        match event {
            ActionEvent::Classified(c) => rec.classification = Some(c.clone()),
            ActionEvent::Held => {
                ch.part(ProjectPart::HeldActions);
            }
            ActionEvent::Approved | ActionEvent::Denied => {
                rec.decided = Some(at);
                ch.part(ProjectPart::HeldActions);
            }
            ActionEvent::Failed { error } => rec.error = Some(error.clone()),
            _ => {}
        }
        ch.record(record);
        Ok(())
    }

    pub(super) fn committed_trial(&self, trial: TrialId) -> Result<(), Fail> {
        let record = Record::Trial(trial);
        match self.trials.get(&trial) {
            None => Err(unknown(record)),
            Some(t) if t.state == TrialState::Committed => Ok(()),
            Some(_) => Err((Some(record), Corrupt::TrialNotCommitted)),
        }
    }
}

/// The assignee of a task in progress.
fn in_progress(state: &TaskState) -> Option<AgentId> {
    match state {
        TaskState::InProgress(a) => Some(a.assignee),
        _ => None,
    }
}

/// An agent's jobs show in its view. The daemon's routines show only in the Log.
fn job_changed(does: &ProjectJob, id: JobId, ch: &mut Ch) {
    ch.record(Record::Job(id));
    match does {
        ProjectJob::Wake {
            scheduled_by,
            agent,
            ..
        } => {
            ch.part(ProjectPart::Team)
                .record(Record::Agent(*scheduled_by))
                .record(Record::Agent(*agent));
        }
        ProjectJob::Delegate {
            scheduled_by, to, ..
        } => {
            ch.part(ProjectPart::Team)
                .record(Record::Agent(*scheduled_by))
                .record(Record::Agent(*to));
        }
        ProjectJob::Routine(_) => {}
    }
}

fn file_changed(id: FileId, attached: bool, ch: &mut Ch) {
    ch.part(ProjectPart::Files).record(Record::File(id));
    if attached {
        ch.part(ProjectPart::Chat);
    }
}
