//! Keys. A tab's keys act on the line selected in its page.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use x100_core::action::Decision;
use x100_core::id::AgentId;
use x100_core::message::Route;
use x100_proto::{ReplaceDriver, StopDriver};

use crate::app::{AgentMode, AgentUi, App, Prompt, TABS, Tab, Then, Where};
use crate::input::Input;
use crate::msg::Msg;
use crate::output::Output;
use crate::page::Target;
use crate::pages::follows_end;

enum Typed {
    Pending,
    Cancelled,
    Done(String),
}

fn typing(prompt: &mut Prompt, key: KeyEvent) -> Typed {
    match key.code {
        KeyCode::Esc => Typed::Cancelled,
        // Alt-Enter is a new line, as in the chat; a skill's text has several.
        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => {
            Typed::Done(prompt.input.take())
        }
        _ => {
            prompt.input.key(key);
            Typed::Pending
        }
    }
}

pub fn prompt(title: &str, then: Then) -> Prompt {
    Prompt {
        title: title.to_owned(),
        input: Input::default(),
        masked: false,
        then,
    }
}

impl App {
    pub fn key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.notice = None;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        match &self.at {
            Where::Projects => self.key_list(key),
            Where::New => self.key_new(key),
            Where::Project(_) => self.key_project(key),
        }
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.at {
            Where::New => match &mut self.new_prompt {
                Some(p) => p.input.insert_str(text),
                None => self.new_input.insert_str(text),
            },
            Where::Project(ui) => match &mut ui.prompt {
                Some(p) => p.input.insert_str(text),
                None if ui.tab == Tab::Chat => ui.input.insert_str(text),
                None => {}
            },
            Where::Projects => {}
        }
    }

    fn project_ids(&self) -> Vec<x100_core::id::ProjectId> {
        let mut ids: Vec<_> = self.client.with_state(|s| {
            s.projects
                .iter()
                .map(|(id, p)| (p.project().map(|x| x.created), *id))
                .collect::<Vec<_>>()
        });
        ids.sort();
        ids.into_iter().map(|(_, id)| id).collect()
    }

    fn key_list(&mut self, key: KeyEvent) {
        let ids = self.project_ids();
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') => {
                self.at = Where::New;
                self.saved();
            }
            KeyCode::Up | KeyCode::Char('k') => self.list_sel = self.list_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.list_sel = (self.list_sel + 1).min(ids.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                if let Some(id) = ids.get(self.list_sel) {
                    self.open_project(*id);
                }
            }
            _ => {}
        }
    }

    fn key_new(&mut self, key: KeyEvent) {
        if let Some(p) = &mut self.new_prompt {
            match typing(p, key) {
                Typed::Pending => {}
                Typed::Cancelled => self.new_prompt = None,
                Typed::Done(text) => {
                    self.new_prompt = None;
                    self.new_dir = Some(text.trim().to_owned()).filter(|d| !d.is_empty());
                }
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if !self.project_ids().is_empty() {
                    self.at = Where::Projects;
                    self.saved();
                }
            }
            KeyCode::Char('d') if ctrl => {
                self.new_prompt = Some(prompt(
                    "Start on an existing folder (absolute path, empty for none)",
                    Then::NewProjectDir,
                ));
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => {
                let text = self.new_input.text().trim().to_owned();
                if !text.is_empty() {
                    self.start_project(text, self.new_dir.clone());
                    self.info("Starting the project…");
                }
            }
            _ => {
                self.new_input.key(key);
            }
        }
    }

    fn key_project(&mut self, key: KeyEvent) {
        let Where::Project(ui) = &mut self.at else {
            return;
        };
        if let Some(p) = &mut ui.prompt {
            match typing(p, key) {
                Typed::Pending => {}
                Typed::Cancelled => ui.prompt = None,
                Typed::Done(text) => {
                    let then = ui.prompt.take().map(|p| p.then);
                    if let Some(then) = then {
                        self.answered(then, text);
                    }
                }
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let chat = ui.tab == Tab::Chat;
        match key.code {
            KeyCode::Char('o') if ctrl => {
                self.at = Where::Projects;
                self.saved();
                return;
            }
            KeyCode::Tab => return self.step_tab(1),
            KeyCode::BackTab => return self.step_tab(TABS.len() - 1),
            KeyCode::Char(c @ ('1'..='9' | '0')) if !chat => {
                let i = if c == '0' {
                    9
                } else {
                    c as usize - '1' as usize
                };
                return self.set_tab(TABS[i]);
            }
            KeyCode::Char('q') if !chat => {
                self.quit = true;
                return;
            }
            KeyCode::Char('p') if !chat => {
                self.at = Where::Projects;
                self.saved();
                return;
            }
            _ => {}
        }
        match key.code {
            KeyCode::PageUp => return self.scroll(-(self.body.1 as isize)),
            KeyCode::PageDown => return self.scroll(self.body.1 as isize),
            _ => {}
        }
        match ui.tab {
            Tab::Chat => self.key_chat(key),
            tab => {
                if self.navigate(key) {
                    return;
                }
                match tab {
                    Tab::Held => self.key_held(key),
                    Tab::Team => self.key_team(key),
                    Tab::Specs => self.key_specs(key),
                    Tab::Tasks => {
                        if key.code == KeyCode::Char('n') {
                            self.ask("Title of the task", Then::RaiseTitle);
                        }
                    }
                    Tab::Files => self.key_files(key),
                    Tab::Budget => self.key_budget(key),
                    Tab::Settings => self.key_settings(key),
                    Tab::Chat | Tab::Log | Tab::Preview => {}
                }
            }
        }
    }

    fn step_tab(&mut self, by: usize) {
        let Some(ui) = self.project_ui() else { return };
        let i = TABS.iter().position(|t| *t == ui.tab).unwrap_or(0);
        self.set_tab(TABS[(i + by) % TABS.len()]);
    }

    pub fn ask(&mut self, title: &str, then: Then) {
        if let Some(ui) = self.project_ui_mut() {
            ui.prompt = Some(prompt(title, then));
        }
    }

    /// A prompt that starts with `text` in it, to change rather than retype.
    pub fn ask_with(&mut self, title: &str, then: Then, text: &str) {
        if let Some(ui) = self.project_ui_mut() {
            let mut p = prompt(title, then);
            p.input.insert_str(text);
            ui.prompt = Some(p);
        }
    }

    pub fn ask_secret(&mut self, title: &str, then: Then) {
        if let Some(ui) = self.project_ui_mut() {
            let mut p = prompt(title, then);
            p.masked = true;
            ui.prompt = Some(p);
        }
    }

    /// Moves the selection, or the view where a page has nothing to select.
    fn navigate(&mut self, key: KeyEvent) -> bool {
        let delta: isize = match key.code {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::Home => isize::MIN / 2,
            KeyCode::End => isize::MAX / 2,
            _ => return false,
        };
        let width = self.body.0 as usize;
        let Some(page) = self.page(width) else {
            return false;
        };
        let count = page.targets().len();
        let Some(ui) = self.project_ui_mut() else {
            return false;
        };
        if count == 0 {
            self.scroll(delta);
        } else {
            let at = ui.sel.entry(ui.tab).or_insert(0);
            *at = (*at as isize + delta).clamp(0, count as isize - 1) as usize;
        }
        true
    }

    /// Scrolls the page: down is towards newer lines in a tab that follows its end.
    pub fn scroll(&mut self, delta: isize) {
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        let end = follows_end(ui);
        let off = ui.off.entry(ui.tab).or_insert(0);
        let by = if end { -delta } else { delta };
        *off = (*off as isize + by).max(0) as usize;
        let (tab, id) = (ui.tab, ui.id);
        // Reading past the last line of the Log asks for older events.
        if tab == Tab::Log {
            ui.wants_older = true;
            self.sync_log_for(id);
        }
    }

    fn key_chat(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Where::Project(ui) = &mut self.at else {
            return;
        };
        match key.code {
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => self.send_chat(),
            KeyCode::Char('a') if ctrl => self.ask("File to attach (a path)", Then::AttachPath),
            KeyCode::Esc => {
                ui.input.clear();
                ui.attachments.clear();
            }
            KeyCode::Up if ui.input.is_empty() => self.scroll(-1),
            KeyCode::Down if ui.input.is_empty() => self.scroll(1),
            _ => {
                ui.input.key(key);
            }
        }
    }

    fn send_chat(&mut self) {
        let Where::Project(ui) = &mut self.at else {
            return;
        };
        let body = ui.input.text().trim().to_owned();
        if body.is_empty() {
            return;
        }
        ui.input.clear();
        ui.off.insert(Tab::Chat, 0);
        let files: Vec<_> = ui.attachments.drain(..).map(|f| f.id).collect();
        let project = ui.id;
        // A message answers the question the CEO is waiting on, the oldest first.
        let reply = self.client.project(project).and_then(|p| {
            let mut open: Vec<_> = p
                .open_questions()
                .filter(|m| m.message.route == Route::CeoToBuilder)
                .collect();
            open.sort_by_key(|m| m.at);
            open.first().map(|m| m.message.id)
        });
        let client = self.client.clone();
        self.spawn(async move {
            client.send_chat(project, body, files, reply).await?;
            Ok(String::new())
        });
    }

    fn key_held(&mut self, key: KeyEvent) {
        let decision = match key.code {
            KeyCode::Char('a') => Decision::Approved,
            KeyCode::Char('d') => Decision::Denied,
            _ => return,
        };
        let (Some(Target::Action(action)), Some(ui)) = (self.selected(), self.project_ui()) else {
            return;
        };
        let (project, client) = (ui.id, self.client.clone());
        let word = if decision == Decision::Approved {
            "Approved"
        } else {
            "Denied"
        };
        self.spawn(async move {
            client.decide_action(project, action, decision).await?;
            Ok(word.into())
        });
    }

    fn key_specs(&mut self, key: KeyEvent) {
        let kinds = crate::views::specs::KINDS;
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        let i = kinds.iter().position(|k| *k == ui.spec).unwrap_or(0);
        match key.code {
            KeyCode::Left => ui.spec = kinds[(i + 2) % 3],
            KeyCode::Right => ui.spec = kinds[(i + 1) % 3],
            _ => return,
        }
        ui.off.insert(Tab::Specs, 0);
    }

    fn key_files(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('u') => self.ask("File to add (a path)", Then::UploadPath),
            KeyCode::Char('x') => {
                if let Some(Target::File(id)) = self.selected() {
                    let name = self
                        .project_ui()
                        .and_then(|u| self.client.project(u.id))
                        .and_then(|p| p.file(id).map(|f| f.file.name.clone()))
                        .unwrap_or_default();
                    self.ask(&format!("Remove {name}? (y/n)"), Then::RemoveFile(id));
                }
            }
            _ => {}
        }
    }

    fn key_budget(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('b') => self.ask("Project budget in dollars", Then::ProjectBudget),
            KeyCode::Char('e') => {
                if let Some(Target::Lead(lead)) = self.selected() {
                    self.ask("Lead's budget in dollars", Then::LeadBudget(lead));
                }
            }
            _ => {}
        }
    }

    fn key_team(&mut self, key: KeyEvent) {
        let Some(ui) = self.project_ui() else { return };
        match (&ui.agent, key.code) {
            (None, KeyCode::Char('d')) => {
                let ui = self.project_ui_mut().expect("a project is open");
                ui.drivers = !ui.drivers;
                ui.sel.insert(Tab::Team, 0);
            }
            (None, KeyCode::Enter) if !ui.drivers => {
                if let Some(Target::Agent(id)) = self.selected() {
                    self.open_agent(id);
                }
            }
            (None, KeyCode::Char('s')) if ui.drivers => {
                if let Some(Target::Driver(driver)) = self.selected() {
                    let (project, client) = (ui.id, self.client.clone());
                    self.spawn(async move {
                        client.request(StopDriver { project, driver }).await?;
                        Ok("Stopped the driver; its work goes to another".into())
                    });
                }
            }
            (None, KeyCode::Char('r')) if ui.drivers => {
                if let Some(Target::Driver(driver)) = self.selected() {
                    let project = ui.id;
                    if let Some(agent) = self
                        .client
                        .project(project)
                        .and_then(|p| p.driver(driver).map(|d| d.driver.agent))
                    {
                        self.replace_driver(project, agent);
                    }
                }
            }
            (Some(a), KeyCode::Esc) => {
                let _ = a;
                self.close_agent();
            }
            (Some(a), KeyCode::Enter) if a.mode == AgentMode::Children => {
                if let Some(Target::Agent(id)) = self.selected() {
                    self.open_agent(id);
                }
            }
            (Some(a), KeyCode::Char('r')) => {
                let (project, agent) = (ui.id, a.id);
                self.replace_driver(project, agent);
            }
            (Some(a), KeyCode::Char('l')) => {
                let (project, agent) = (ui.id, a.id);
                self.ask_look(project, agent);
            }
            (Some(a), KeyCode::Char('n')) if a.mode == AgentMode::Skills => {
                let agent = a.id;
                self.ask_new_skill(agent);
            }
            (Some(a), KeyCode::Char('e')) if a.mode == AgentMode::Skills => {
                let agent = a.id;
                if let Some(Target::Skill(skill)) = self.selected() {
                    self.ask_edit_skill(agent, skill);
                }
            }
            (Some(_), KeyCode::Char(c)) => {
                let mode = match c {
                    'c' => AgentMode::Children,
                    'o' => AgentMode::Output,
                    'w' => AgentMode::Waits,
                    'm' => AgentMode::Memories,
                    'k' => AgentMode::Skills,
                    't' => AgentMode::Trials,
                    _ => return,
                };
                self.set_mode(mode);
            }
            _ => {}
        }
    }

    fn replace_driver(&mut self, project: x100_core::id::ProjectId, agent: AgentId) {
        let client = self.client.clone();
        self.spawn(async move {
            client.request(ReplaceDriver { project, agent }).await?;
            Ok("Replacing the driver; the work hands off".into())
        });
    }

    /// Opens an agent's page. Going deeper leaves the one opened before to come back to.
    pub fn open_agent(&mut self, id: AgentId) {
        let token = self.fresh_token();
        let (client, tx, handle) = (self.client.clone(), self.tx.clone(), self.handle.clone());
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        let project = ui.id;
        if let Some(prev) = ui.agent.as_ref().map(|a| a.id) {
            ui.back.push(prev);
        }
        let runs = client
            .project(project)
            .is_some_and(|p| p.children(id).is_empty());
        ui.agent = Some(AgentUi {
            id,
            chosen: false,
            mode: if runs {
                AgentMode::Output
            } else {
                AgentMode::Children
            },
            output: Output::open(client, project, id, token, tx, handle),
            memories: None,
            skills: None,
        });
        ui.sel.insert(Tab::Team, 0);
        ui.off.insert(Tab::Team, 0);
    }

    fn close_agent(&mut self) {
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        ui.agent = None;
        match ui.back.pop() {
            Some(prev) => {
                // Opening it again pushes nothing: it was just closed.
                self.open_agent(prev);
            }
            None => {
                ui.sel.insert(Tab::Team, 0);
            }
        }
    }

    fn set_mode(&mut self, mode: AgentMode) {
        let (client, tx, handle) = (self.client.clone(), self.tx.clone(), self.handle.clone());
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        let project = ui.id;
        let Some(a) = &mut ui.agent else { return };
        a.mode = mode;
        a.chosen = true;
        let agent = a.id;
        ui.sel.insert(Tab::Team, 0);
        ui.off.insert(Tab::Team, 0);
        match mode {
            AgentMode::Memories => {
                handle.spawn(async move {
                    let result = client.list_memories(project, agent).await;
                    let _ = tx.send(Msg::Memories { agent, result });
                });
            }
            AgentMode::Skills => {
                handle.spawn(async move {
                    let result = client.list_skills(project, None).await;
                    let _ = tx.send(Msg::Skills { agent, result });
                });
            }
            _ => {}
        }
    }
}
