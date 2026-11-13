//! Drawing. Everything on screen is built from the client's state each time, so what is shown is
//! what the client holds.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use x100_client::words;

use crate::app::{App, Prompt, TABS, Tab, Where};
use crate::input::Input;
use crate::page::{ACCENT, BAD, BOLD, DIM, GOOD, WARN, clip};
use crate::pages::follows_end;

/// The text of an input broken to `width` columns, and where its cursor falls in it.
fn layout(input: &Input, width: usize, masked: bool) -> (Vec<String>, (u16, u16)) {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut cursor = (0usize, 0usize);
    let mut row_len = 0;
    let n = input.text().chars().count();
    for (i, c) in input.text().chars().enumerate() {
        if i == input.cursor() {
            cursor = (lines.len() - 1, row_len);
        }
        if c == '\n' {
            lines.push(String::new());
            row_len = 0;
            continue;
        }
        if row_len == width {
            lines.push(String::new());
            row_len = 0;
        }
        lines
            .last_mut()
            .expect("a line")
            .push(if masked { '•' } else { c });
        row_len += 1;
    }
    if input.cursor() >= n {
        cursor = (lines.len() - 1, row_len.min(width));
    }
    (lines, (cursor.1 as u16, cursor.0 as u16))
}

fn tab_bar(app: &App) -> Line<'static> {
    let Some(ui) = app.project_ui() else {
        return Line::default();
    };
    let held = app
        .client
        .project(ui.id)
        .map_or(0, |p| p.held_actions().count());
    let mut spans = Vec::new();
    for (i, tab) in TABS.iter().enumerate() {
        let mut name = format!("{} {}", (i + 1) % 10, tab.name());
        if *tab == Tab::Held && held > 0 {
            name.push_str(&format!("({held})"));
        }
        if ui.dots.contains(tab) {
            name.push('•');
        }
        let style = if *tab == ui.tab {
            ACCENT.add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else if ui.dots.contains(tab) || (*tab == Tab::Held && held > 0) {
            WARN
        } else {
            DIM
        };
        spans.push(Span::styled(format!(" {name} "), style));
    }
    Line::from(spans)
}

fn hints(app: &App) -> String {
    match &app.at {
        Where::Projects => "Enter opens · n new project · q quits".into(),
        Where::New => "Enter sends · Ctrl-D starts on a folder · Esc back".into(),
        Where::Project(ui) => {
            let tab = match ui.tab {
                Tab::Chat => "Enter sends · Ctrl-A attaches · PgUp/PgDn scroll",
                Tab::Held => "a approve · d deny",
                Tab::Team => match &ui.agent {
                    Some(_) => {
                        "Esc back · c o w m k t switch what is shown · r replaces its driver"
                    }
                    None if ui.drivers => "s stop · r replace driver · d agents",
                    None => "Enter opens · d drivers",
                },
                Tab::Specs => "←/→ switch document · ↑/↓ scroll",
                Tab::Tasks => "n raise a task",
                Tab::Files => "u add · x remove",
                Tab::Budget => "b project budget · e lead budget",
                Tab::Log => "↑/↓ PgUp/PgDn scroll",
                Tab::Preview => "",
                Tab::Settings => "Enter changes · c credential · v value · x remove · r range",
            };
            format!("{tab}  ·  Tab next · p projects · q quits")
        }
    }
}

impl App {
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        match &self.at {
            Where::Projects => self.draw_list(frame, area),
            Where::New => self.draw_new(frame, area),
            Where::Project(_) => self.draw_project(frame, area),
        }
    }

    fn footer(&self, frame: &mut Frame, area: Rect) {
        let line = match &self.notice {
            Some(n) => Line::styled(
                clip(&n.text, area.width as usize),
                if n.error { BAD } else { GOOD },
            ),
            None => Line::styled(clip(&hints(self), area.width as usize), DIM),
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_list(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(Line::styled("x100 · projects", BOLD)),
            rows[0],
        );
        let ids: Vec<_> = {
            let mut v: Vec<_> = self.client.with_state(|s| {
                s.projects
                    .iter()
                    .map(|(id, p)| (p.project().map(|x| x.created), *id))
                    .collect()
            });
            v.sort();
            v.into_iter().map(|(_, id)| id).collect()
        };
        let mut lines = Vec::new();
        self.client.with_state(|s| {
            for (i, id) in ids.iter().enumerate() {
                let Some(p) = s.projects.get(id) else {
                    continue;
                };
                let live = if p.is_live() { "● " } else { "  " };
                let text = format!(
                    "{live}{:<28}{}",
                    words::project_name(Some(p)),
                    words::agents(p.agent_count())
                );
                let style = if i == self.list_sel {
                    Style::new().add_modifier(Modifier::REVERSED)
                } else {
                    Style::new()
                };
                lines.push(Line::styled(text, style));
            }
        });
        if lines.is_empty() {
            lines.push(Line::styled("No projects yet. Press n to start one.", DIM));
        }
        frame.render_widget(Paragraph::new(lines), rows[1]);
        self.footer(frame, rows[2]);
    }

    fn draw_new(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(area);
        frame.render_widget(Paragraph::new(Line::styled("x100", BOLD)), rows[0]);
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw("Tell the CEO what you want to build."),
                Line::styled("Sending the first message starts the project.", DIM),
            ]),
            rows[1],
        );
        if let Some(dir) = &self.new_dir {
            frame.render_widget(
                Paragraph::new(Line::styled(format!("Starts on the folder {dir}"), DIM)),
                rows[2],
            );
        }
        let (lines, (cx, cy)) = layout(
            &self.new_input,
            (rows[3].width as usize).saturating_sub(2),
            false,
        );
        let block = Block::default().borders(Borders::ALL).border_style(ACCENT);
        let inner = block.inner(rows[3]);
        frame.render_widget(
            Paragraph::new(lines.into_iter().map(Line::raw).collect::<Vec<_>>()).block(block),
            rows[3],
        );
        if self.new_prompt.is_none() {
            frame.set_cursor_position(Position::new(
                inner.x + cx,
                inner.y + cy.min(inner.height.saturating_sub(1)),
            ));
        }
        if let Some(p) = &self.new_prompt {
            let cursor = prompt_lines(frame, p, rows[4]);
            frame.set_cursor_position(cursor);
        }
        self.footer(frame, rows[5]);
    }

    fn draw_project(&mut self, frame: &mut Frame, area: Rect) {
        let width = area.width as usize;
        let header = self.header(width);
        let prompt_height = if matches!(self.project_ui(), Some(u) if u.prompt.is_some()) {
            2
        } else {
            0
        };
        let chat = matches!(self.project_ui(), Some(u) if u.tab == Tab::Chat);
        let input_height: u16 = if chat {
            let ui = self.project_ui().expect("a project is open");
            let (lines, _) = layout(&ui.input, width.saturating_sub(2), false);
            (lines.len() as u16 + 2).min(7) + u16::from(!ui.attachments.is_empty())
        } else {
            0
        };
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(header.len() as u16),
            Constraint::Min(2),
            Constraint::Length(input_height),
            Constraint::Length(prompt_height),
            Constraint::Length(1),
        ])
        .split(area);

        let (name, agents) = self
            .project_ui()
            .map(|ui| {
                let p = self.client.project(ui.id);
                (
                    words::project_name(p.as_ref()),
                    p.map_or(0, |p| p.agent_count()),
                )
            })
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("x100 · {name}"), BOLD),
                Span::styled(format!(" · {}", words::agents(agents)), DIM),
            ])),
            rows[0],
        );
        frame.render_widget(Paragraph::new(tab_bar(self)), rows[1]);
        frame.render_widget(Paragraph::new(header), rows[2]);

        let body = rows[3];
        self.body = (body.width, body.height);
        self.draw_body(frame, body);

        let mut cursor = None;
        if chat {
            cursor = Some(self.draw_input(frame, rows[4]));
        }
        if let Some(p) = self.project_ui().and_then(|u| u.prompt.as_ref()) {
            cursor = Some(prompt_lines(frame, p, rows[5]));
        }
        if let Some(c) = cursor {
            frame.set_cursor_position(c);
        }
        self.footer(frame, rows[6]);
    }

    fn draw_body(&mut self, frame: &mut Frame, area: Rect) {
        let Some(page) = self.page(area.width as usize) else {
            return;
        };
        let h = area.height as usize;
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        let tail = follows_end(ui);
        let len = page.items.len();
        let targets = page.targets();
        let want = ui
            .sel
            .get(&ui.tab)
            .copied()
            .unwrap_or(0)
            .min(targets.len().saturating_sub(1));
        ui.sel.insert(ui.tab, want);
        let selected_line = targets.get(want).map(|(i, _)| *i);
        let off = ui.off.entry(ui.tab).or_insert(0);
        let max_top = len.saturating_sub(h);
        let top = if tail {
            *off = (*off).min(max_top);
            max_top - *off
        } else {
            let mut top = (*off).min(max_top);
            if let Some(line) = selected_line {
                if line < top {
                    top = line;
                } else if line >= top + h {
                    top = line + 1 - h;
                }
            }
            *off = top;
            top
        };
        let lines: Vec<Line> = page
            .items
            .iter()
            .enumerate()
            .skip(top)
            .take(h)
            .map(|(i, item)| {
                let mut line = item.line.clone();
                if Some(i) == selected_line {
                    line = line.patch_style(Style::new().add_modifier(Modifier::REVERSED));
                }
                line
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);

        let agent_output = matches!(&ui.agent, Some(a) if a.mode == crate::app::AgentMode::Output);
        if ui.tab == Tab::Team && agent_output {
            let id = ui.id;
            let client = self.client.clone();
            if let (Some(a), Some(p)) = (
                self.project_ui_mut().and_then(|u| u.agent.as_mut()),
                client.project(id),
            ) {
                a.output.resize(&p, area.width, area.height);
            }
        }
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) -> Position {
        let ui = self.project_ui().expect("a project is open");
        let mut area = area;
        if !ui.attachments.is_empty() {
            let names = ui
                .attachments
                .iter()
                .map(|f| format!("[{}]", f.name))
                .collect::<Vec<_>>()
                .join(" ");
            frame.render_widget(
                Paragraph::new(Line::styled(
                    clip(&format!("Attached: {names}"), area.width as usize),
                    DIM,
                )),
                Rect { height: 1, ..area },
            );
            area.y += 1;
            area.height = area.height.saturating_sub(1);
        }
        let (lines, (cx, cy)) = layout(&ui.input, (area.width as usize).saturating_sub(2), false);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(ACCENT)
            .title("Message the CEO");
        let inner = block.inner(area);
        let skip = (cy as usize).saturating_sub(inner.height.saturating_sub(1) as usize);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(skip)
                    .map(Line::raw)
                    .collect::<Vec<_>>(),
            )
            .block(block),
            area,
        );
        Position::new(inner.x + cx, inner.y + cy - skip as u16)
    }
}

/// A one-line question and its answer box; returns where the cursor goes.
fn prompt_lines(frame: &mut Frame, p: &Prompt, area: Rect) -> Position {
    let (lines, (cx, _)) = layout(&p.input, (area.width as usize).saturating_sub(3), p.masked);
    let shown = lines.first().cloned().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(clip(&p.title, area.width as usize), WARN),
            Line::from(vec![Span::styled("> ", ACCENT), Span::raw(shown)]),
        ]),
        area,
    );
    Position::new(area.x + 2 + cx, area.y + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_drawn_as_dots() {
        let mut input = Input::default();
        input.insert_str("hunter2");
        let (lines, cursor) = layout(&input, 20, true);
        assert_eq!(lines, ["•••••••"]);
        assert_eq!(cursor, (7, 0));
    }
}
