//! The chat with the CEO: what the builder said, what the CEO answered and asked, and its reports.

use x100_core::agent::AgentState;
use x100_core::fold::ChatItem;
use x100_core::message::Route;
use x100_core::task::Outcome;

use super::Ctx;
use crate::page::{ACCENT, BAD, BOLD, DIM, GOOD, Page, WARN, span, spans};

pub fn page(c: &Ctx) -> Page {
    let mut page = Page::new();
    let files = |ids: &[x100_core::id::FileId]| -> String {
        ids.iter()
            .filter_map(|id| c.p.file(*id))
            .map(|f| format!("[{}]", f.file.name))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let started = c.p.chat().iter().any(|e| match &e.item {
        ChatItem::Message(id) => {
            c.p.message(*id)
                .is_some_and(|m| m.message.route == Route::BuilderToCeo)
        }
        ChatItem::Report { .. } => false,
    });
    if !started {
        page.text("Tell the CEO what you want built. Sending your first message starts the work.");
    }
    for entry in c.p.chat() {
        match &entry.item {
            ChatItem::Message(id) => {
                let Some(m) = c.p.message(*id) else { continue };
                let (who, style) = match m.message.route {
                    Route::BuilderToCeo => ("You", BOLD),
                    Route::DaemonToBuilder => ("x100", WARN),
                    _ => ("CEO", ACCENT.patch(BOLD)),
                };
                let mut body = m.message.body.clone();
                let attached = files(&m.message.attachments);
                if !attached.is_empty() {
                    body.push('\n');
                    body.push_str(&attached);
                }
                let tag = match (m.message.route, m.awaited, m.reply) {
                    (Route::CeoToBuilder, true, None) => "  (asks you)",
                    _ => "",
                };
                push_message(&mut page, who, style, &body, tag, c.width);
            }
            ChatItem::Report { task, report } => {
                let title =
                    c.p.task(*task)
                        .map_or_else(|| "a task".into(), |t| t.task.title.clone());
                let (word, style) = match report.outcome {
                    Outcome::Completed => ("done", GOOD),
                    _ => ("failed", BAD),
                };
                page.line(spans(vec![
                    span("CEO report ", ACCENT.patch(BOLD)),
                    span(format!("on \u{201c}{title}\u{201d}, "), Default::default()),
                    span(word, style),
                ]));
                page.paragraph(&report.summary, "  ", c.width);
            }
        }
        page.blank();
    }
    let working =
        c.p.project()
            .and_then(|p| c.p.agent(p.ceo))
            .is_some_and(|a| a.state == AgentState::Working);
    if working {
        page.styled("CEO is working…", DIM);
    }
    page
}

fn push_message(
    page: &mut Page,
    who: &str,
    style: ratatui::style::Style,
    body: &str,
    tag: &str,
    width: usize,
) {
    let mut first = true;
    for line in crate::page::wrap(body, width.saturating_sub(7).max(10)) {
        if first {
            let mut parts = vec![
                span(format!("{who:<6}"), style),
                span(" ", Default::default()),
                span(line, Default::default()),
            ];
            if !tag.is_empty() {
                parts.push(span(tag.to_owned(), WARN));
            }
            page.line(spans(parts));
            first = false;
        } else {
            page.text(format!("       {line}"));
        }
    }
}
