//! The Tasks tab: a flat list in the order tasks were opened.

use x100_core::fold::ProjectState;
use x100_core::id::TaskId;
use x100_core::party::Party;
use x100_core::task::TaskState;

use super::Ctx;
use crate::page::{DIM, GOOD, Page, Target, cell, span, spans};

pub fn status(state: &TaskState) -> &'static str {
    match state {
        TaskState::NewIdea => "New idea",
        TaskState::InProgress(_) => "In progress",
        TaskState::InReview(..) => "In review",
        TaskState::Promoting(..) => "Promoting",
        TaskState::Done(..) => "Done",
    }
}

/// Every task, in the order it was opened.
pub fn ordered(p: &ProjectState) -> Vec<TaskId> {
    let mut tasks: Vec<_> = p.tasks().collect();
    tasks.sort_by_key(|t| t.seq);
    tasks.into_iter().map(|t| t.task.id).collect()
}

pub fn page(c: &Ctx) -> Page {
    let mut page = Page::new();
    page.styled("n raises a task as a new idea.", DIM);
    page.styled(
        format!(
            "{}{}{}{}{}",
            cell("#", 4),
            cell("TITLE", 38),
            cell("STATUS", 13),
            cell("PART", 14),
            "RAISED BY"
        ),
        DIM,
    );
    for (i, id) in ordered(c.p).into_iter().enumerate() {
        let Some(t) = c.p.task(id) else { continue };
        let raised = match t.task.raised_by {
            Party::Builder => "You".to_owned(),
            Party::Agent(a) => super::role(c.p, a),
        };
        let live = if c.p.task_is_live(id) { "● " } else { "  " };
        page.row(
            spans(vec![
                span(cell(&(i + 1).to_string(), 4), DIM),
                span(live, GOOD),
                span(cell(&t.task.title, 36), Default::default()),
                span(cell(status(&t.state), 13), Default::default()),
                span(cell(t.task.part.as_deref().unwrap_or("-"), 14), DIM),
                span(raised, DIM),
            ]),
            Target::Task(id),
        );
    }
    if page.targets().is_empty() {
        page.text("No tasks yet.");
    }
    page
}

#[cfg(test)]
mod tests {
    use x100_core::event::{Envelope, ProjectEvent, Seq};
    use x100_core::fold::DaemonState;
    use x100_core::testing::fernleaf;

    use super::*;

    /// Two tasks opened in the same millisecond, the second with the smaller id, are listed as
    /// the log opened them.
    #[test]
    fn tasks_opened_in_the_same_millisecond_are_listed_as_the_log_opened_them() {
        let log = fernleaf().log;
        let mut p = ProjectState::fold(&log).expect("the scenario folds");
        let last = log.last().unwrap();
        let (task, opening) = log
            .iter()
            .find_map(|e| match &e.event {
                ProjectEvent::TaskOpened { task, opening } => Some((task.clone(), opening.clone())),
                _ => None,
            })
            .unwrap();
        for (n, (id, title)) in [(u128::MAX - 1, "OPENED-FIRST"), (1, "OPENED-SECOND")]
            .into_iter()
            .enumerate()
        {
            let mut task = task.clone();
            task.id = TaskId::from_u128(id);
            task.title = title.into();
            let e = Envelope {
                seq: Seq(last.seq.0 + 1 + n as u64),
                at: last.at,
                by: last.by,
                event: ProjectEvent::TaskOpened {
                    task,
                    opening: opening.clone(),
                },
            };
            p.apply(&e).expect("the task opens");
        }
        let d = DaemonState::default();
        let c = Ctx {
            p: &p,
            d: &d,
            width: 120,
            colours: crate::look::Colours::Basic,
        };
        let text = page(&c).plain();
        let at = |title: &str| {
            text.find(title)
                .unwrap_or_else(|| panic!("{title} in\n{text}"))
        };
        assert!(at("OPENED-FIRST") < at("OPENED-SECOND"), "{text}");
    }
}
