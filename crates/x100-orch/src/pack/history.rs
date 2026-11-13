//! What the log says about an agent's current task that the folded state does not keep: every
//! daemon tool call made in it with its result, and the commit its work started from.

use x100_core::action::{ActionEvent, Request};
use x100_core::event::{ProjectEvent, Seq};
use x100_core::git::CommitId;
use x100_core::id::{ActionId, AgentId, TaskId};
use x100_core::task::TaskEvent;
use x100_core::tool::ToolCall;
use x100_store::ProjectStore;

use crate::tools::{Tool, reply};

const PAGE: usize = 500;

/// One daemon tool call and how it ended.
pub(crate) struct Call {
    pub action: ActionId,
    pub call: ToolCall,
    /// The result the driver read, or why it was refused. `None` while it is still in flight.
    pub result: Option<Result<String, String>>,
}

impl Call {
    pub fn line(&self) -> String {
        let input = serde_json::to_string(&self.call).unwrap_or_default();
        let result = match &self.result {
            Some(Ok(r)) => format!("-> {r}"),
            Some(Err(e)) => format!("-> refused: {e}"),
            None => "-> still in flight when the last driver stopped".to_owned(),
        };
        format!("- {} {input}\n  {result}", Tool::of(&self.call).name())
    }
}

#[derive(Default)]
pub(crate) struct History {
    pub calls: Vec<Call>,
    /// The agent's branch when the task first reached it, for the diff since.
    pub base: Option<CommitId>,
}

/// Reads the log back to where `task` began, then forward, keeping `agent`'s calls and the first
/// refresh of its branch. The events of one call are appended together, so a call's effects are
/// the events between its allowing and its end.
pub(crate) fn history(store: &ProjectStore, agent: AgentId, task: TaskId) -> History {
    let mut events = Vec::new();
    let mut before: Option<Seq> = None;
    'back: loop {
        let Ok(page) = store.log.read_before(before, PAGE) else {
            break;
        };
        let Some(last) = page.last().map(|e| e.seq) else {
            break;
        };
        before = Some(last);
        for e in page {
            let began = match &e.event {
                ProjectEvent::TaskOpened { task: t, .. } => t.id == task,
                ProjectEvent::Task {
                    task: t,
                    event: TaskEvent::Delegated(_),
                } => *t == task,
                _ => false,
            };
            events.push(e.event);
            if began {
                break 'back;
            }
        }
    }
    events.reverse();
    let mut history = History::default();
    let mut open: Option<(ActionId, usize, Vec<ProjectEvent>)> = None;
    for event in events {
        match &event {
            ProjectEvent::ActionProposed(a) if a.agent == agent => {
                if let Request::Tool { call, .. } = &a.request
                    && !Tool::of(call).reads_only()
                {
                    history.calls.push(Call {
                        action: a.id,
                        call: call.clone(),
                        result: None,
                    });
                    open = Some((a.id, history.calls.len() - 1, Vec::new()));
                }
            }
            ProjectEvent::Action { action, event: end }
                if open.as_ref().is_some_and(|o| o.0 == *action) =>
            {
                let result = match end {
                    ActionEvent::CarriedOut => {
                        let (_, i, effects) = open.take().expect("open");
                        Some((i, Ok(reply(&history.calls[i].call, &effects))))
                    }
                    ActionEvent::Failed { error } => {
                        let (_, i, _) = open.take().expect("open");
                        Some((i, Err(error.clone())))
                    }
                    _ => None,
                };
                if let Some((i, r)) = result {
                    history.calls[i].result = Some(r);
                }
            }
            ProjectEvent::BranchRefreshed {
                agent: a, commit, ..
            } if *a == agent && history.base.is_none() => {
                history.base = Some(commit.clone());
            }
            _ => {
                if let Some((_, _, effects)) = &mut open {
                    effects.push(event.clone());
                }
            }
        }
    }
    history
}
