//! `set_up_project`: the CEO names a new project once it knows what the project is. The first call
//! sets it up and opens the builder's task from the conversation; a later one renames it.

use x100_core::event::ProjectEvent;
use x100_core::id::FileId;
use x100_core::message::Route;
use x100_core::party::Actor;
use x100_core::project::Identity;

use super::{At, NewCodeDir, Plan, Refusal};
use crate::builder::builder_task;
use crate::settle::deliver;

pub(super) fn set_up(
    at: &At,
    name: &str,
    intent: &str,
    brief: Option<&str>,
) -> Result<Plan, Refusal> {
    let (name, intent) = (name.trim(), intent.trim());
    if name.is_empty() {
        return Err("`name` is empty: give the project a short name".into());
    }
    if intent.is_empty() {
        return Err("`intent` is empty: say in one line what the project is for".into());
    }
    let identity = Identity {
        name: name.to_owned(),
        intent: intent.to_owned(),
    };
    let me = Actor::Agent(at.id());
    if let Some(current) = &at.project.identity {
        if *current == identity {
            return Err("the project already has this name and intent".into());
        }
        return Ok(Plan {
            events: vec![(me, ProjectEvent::Renamed(identity))],
            ..Plan::default()
        });
    }
    let brief = brief.map(str::trim).filter(|b| !b.is_empty()).ok_or(
        "the first call sets the project up and opens your task, so it needs `brief`: what the \
         builder asks for, as you understand it",
    )?;
    let code_dir = match &at.project.code_dir {
        Some(_) => None,
        None => {
            let root = at.ctx.settings.projects_root.clone().ok_or(
                "x100 has no projects root set, so the project has nowhere to keep its code",
            )?;
            Some(NewCodeDir {
                root,
                slug: slug(name),
            })
        }
    };
    let ceo = at.id();
    let (task, opened) = builder_task(at.state, at.ctx.clock, ceo, brief, attachments(at));
    Ok(Plan {
        events: vec![
            (me, ProjectEvent::SetUp(identity)),
            (me, opened.1),
            deliver(
                at.ctx.clock,
                ceo,
                x100_core::inbox::InboxItem::Task { task },
            ),
        ],
        code_dir,
        ..Plan::default()
    })
}

/// Every file the builder attached to a message so far, once each, in the order sent.
fn attachments(at: &At) -> Vec<FileId> {
    let mut out: Vec<FileId> = Vec::new();
    for m in at.state.messages() {
        if m.message.route == Route::BuilderToCeo {
            for f in &m.message.attachments {
                if !out.contains(f) {
                    out.push(*f);
                }
            }
        }
    }
    out
}

/// A directory name from a project's name: lower case letters and digits, words joined by `-`.
pub(super) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "project".to_owned()
    } else {
        out.to_owned()
    }
}

#[cfg(test)]
mod tests;
