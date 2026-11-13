//! Reflect now, and the file and log work of reflection's calls that need no lab: reading the log,
//! correcting a note, and committing a change straight to main when the project does not trial
//! changes. Each call returns the events it caused and what the CEO reads. Everything in the
//! repositories goes through `x100-memory`.

use x100_core::event::{ProjectEvent, Seq};
use x100_core::id::{AgentId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::memory::NoteName;
use x100_core::party::Actor;
use x100_core::time::Timestamp;
use x100_core::trial::TrialChange;
use x100_memory::{Memory, Target};
use x100_store::ProjectStore;

use crate::builder::Refused;
use crate::recall::{Done, readable};
use crate::tools::{READ_LOG_MAX, who};

/// How much of an event without words of its own `read_log` shows, in characters.
const RECORD_CHARS: usize = 600;

/// Events `from` to `to` of the log, one line each, at most [`READ_LOG_MAX`].
pub(crate) fn read_log(store: &ProjectStore, from: Seq, to: u64) -> Result<Done, String> {
    let wanted = (to + 1 - from.0).min(READ_LOG_MAX);
    let events = store
        .log
        .read_after(Some(Seq(from.0 - 1)), wanted as usize)
        .map_err(|e| format!("the log could not be read: {e}"))?;
    let shown = events.last().map_or(from.0, |e| e.seq.0);
    let mut lines: Vec<String> = store.log.with_state(|state| {
        events
            .iter()
            .map(|e| {
                let by = match e.by {
                    x100_core::party::Actor::Builder => "the builder".to_owned(),
                    x100_core::party::Actor::Daemon => "the daemon".to_owned(),
                    x100_core::party::Actor::Agent(a) => {
                        who(state, x100_core::party::Party::Agent(a))
                    }
                };
                let what = match readable(&e.event) {
                    Some((kind, text)) => format!("{kind}: {text}"),
                    None => clip(&serde_json::to_string(&e.event).unwrap_or_default()),
                };
                format!("#{} at {} by {by}: {what}", e.seq.0, e.at.as_millis())
            })
            .collect()
    });
    if shown < to {
        lines.push(format!(
            "(more: call read_log again from {} to read on)",
            shown + 1
        ));
    }
    Ok(Done {
        events: Vec::new(),
        reply: lines.join("\n"),
    })
}

fn clip(text: &str) -> String {
    if text.chars().count() <= RECORD_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(RECORD_CHARS).collect();
    format!("{cut}...")
}

fn failed(e: x100_memory::Error) -> String {
    e.to_string()
}

/// Rewrites a note the log shows to be wrong, as one commit on main. It is never put on trial.
pub(crate) fn correct(
    memory: &Memory,
    agent: AgentId,
    note: &NoteName,
    body: &str,
    sources: &[Seq],
    now: Timestamp,
) -> Result<Done, String> {
    let old = memory.note(agent, note).map_err(|e| match e {
        x100_memory::Error::NotFound(_) => format!(
            "{agent} has no note {}: a correction rewrites a note that is there",
            note.0
        ),
        other => failed(other),
    })?;
    let (name, commit) = memory
        .remember(agent, &old.name, body, sources, now)
        .map_err(failed)?;
    let reply = serde_json::json!({ "corrected": name, "commit": commit.0 }).to_string();
    Ok(Done {
        events: vec![ProjectEvent::MemoryWritten {
            agent,
            note: name,
            commit,
        }],
        reply,
    })
}

/// Commits reflection's change straight to main: a skill as one commit, a compaction as one
/// commit once it is found to leave the memory smaller with an index that lists every note.
pub(crate) fn commit(
    memory: &Memory,
    change: &TrialChange,
    now: Timestamp,
) -> Result<Done, String> {
    match change {
        TrialChange::EditSkill { skill, body } | TrialChange::NewSkill { skill, body } => {
            let new = matches!(change, TrialChange::NewSkill { .. });
            skill_exists(memory, skill, new)?;
            let message = format!("skill {}: reflection's change", skill.name.0);
            let (version, commit) = memory
                .write_skill(skill, body, &Target::Main, &message)
                .map_err(failed)?
                .ok_or_else(|| format!("{} already says that", skill.name.0))?;
            let reply = serde_json::json!({
                "committed": skill.name.0, "version": version.0, "commit": commit.0
            })
            .to_string();
            Ok(Done {
                events: vec![ProjectEvent::SkillCommitted {
                    skill: skill.clone(),
                    version,
                    commit,
                    trial: None,
                }],
                reply,
            })
        }
        TrialChange::CompactMemory { agent, notes } => {
            let changes = memory.compaction(*agent, notes).map_err(failed)?;
            let branch = format!("trial/direct-{}", now.as_millis());
            let target = Target::Branch(branch.clone());
            let message = "reflection's compaction";
            memory
                .change_notes(*agent, &target, &changes, now, message)
                .map_err(failed)?;
            let sizes = (
                memory.size_on(*agent, &Target::Main).map_err(failed)?,
                memory.size_on(*agent, &target).map_err(failed)?,
            );
            if let Err(why) = x100_lab::rule::compaction(size(sizes.0), size(sizes.1)) {
                memory
                    .discard(x100_core::memory::SkillScope::Agent(*agent), &branch)
                    .map_err(failed)?;
                return Err(why);
            }
            let commit = memory.merge_notes(*agent, &branch, now).map_err(failed)?;
            let reply = serde_json::json!({
                "compacted": agent, "bytes_before": sizes.0.bytes,
                "bytes_after": sizes.1.bytes, "commit": commit.0
            })
            .to_string();
            Ok(Done {
                events: vec![ProjectEvent::MemoryCompacted {
                    agent: *agent,
                    commit,
                    trial: None,
                }],
                reply,
            })
        }
    }
}

/// An edit needs the skill on main, and a new skill needs its name free.
pub(crate) fn skill_exists(
    memory: &Memory,
    skill: &x100_core::memory::SkillRef,
    new: bool,
) -> Result<(), String> {
    match (memory.skill(skill, &Target::Main), new) {
        (Ok(_), false) | (Err(x100_memory::Error::NotFound(_)), true) => Ok(()),
        (Ok(_), true) => Err(format!(
            "there is a skill {} already: propose an edit of it",
            skill.name.0
        )),
        (Err(x100_memory::Error::NotFound(_)), false) => Err(format!(
            "there is no skill {} to edit: propose it as a new skill",
            skill.name.0
        )),
        (Err(e), _) => Err(failed(e)),
    }
}

/// The measure the rule for compactions reads.
pub(crate) fn size(s: x100_memory::Size) -> x100_lab::rule::MemorySize {
    x100_lab::rule::MemorySize {
        bytes: s.bytes,
        unlisted: s.unlisted,
    }
}

/// What the first line of a reflection task says, which is also its title.
pub(crate) const REFLECTION: &str = "Reflect on the last piece of work";

/// Reflect now: the builder delegates a reflection task to the CEO, over the log since the last
/// one. Refused before the project is set up, and while the CEO holds another task of the
/// builder's, since reflection is on finished work. Blocks.
pub(crate) fn reflect(ctx: &crate::ctx::ProjectCtx) -> Result<x100_core::id::TaskId, Refused> {
    let since = last_reflection(ctx);
    ctx.commit(|s, clock| {
        let decided = || -> Result<(TaskId, Vec<(Actor, ProjectEvent)>), Refused> {
            let p = s
                .project()
                .ok_or_else(|| Refused::NotFound("the project has no CEO".into()))?;
            if !p.is_set_up() || p.code_dir.is_none() {
                return Err(Refused::WrongState(
                    "the project is not set up yet, so there is no work to reflect on".into(),
                ));
            }
            if let Some(task) = crate::builder::builder_task_open(s, p.ceo) {
                return Err(Refused::WrongState(format!(
                    "the CEO is still working on {task}: reflect once it is done"
                )));
            }
            let to = s.seq().map_or(1, |q| q.0);
            let from = since.map_or(1, |q| (q.0 + 1).min(to));
            let ask = ask(from, to, p.settings.trial_before_commit, &accepted(s));
            let (task, opened) = crate::builder::builder_task(s, clock, p.ceo, &ask, Vec::new());
            let told = crate::settle::deliver(clock, p.ceo, InboxItem::Task { task });
            Ok((task, vec![opened, told]))
        };
        match decided() {
            Ok((task, events)) => (events, Ok(task)),
            Err(e) => (Vec::new(), Err(e)),
        }
    })?
}

/// Where the last reflection task was opened in the log, if there was one.
fn last_reflection(ctx: &crate::ctx::ProjectCtx) -> Option<Seq> {
    let mut before = None;
    loop {
        let page = ctx.store.log.read_before(before, 500).ok()?;
        let last = page.last()?.seq;
        for e in &page {
            if let ProjectEvent::TaskOpened { task, .. } = &e.event
                && task.raised_by == x100_core::party::Party::Builder
                && task.title == REFLECTION
            {
                return Some(e.seq);
            }
        }
        before = Some(last);
    }
}

/// How many accepted tasks a reflection task lists.
const ACCEPTED: usize = 20;

/// The newest tasks that ended in an accepted result, reflections aside, as lines a trial's eval
/// set can name them by.
fn accepted(state: &x100_core::fold::ProjectState) -> String {
    let mut done: Vec<_> = state
        .tasks()
        .filter(|t| {
            matches!(&t.state, x100_core::task::TaskState::Done(_, r)
                if r.outcome == x100_core::task::Outcome::Completed)
                && t.task.title != REFLECTION
        })
        .collect();
    done.sort_by_key(|t| std::cmp::Reverse(t.seq));
    done.iter()
        .take(ACCEPTED)
        .map(|t| format!("Accepted: {} ({})\n", t.task.id, t.task.title))
        .collect()
}

fn ask(from: u64, to: u64, trials: bool, accepted: &str) -> String {
    let accepted = if accepted.is_empty() {
        "No task has ended accepted yet, so there is nothing to replay.\n".to_owned()
    } else {
        format!("Tasks that ended accepted, newest first, which a trial can replay:\n{accepted}")
    };
    let setting = if trials {
        "on: each skill change or compaction you propose is tried in the lab, on past tasks or \
         probe questions, with and without it, and committed only if it helps"
    } else {
        "off: each skill change or compaction you propose is committed at once, as a commit the \
         builder can see and revert"
    };
    format!(
        "{REFLECTION}\n\n\
         Read the log of the work, events {from} to {to}, with `read_log`, and what your agents \
         remember, with `read_memories`. The log is the ground truth. Then:\n\
         - Check each note against the log. Correct a note the log shows to be wrong with \
         `propose_change` (a correction), citing the events that show it. Corrections are never \
         put on trial.\n\
         - Compact a memory that has grown, keeping what is used: `propose_change` (a trial of \
         compact_memory), with probe questions whose answers are in the notes now.\n\
         - Improve a skill that would have helped, or write a new one: `propose_change` (a trial \
         of edit_skill or new_skill), with past tasks that ended accepted and checks to run on \
         them.\n\
         - Propose a change to the team only with the telemetry that shows the need: \
         `propose_change` (team). The builder decides it.\n\
         The project's \"trial before commit\" setting is {setting}. Quote decisions and terms \
         word for word rather than summarise them. Report this task when you are done, saying \
         what you changed and why.\n\n{accepted}"
    )
}
