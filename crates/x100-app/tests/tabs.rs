//! The tabs after the Fernleaf script, against a real daemon on the fake clock, without a display:
//! each shows what the client library holds, a file removed from Files is in the Log, a task raised
//! shows in Tasks, Ask me about reaches the daemon, a control whose method is not available yet
//! says so, and one task changing redraws its row alone.

mod common;

use common::*;
use gpui::{Entity, VisualTestContext, WindowHandle};
use x100_app::budget::{BudgetModel, BudgetTab};
use x100_app::files::{FileModel, FilesView};
use x100_app::infra::InfraView;
use x100_app::log::{self, LogView, Names};
use x100_app::notice::Outcome;
use x100_app::preview::PreviewView;
use x100_app::settings::{Control, SettingsModel, SettingsView};
use x100_app::shell::Shell;
use x100_app::specs::{SpecModel, SpecsView};
use x100_app::tabs::{MainPanel, Tab};
use x100_app::tasks::{Status, TaskModel, TasksView};
use x100_client::Client;
use x100_core::action::{AskMeAbout, Decision};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, ProjectId};
use x100_core::party::Party;
use x100_core::spec::SpecKind;

/// A one-pixel PNG, to hold as an image.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0xf8, 0xcf, 0xc0,
    0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

struct Open {
    run: Run,
    client: Client,
    project: ProjectId,
    window: WindowHandle<Shell>,
    vcx: VisualTestContext,
}

/// The Fernleaf script, run to the end with two files held, and the window open on it.
fn script() -> Open {
    let mut run = Run::start();
    let client = run.client();
    let project = run.fernleaf_project(&client);
    run.rt
        .block_on(client.upload_file(project, "sketch.png", "image/png", PNG.to_vec()))
        .expect("the picture is held");
    run.rt
        .block_on(client.upload_file(project, "notes.md", "text/markdown", b"# Notes\n".to_vec()))
        .expect("the note is held");
    run.rt
        .block_on(client.send_chat(
            project,
            "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
            vec![],
            None,
        ))
        .expect("the first message is sent");
    run.settle(&client, project);
    let (window, mut vcx) = run.open(client.clone());
    pump_until(&mut vcx, "the main panel opens", |vcx| {
        bounds(vcx, "tab-Team").is_some()
    });
    Open {
        run,
        client,
        project,
        window,
        vcx,
    }
}

impl Open {
    fn state(&self) -> ProjectState {
        self.client.project(self.project).expect("the project")
    }

    fn main(&self) -> Entity<MainPanel> {
        self.vcx.read(|cx| {
            self.window
                .read(cx)
                .unwrap()
                .project(self.project)
                .unwrap()
                .main
                .clone()
        })
    }

    /// Opens a tab and returns its view.
    fn open<V: 'static>(&mut self, tab: Tab) -> Entity<V> {
        let selector = leak(format!("tab-{}", tab.name()));
        click(&mut self.vcx, selector);
        let main = self.main();
        self.vcx
            .read(|cx| main.read(cx).tab_view::<V>(tab))
            .expect("the tab's view")
    }

    fn click(&mut self, selector: String) {
        click(&mut self.vcx, leak(selector));
    }

    fn pump_until(&mut self, what: &str, done: impl FnMut(&mut VisualTestContext) -> bool) {
        pump_until(&mut self.vcx, what, done);
    }

    /// Pumps until the client's state for the project satisfies `done`.
    fn until_state(&mut self, what: &str, done: impl Fn(&ProjectState) -> bool) {
        let (client, project) = (self.client.clone(), self.project);
        pump_until(&mut self.vcx, what, move |_| {
            done(&client.project(project).expect("the project"))
        });
    }
}

/// Scrolls the page under the pointer down, as the wheel would, to reach what is below the fold.
fn scroll_down(vcx: &mut VisualTestContext) {
    vcx.simulate_event(gpui::ScrollEvent {
        position: gpui::point(gpui::px(900.), gpui::px(500.)),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-1000.))),
        ..Default::default()
    });
    pump(vcx);
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[test]
fn every_tab_shows_what_the_client_holds_after_the_script() {
    let mut o = script();
    let state = o.state();

    // Specs: each of the three, with its author, revision and status as the state has it.
    let specs = o.open::<SpecsView>(Tab::Specs);
    let mut written = 0;
    for (kind, selector) in [
        (SpecKind::Brd, "spec-BRD"),
        (SpecKind::Prd, "spec-PRD"),
        (SpecKind::Add, "spec-ADD"),
    ] {
        o.click(selector.into());
        let want = SpecModel::read(&state, kind);
        written += usize::from(want.is_some());
        o.pump_until("the spec is shown", |vcx| {
            vcx.read(|cx| specs.read(cx).shown().cloned()) == want
        });
        if let Some(want) = want {
            assert!(want.revision >= 1 && !want.body.is_empty());
            assert!(
                want.author == "CEO" || want.author == "Product" || want.author == "Tech",
                "{}",
                want.author
            );
        }
    }
    assert_eq!(written, state.specs().count());
    assert!(written >= 2, "the script writes at least the BRD and PRD");

    // Tasks: every task, with its status.
    let tasks = o.open::<TasksView>(Tab::Tasks);
    let want = TaskModel::all(&state);
    assert_eq!(want.len(), state.tasks().count());
    assert!(want.len() >= 5, "{want:?}");
    o.pump_until("the rows show the tasks", |vcx| {
        let rows: Vec<_> = vcx.read(|cx| {
            tasks
                .read(cx)
                .rows()
                .iter()
                .map(|r| r.read(cx).shown().cloned())
                .collect()
        });
        rows == want.iter().cloned().map(Some).collect::<Vec<_>>()
    });
    assert!(
        want.iter().any(|t| t.status == Status::InProgress)
            && want.iter().any(|t| t.status == Status::Done),
        "the script leaves tasks in more than one status: {want:?}"
    );

    // Files: the two held files, who added them, their kind and size.
    let files = o.open::<FilesView>(Tab::Files);
    let want = FileModel::all(&state);
    assert_eq!(want.len(), 2);
    o.pump_until("the files are shown", |vcx| {
        vcx.read(|cx| files.read(cx).shown().to_vec()) == want
    });
    let sketch = want.iter().find(|f| f.name == "sketch.png").unwrap();
    assert_eq!(
        (sketch.kind.as_str(), sketch.by.as_str(), sketch.size),
        ("Image", "You", PNG.len() as u64)
    );

    // Budget: the project line, each lead's, each agent's spend, the lab.
    let budget = o.open::<BudgetTab>(Tab::Budget);
    let want = BudgetModel::read(&state).expect("a budget");
    assert_eq!(want.view.leads.len(), 3);
    assert!(
        want.view.project.spent.micros() > 0,
        "the script spends money"
    );
    assert_eq!(
        want.view.project.budget,
        x100_core::money::Money::from_dollars(50)
    );
    o.pump_until("the budget is shown", |vcx| {
        vcx.read(|cx| budget.read(cx).shown().cloned()) == Some(want.clone())
    });

    // Preview and Infra: no address, so a plain line; the map is not in v1.
    let preview = o.open::<PreviewView>(Tab::Preview);
    assert_eq!(
        vcx_read(&o.vcx, |cx| preview.read(cx).address().map(str::to_owned)),
        state.project().unwrap().preview
    );
    assert!(bounds(&mut o.vcx, "preview-link").is_none());
    let _ = o.open::<InfraView>(Tab::Infra);

    // Settings: what the project's settings, team and harnesses say.
    let settings = o.open::<SettingsView>(Tab::Settings);
    let daemon = o.client.state().daemon;
    let want = SettingsModel::read(&state, &daemon).expect("settings");
    assert_eq!(want.leads.len(), 3);
    o.pump_until("the settings are shown", |vcx| {
        vcx.read(|cx| settings.read(cx).shown().cloned()) == Some(want.clone())
    });
}

fn vcx_read<R>(vcx: &VisualTestContext, f: impl FnOnce(&gpui::App) -> R) -> R {
    vcx.read(f)
}

#[test]
fn the_log_lists_the_newest_events_first_and_lays_out_only_what_is_in_view() {
    let mut o = script();
    let log = o.open::<LogView>(Tab::Log);
    let seq = o.state().seq().expect("events");
    o.pump_until("the log has caught up", |vcx| {
        vcx.read(|cx| log.read(cx).caught_up(cx))
    });
    let (count, newest, drawn) = vcx_read(&o.vcx, |cx| {
        let l = log.read(cx);
        (
            l.lines().len(),
            l.lines().iter().take(30).cloned().collect::<Vec<_>>(),
            l.drawn(),
        )
    });
    assert!(count > 100, "the script leaves a long log: {count}");
    assert!(
        drawn > 0 && drawn < 60 && drawn < count,
        "{drawn} of {count} rows were laid out"
    );
    // Newest first, each line the wording of the event the daemon holds there.
    assert_eq!(newest[0].seq, seq);
    let events = o
        .run
        .rt
        .block_on(o.client.read_events_before(o.project, None, 30))
        .expect("a page");
    let state = o.state();
    let daemon = o.client.state().daemon;
    let files = Default::default();
    let names = Names {
        project: &state,
        daemon: &daemon,
        files: &files,
    };
    let want: Vec<_> = events
        .iter()
        .map(|e| {
            let l = log::line(e, &names);
            (e.seq, l.who, l.text)
        })
        .collect();
    let shown: Vec<_> = newest
        .iter()
        .map(|l| (l.seq, l.who.clone(), l.text.clone()))
        .collect();
    assert_eq!(shown, want);
    assert!(newest.windows(2).all(|w| w[0].seq > w[1].seq));
}

#[test]
fn removing_a_file_from_the_files_tab_reaches_the_daemon_and_shows_in_the_log() {
    let mut o = script();
    let files = o.open::<FilesView>(Tab::Files);
    o.pump_until("both files are shown", |vcx| {
        vcx.read(|cx| files.read(cx).shown().len()) == 2
    });
    // The cross asks first; Keep leaves the file alone.
    o.click("file-x-sketch.png".into());
    o.click("file-remove-sketch.png".into());
    o.until_state("the daemon has removed it", |p| {
        !p.files().any(|f| f.file.name == "sketch.png")
    });
    o.pump_until("the tab no longer shows it", |vcx| {
        vcx.read(|cx| {
            files
                .read(cx)
                .shown()
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
        }) == ["notes.md"]
    });
    // The daemon's own log has the removal as its newest event.
    let newest = o
        .run
        .rt
        .block_on(o.client.read_events_before(o.project, None, 1))
        .unwrap();
    assert!(
        matches!(newest[0].event, ProjectEvent::FileRemoved { .. }),
        "{:?}",
        newest[0].event
    );
    // And the Log tab says so, naming the file although the state has forgotten it.
    let log = o.open::<LogView>(Tab::Log);
    o.pump_until("the log shows the removal", |vcx| {
        vcx.read(|cx| {
            log.read(cx)
                .lines()
                .front()
                .map(|l| (l.who.clone(), l.text.clone()))
        }) == Some(("You".into(), "Removed sketch.png to save space".into()))
    });
}

#[test]
fn keeping_a_file_leaves_it() {
    let mut o = script();
    let files = o.open::<FilesView>(Tab::Files);
    o.pump_until("both files are shown", |vcx| {
        vcx.read(|cx| files.read(cx).shown().len()) == 2
    });
    o.click("file-x-notes.md".into());
    assert!(
        bounds(&mut o.vcx, "file-remove-notes.md").is_some(),
        "asked to confirm"
    );
    // Nothing is removed by the cross alone.
    pump(&mut o.vcx);
    assert_eq!(o.state().files().count(), 2);
}

#[test]
fn a_task_raised_in_the_tasks_tab_is_in_the_list() {
    let mut o = script();
    let tasks = o.open::<TasksView>(Tab::Tasks);
    let before = o.state().tasks().count();
    o.click("raise-task".into());
    o.click("raise-title".into());
    o.vcx.simulate_input("Add a pricing FAQ");
    o.click("raise-ask".into());
    o.vcx
        .simulate_input("Answer the questions people ask before they subscribe.");
    o.click("raise-part".into());
    o.vcx.simulate_input("Website");
    o.click("raise-send".into());
    o.until_state("the daemon holds the task", |p| {
        p.tasks().count() == before + 1
    });
    let p = o.state();
    let raised = p
        .tasks()
        .find(|t| t.task.title == "Add a pricing FAQ")
        .expect("the task");
    assert_eq!(raised.task.raised_by, Party::Builder);
    assert_eq!(raised.task.part.as_deref(), Some("Website"));
    assert_eq!(Status::of(&raised.state), Status::NewIdea);
    let want = TaskModel::all(&p);
    o.pump_until("the Tasks tab lists it", |vcx| {
        let rows: Vec<_> = vcx.read(|cx| {
            tasks
                .read(cx)
                .rows()
                .iter()
                .map(|r| r.read(cx).shown().cloned())
                .collect()
        });
        rows == want.iter().cloned().map(Some).collect::<Vec<_>>()
    });
    let mine = want
        .iter()
        .find(|t| t.title == "Add a pricing FAQ")
        .unwrap();
    assert_eq!(
        (mine.status, mine.raised_by.as_str(), mine.part.as_str()),
        (Status::NewIdea, "You", "Website")
    );
    // The form closed once the daemon had the task.
    assert!(bounds(&mut o.vcx, "raise-send").is_none());
}

#[test]
fn a_task_without_a_title_is_not_sent() {
    let mut o = script();
    let tasks = o.open::<TasksView>(Tab::Tasks);
    let before = o.state().tasks().count();
    o.click("raise-task".into());
    o.click("raise-send".into());
    pump(&mut o.vcx);
    assert!(matches!(
        vcx_read(&o.vcx, |cx| tasks.read(cx).outcome().cloned()),
        Some(Outcome::Failed(_))
    ));
    assert_eq!(o.state().tasks().count(), before);
}

#[test]
fn changing_ask_me_about_reaches_the_daemon_and_the_log() {
    let mut o = script();
    let settings = o.open::<SettingsView>(Tab::Settings);
    assert_eq!(
        o.state().project().unwrap().settings.ask_me_about,
        AskMeAbout::HighRiskOnly
    );
    // Every decision is the fourth segment.
    o.click("ask-3".into());
    o.until_state("the daemon holds the new setting", |p| {
        p.project().unwrap().settings.ask_me_about == AskMeAbout::EveryDecision
    });
    o.pump_until("the tab shows it", |vcx| {
        vcx.read(|cx| settings.read(cx).shown().map(|m| m.ask)) == Some(AskMeAbout::EveryDecision)
    });
    assert_eq!(
        vcx_read(&o.vcx, |cx| settings
            .read(cx)
            .outcome(Control::Ask)
            .cloned()),
        None
    );
    let log = o.open::<LogView>(Tab::Log);
    o.pump_until("the log says so", |vcx| {
        vcx.read(|cx| log.read(cx).lines().front().map(|l| l.text.clone()))
            == Some("Changed Ask me about to Every decision".into())
    });
}

#[test]
fn turning_a_model_on_reaches_the_daemon() {
    let mut o = script();
    let settings = o.open::<SettingsView>(Tab::Settings);
    let daemon = o.client.state().daemon;
    let model = SettingsModel::read(&o.state(), &daemon).unwrap();
    let off = model
        .models
        .iter()
        .position(|m| !m.allowed)
        .expect("the script's harness offers a model that is not allowed");
    o.click(format!("model-{off}"));
    let what = model.models[off].what.clone();
    o.until_state("the daemon allows it", |p| {
        p.project().unwrap().settings.allowed.contains(&what)
    });
    o.pump_until("the tab shows it on", |vcx| {
        vcx.read(|cx| settings.read(cx).shown().map(|m| m.models[off].allowed)) == Some(true)
    });
}

/// What a control that goes to a method the daemon may not have made yet ends up as: either it
/// says so beside itself and the state is as it was, or it worked and the state changed.
fn says_so_or_worked(outcome: Option<Outcome>, unchanged: bool) {
    match outcome {
        Some(Outcome::Unavailable(text)) => {
            assert!(unchanged, "it said {text:?} and still changed the state");
            assert!(
                text.starts_with("Not available yet: it arrives with M"),
                "{text}"
            );
        }
        Some(Outcome::Failed(why)) => panic!("refused: {why}"),
        Some(Outcome::Working) => panic!("still working"),
        None => assert!(!unchanged, "it claimed to have worked and nothing changed"),
    }
}

#[test]
fn controls_whose_method_is_not_available_yet_say_so() {
    let mut o = script();
    let settings = o.open::<SettingsView>(Tab::Settings);
    let p = o.state();
    let tech = p.agents().find(|a| a.role == "Tech").unwrap().id;
    let client = o.client.clone();
    let tech_budget = move |p: &ProjectState| {
        SettingsModel::read(p, &client.state().daemon)
            .unwrap()
            .leads
            .iter()
            .find(|l| l.id == tech)
            .map(|l| (l.budget, l.effort))
            .unwrap()
    };
    let before = tech_budget(&p);
    let (client2, project) = (o.client.clone(), o.project);

    // A lead's budget: M8 makes it.
    o.click("lead-budget-Tech-field".into());
    o.vcx.simulate_input("1.50");
    o.click("lead-budget-Tech-set".into());
    o.pump_until("the budget control has an answer", |vcx| {
        let outcome = vcx.read(|cx| {
            settings
                .read(cx)
                .outcome(Control::LeadBudget(tech))
                .cloned()
        });
        outcome != Some(Outcome::Working)
            && (outcome.is_some() || tech_budget(&client2.project(project).unwrap()).0 != before.0)
    });
    let outcome = vcx_read(&o.vcx, |cx| {
        settings
            .read(cx)
            .outcome(Control::LeadBudget(tech))
            .cloned()
    });
    let now = tech_budget(&o.state());
    says_so_or_worked(outcome, now.0 == before.0);

    // A lead's effort range: M8 makes it too. Raise the bottom to High.
    o.click("effort-lo-Tech-2".into());
    o.pump_until("the effort control has an answer", |vcx| {
        let outcome = vcx.read(|cx| settings.read(cx).outcome(Control::Effort(tech)).cloned());
        outcome != Some(Outcome::Working)
            && (outcome.is_some() || tech_budget(&client2.project(project).unwrap()).1 != before.1)
    });
    let outcome = vcx_read(&o.vcx, |cx| {
        settings.read(cx).outcome(Control::Effort(tech)).cloned()
    });
    says_so_or_worked(outcome, tech_budget(&o.state()).1 == before.1);

    // A bad amount is refused by the app without asking the daemon.
    o.click("lead-budget-Tech-field".into());
    o.vcx.simulate_input("lots");
    o.click("lead-budget-Tech-set".into());
    pump(&mut o.vcx);
    assert!(matches!(
        vcx_read(&o.vcx, |cx| settings
            .read(cx)
            .outcome(Control::LeadBudget(tech))
            .cloned()),
        Some(Outcome::Failed(_))
    ));
}

#[test]
fn credentials_are_added_with_a_value_and_a_scope() {
    let mut o = script();
    let settings = o.open::<SettingsView>(Tab::Settings);
    let creds = vcx_read(&o.vcx, |cx| settings.read(cx).credentials().clone());
    let before = vcx_read(&o.vcx, |cx| creds.read(cx).shown().len());
    // Credentials are at the foot of the tab, under the project's name, intent and directory.
    scroll_down(&mut o.vcx);
    o.click("cred-add-open".into());
    o.vcx.simulate_input("Stripe test key");
    scroll_down(&mut o.vcx);
    o.click("cred-value".into());
    o.vcx.simulate_input("sk_test_not_a_real_key");
    scroll_down(&mut o.vcx);
    o.click("cred-add-send".into());
    o.pump_until("the add has an answer", |vcx| {
        let outcome = vcx.read(|cx| creds.read(cx).outcome().cloned());
        outcome != Some(Outcome::Working)
            && (outcome.is_some() || vcx.read(|cx| creds.read(cx).shown().len()) > before)
    });
    let outcome = vcx_read(&o.vcx, |cx| creds.read(cx).outcome().cloned());
    let after = vcx_read(&o.vcx, |cx| creds.read(cx).shown().len());
    assert_eq!(outcome, None, "the daemon took it");
    assert_eq!(after, before + 1);
    let added = vcx_read(&o.vcx, |cx| creds.read(cx).shown().last().cloned()).unwrap();
    assert_eq!(added.name, "Stripe test key");
    // The value is not on screen or in the state the app holds.
    let held: Vec<_> = vcx_read(&o.vcx, |cx| creds.read(cx).shown().to_vec());
    assert!(!format!("{held:?}").contains("sk_test_not_a_real_key"));
}

/// The daemon's one unsafe setting is off and the sandbox works here, and it turns on and off
/// again from Settings.
#[test]
fn running_without_a_sandbox_is_shown_and_set_from_settings() {
    let mut o = script();
    let settings = o.open::<SettingsView>(Tab::Settings);
    let part = vcx_read(&o.vcx, |cx| settings.read(cx).sandbox().clone());
    let shown = vcx_read(&o.vcx, |cx| part.read(cx).shown().clone());
    assert!(!shown.uncontained);
    assert_eq!(shown.missing, None, "a sandbox can be made on this machine");
    for on in [true, false] {
        scroll_down(&mut o.vcx);
        o.click("run-without-sandbox".into());
        o.pump_until("the setting changes", |vcx| {
            vcx.read(|cx| part.read(cx).shown().uncontained == on)
        });
        assert_eq!(
            vcx_read(&o.vcx, |cx| part.read(cx).outcome().cloned()),
            None
        );
    }
}

#[test]
fn approve_and_deny_say_so_while_the_daemon_has_not_made_them() {
    let mut o = script();
    let held = o.vcx.read(|cx| {
        o.window
            .read(cx)
            .unwrap()
            .project(o.project)
            .unwrap()
            .chat
            .read(cx)
            .held()
            .clone()
    });
    // The script raises nothing that waits for the builder, so there is nothing in the strip.
    assert!(vcx_read(&o.vcx, |cx| held.read(cx).shown().is_empty()));
    // The control's request still goes to the daemon, which answers for an action it does not hold.
    let action = ActionId::from_u128(1);
    held.update(&mut o.vcx, |h, cx| h.decide(action, Decision::Approved, cx));
    o.pump_until("the answer is back", |vcx| {
        vcx.read(|cx| held.read(cx).outcome(action).is_some())
            && vcx.read(|cx| held.read(cx).outcome(action).cloned()) != Some(Outcome::Working)
    });
    let outcome = vcx_read(&o.vcx, |cx| held.read(cx).outcome(action).cloned()).unwrap();
    match outcome {
        Outcome::Unavailable(text) => {
            assert!(
                text.starts_with("Not available yet: it arrives with M"),
                "{text}"
            )
        }
        // Once the daemon makes it, an action it does not hold is refused as such.
        Outcome::Failed(why) => assert!(
            why.to_lowercase().contains("not found") || why.contains("held"),
            "{why}"
        ),
        Outcome::Working => unreachable!(),
    }
}

#[test]
fn one_task_changing_redraws_its_row_alone() {
    let mut o = script();
    let tasks = o.open::<TasksView>(Tab::Tasks);
    let rows = |vcx: &VisualTestContext| -> Vec<(TaskModel, usize)> {
        vcx.read(|cx| {
            tasks
                .read(cx)
                .rows()
                .iter()
                .map(|r| {
                    let r = r.read(cx);
                    (r.shown().cloned().expect("drawn"), r.renders())
                })
                .collect()
        })
    };
    let want = TaskModel::all(&o.state());
    o.pump_until("the rows show the tasks", |vcx| {
        rows(vcx).iter().map(|(m, _)| m.clone()).collect::<Vec<_>>() == want
    });
    let before = rows(&o.vcx);

    // The clock moves past the hour the script's recheck job is due and the morning its daily job
    // first fires, from another client, so no input to this window falls inside the count.
    let at = START + std::time::Duration::from_secs(3 * 3600);
    o.run
        .rt
        .block_on(o.client.request(x100_proto::SetClock { at }))
        .expect("the clock moves");
    o.run.settle(&o.client, o.project);
    let want = TaskModel::all(&o.state());
    o.pump_until("the rows follow the state", |vcx| {
        rows(vcx).iter().map(|(m, _)| m.clone()).collect::<Vec<_>>() == want
    });
    let after = rows(&o.vcx);
    let mut changed = 0;
    let mut alone = 0;
    for (id, b) in before.iter().map(|(m, n)| (m.id, (m, n))) {
        let (m, n) = after
            .iter()
            .find(|(m, _)| m.id == id)
            .map(|(m, n)| (m, n))
            .expect("a task is not removed");
        if m != b.0 {
            changed += 1;
            assert!(n > b.1, "{} changed and was not redrawn", m.title);
        } else {
            alone += 1;
            assert_eq!(n, b.1, "{} did not change and was redrawn", m.title);
        }
    }
    assert!(
        changed >= 1,
        "the clock moving changes at least one task: {before:?} {after:?}"
    );
    assert!(alone >= 1, "at least one task is left alone: {before:?}");
}

#[test]
fn a_log_of_twenty_thousand_events_is_read_by_the_page_and_laid_out_by_the_screenful() {
    const EXTRA: usize = 20_000;
    let mut o = script();
    let before = o.state().seq().unwrap().0;
    // Settings changes are events, and the cheapest way to make a long log.
    let (client, project) = (o.client.clone(), o.project);
    o.run.rt.block_on(async move {
        let levels = [
            AskMeAbout::Nothing,
            AskMeAbout::HighRiskOnly,
            AskMeAbout::MediumAndUp,
            AskMeAbout::EveryDecision,
        ];
        let mut sent = 0;
        while sent < EXTRA {
            let mut batch = tokio::task::JoinSet::new();
            for i in sent..(sent + 250).min(EXTRA) {
                let client = client.clone();
                batch.spawn(async move {
                    client
                        .change_setting(
                            project,
                            x100_core::project::SettingChange::AskMeAbout(levels[i % 4]),
                        )
                        .await
                });
            }
            while let Some(done) = batch.join_next().await {
                done.expect("the task ran").expect("the setting is taken");
            }
            sent += 250;
        }
    });
    o.run.settle(&o.client, o.project);
    let seq = o.state().seq().unwrap().0;
    assert!(
        seq - before >= 19_000,
        "{} events were written",
        seq - before
    );

    // Opening the tab reads one page, not the log.
    let log = o.open::<LogView>(Tab::Log);
    o.pump_until("the first page is in", |vcx| {
        vcx.read(|cx| log.read(cx).caught_up(cx))
    });
    let (loaded, drawn) = vcx_read(&o.vcx, |cx| {
        (log.read(cx).lines().len(), log.read(cx).drawn())
    });
    assert!(
        (1..=700).contains(&loaded),
        "{loaded} lines were made for {seq} events"
    );
    assert!(drawn > 0 && drawn < 60, "{drawn} rows were laid out");

    // Scrolling towards the end reads older pages as they come near, and never lays out more.
    for _ in 0..30 {
        scroll_down(&mut o.vcx);
    }
    o.pump_until("older pages have been read", |vcx| {
        vcx.read(|cx| log.read(cx).lines().len()) > loaded
    });
    let (lines, drawn): (Vec<_>, _) = vcx_read(&o.vcx, |cx| {
        let l = log.read(cx);
        (l.lines().iter().map(|l| l.seq.0).collect(), l.drawn())
    });
    assert!(drawn < 60, "{drawn} rows were laid out");
    assert!(
        lines.len() < 5_000,
        "{} lines for a few screenfuls",
        lines.len()
    );
    assert!(
        lines.windows(2).all(|w| w[0] == w[1] + 1),
        "the loaded lines are one run of events, newest first"
    );
    assert_eq!(lines[0], seq);
}
