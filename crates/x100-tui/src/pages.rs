//! The page the open tab shows, built from the state the client holds.

use ratatui::text::Line;

use crate::app::{AgentMode, App, ProjectUi, Tab};
use crate::output::{Output, Segment};
use crate::page::{DIM, Page, WARN};
use crate::views::{self, Ctx};

/// Whether a tab shows its newest lines at the bottom and is read upwards.
pub fn follows_end(ui: &ProjectUi) -> bool {
    match ui.tab {
        Tab::Chat => true,
        Tab::Team => matches!(&ui.agent, Some(a) if a.mode == AgentMode::Output),
        _ => false,
    }
}

fn output_page(o: &Output) -> Page {
    let mut page = Page::new();
    if o.segments.is_empty() {
        page.styled("No driver has worked for this agent yet.", DIM);
    }
    for s in &o.segments {
        match s {
            Segment::Log(log) => {
                for line in log.lines() {
                    page.line(line);
                }
            }
            Segment::Screen(s) => {
                page.styled(format!("━━ {}", s.marker), WARN);
                for row in s.rows() {
                    page.text(row.trim_end().to_owned());
                }
            }
        }
    }
    page
}

impl App {
    /// The lines above the page: the trail of an open agent.
    pub fn header(&self, width: usize) -> Vec<Line<'static>> {
        let Some(ui) = self.project_ui() else {
            return Vec::new();
        };
        let (Tab::Team, Some(agent)) = (ui.tab, &ui.agent) else {
            return Vec::new();
        };
        self.client.with_state(|s| {
            let Some(p) = s.projects.get(&ui.id) else {
                return Vec::new();
            };
            let c = Ctx {
                p,
                d: &s.daemon,
                width,
                colours: self.colours,
            };
            let mut lines = views::team::header(&c, agent.id);
            let modes = [
                (AgentMode::Children, "c agents"),
                (AgentMode::Output, "o output"),
                (AgentMode::Waits, "w waits and jobs"),
                (AgentMode::Memories, "m notes"),
                (AgentMode::Skills, "k skills"),
                (AgentMode::Trials, "t trials"),
            ];
            let bar = modes
                .iter()
                .map(|(m, name)| {
                    if *m == agent.mode {
                        format!("[{name}]")
                    } else {
                        format!(" {name} ")
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(Line::styled(bar, DIM));
            lines
        })
    }

    /// The open tab's page; `None` when no project is open.
    pub fn page(&self, width: usize) -> Option<Page> {
        let ui = self.project_ui()?;
        Some(self.client.with_state(|s| {
            let Some(p) = s.projects.get(&ui.id) else {
                return Page::new();
            };
            let c = Ctx {
                p,
                d: &s.daemon,
                width,
                colours: self.colours,
            };
            match ui.tab {
                Tab::Chat => views::chat::page(&c),
                Tab::Held => views::held::page(&c),
                Tab::Team => match &ui.agent {
                    None if ui.drivers => views::team::drivers(&c),
                    None => views::team::page(&c),
                    Some(a) => match a.mode {
                        AgentMode::Children => views::team::children(&c, a.id),
                        AgentMode::Output => output_page(&a.output),
                        AgentMode::Waits => views::team::waits(&c, a.id),
                        AgentMode::Memories => views::agent::memories(&c, &a.memories),
                        AgentMode::Skills => views::agent::skills(&c, a.id, &a.skills),
                        AgentMode::Trials => views::agent::trials(&c, a.id),
                    },
                },
                Tab::Specs => views::specs::page(&c, ui.spec),
                Tab::Tasks => views::tasks::page(&c),
                Tab::Files => views::files::page(&c),
                Tab::Budget => views::budget::page(&c),
                Tab::Log => views::log::page(&ui.log, width),
                Tab::Preview => views::preview::page(&c),
                Tab::Settings => views::settings::page(&c, &self.integrations),
            }
        }))
    }

    pub fn selected(&self) -> Option<crate::page::Target> {
        let ui = self.project_ui()?;
        let page = self.page(self.body.0 as usize)?;
        let targets = page.targets();
        let i = ui.sel.get(&ui.tab).copied().unwrap_or(0);
        targets
            .get(i.min(targets.len().saturating_sub(1)))
            .map(|(_, t)| (*t).clone())
    }
}
