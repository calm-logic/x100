//! Reflection and the lab in the app, against a real daemon on the fake clock with a script of
//! this test's own:
//!
//! - "Trial before commit" starts off in Settings, and clicking it turns the project's setting on
//!   as the daemon records it, and the control says so.
//! - Reflect now, with the CEO between tasks, opens the reflection task and the CEO gets it; with
//!   the CEO still on the builder's task, the control shows the daemon's refusal.
//! - After a reflection with the setting on, the CEO's page lists both trials from the daemon: the
//!   helpful skill committed, the unhelpful one dropped with its reason, and each arm's scores.
//!
//! The script is the daemon's own lab test's: the CEO sets the project up and does the brief, and
//! on reflection proposes two skills of its own, each tried on the brief with a check. A lab
//! driver whose pack holds the helpful skill writes what the check looks for; any other does not.

mod common;

use std::time::Duration;

use common::*;
use gpui::{Entity, VisualTestContext, WindowHandle};
use x100_app::notice::Outcome;
use x100_app::reflection::{Reflection, TrialCard};
use x100_app::settings::{Reflected, ReflectionPart, SettingsView};
use x100_app::shell::Shell;
use x100_app::tabs::{MainPanel, Tab};
use x100_client::Client;
use x100_core::driver::{DriverState, Purpose};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, ProjectId, TaskId};
use x100_core::settings::DaemonSetting;
use x100_core::task::TaskState;
use x100_core::trial::TrialState;
use x100_proto::{CreateProject, SetDaemonSetting};

/// Trials run several lab drivers each.
const LONG: Duration = Duration::from_secs(180);

/// A skill of the CEO's own as reflection proposes it, tried on the accepted brief with a check.
fn new_skill(name: &str, line: &str) -> String {
    let body = format!("---\\nname: {name}\\ndescription: How to {name}.\\n---\\n\\n{line}\\n");
    format!(
        r#"  {{ call = {{ propose_change = {{ trial = {{ change = {{ new_skill = {{ skill = {{ scope = {{ agent = "{{{{pack:Your id}}}}" }}, name = "{name}" }}, body = "{body}" }} }}, eval = {{ items = [ {{ past_task = {{ task = "{{{{input:Accepted}}}}", checkpoint = "" }} }}, {{ check = {{ script = {{ command = "grep -q hello greeting.txt" }} }} }} ], runs_per_arm = 3 }} }} }} }}, save = "{name}" }},"#
    )
}

/// Lab turns first, as a lab driver's pack also holds what the CEO's would. "BRIEF" has the CEO
/// set the project up and do its brief; "HOLD" has it set the project up on a brief it waits on,
/// so it stays on the builder's task.
fn script() -> String {
    let reflection = [
        new_skill("greeting", "GREETING-HELPS: write hello into greeting.txt."),
        new_skill("noise", "Say something kind first."),
    ]
    .join("\n");
    format!(
        r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = {{ input = 1.0, output = 5.0 }}

[[turn]]
when = {{ start_pack_contains = "GREETING-HELPS", input_contains = "Write the greeting" }}
repeat = true
steps = [
  {{ write = "greeting.txt", content = "hello" }},
  {{ usage = {{ input = 10, output = 5, cost_micros = 2000 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "wrote hello" }} }} }},
]

[[turn]]
when = {{ start_pack_contains = "LAB RUN", input_contains = "Write the greeting" }}
repeat = true
steps = [
  {{ write = "greeting.txt", content = "hi" }},
  {{ usage = {{ input = 10, output = 5, cost_micros = 2000 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "wrote hi" }} }} }},
]

[[turn]]
when = {{ input_contains = "BRIEF" }}
repeat = true
steps = [
  {{ call = {{ set_up_project = {{ name = "Lab", intent = "Try the lab.", brief = "Write the greeting." }} }} }},
]

[[turn]]
when = {{ input_contains = "HOLD" }}
repeat = true
steps = [
  {{ call = {{ set_up_project = {{ name = "Held", intent = "Hold on.", brief = "Hold the line." }} }} }},
]

[[turn]]
when = {{ input_contains = "A task for you: Write the greeting" }}
repeat = true
steps = [
  {{ write = "greeting.txt", content = "hello" }},
  {{ usage = {{ input = 100, output = 20, cost_micros = 1234 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "Wrote the greeting." }} }} }},
]

# A job far beyond the fake clock, which never moves here: the CEO waits on its task for good.
[[turn]]
when = {{ input_contains = "A task for you: Hold the line" }}
repeat = true
steps = [
  {{ call = {{ schedule = {{ when = {{ once = {{ at = 1894387600000 }} }}, does = {{ wake = {{ agent = "{{{{pack:Your id}}}}", note = "Look again." }} }} }} }}, save = "later" }},
  {{ call = {{ wait = {{ on = {{ job = "{{{{later.job}}}}" }}, task = "{{{{input:Task}}}}", progress = {{ note = "Holding.", done = 10 }} }} }} }},
]

[[turn]]
when = {{ input_contains = "Reflect on the last piece of work" }}
repeat = true
steps = [
{reflection}
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "Reflected." }} }} }},
]

# Anything else, such as a trial's verdict reaching the CEO, is noted and costs nothing.
[[turn]]
repeat = true
steps = [{{ text = "noted" }}]
"##
    )
}

struct Open {
    run: Run,
    client: Client,
    project: ProjectId,
    ceo: AgentId,
    window: WindowHandle<Shell>,
    vcx: VisualTestContext,
}

impl Open {
    /// A project on a folder of its own whose CEO was sent `first`, once the project is quiet, and
    /// the window open on it.
    fn start(first: &str, done: impl Fn(&ProjectState) -> bool) -> Open {
        let mut run = Run::scripted(&script());
        let client = run.client();
        let dir = run.daemon.dir.path().join("lab");
        std::fs::create_dir_all(&dir).expect("the project's folder");
        let project = run.rt.block_on(async {
            // A turn that makes no progress gives its task up at once, as in the daemon's test.
            client
                .request(SetDaemonSetting {
                    setting: DaemonSetting::TurnsWithoutProgress(1),
                })
                .await
                .expect("the setting is taken");
            let project = client
                .request(CreateProject {
                    dir: Some(dir.to_string_lossy().into_owned()),
                    settings: None,
                })
                .await
                .expect("the project is created")
                .project;
            client
                .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|_| ()))
                .await
                .expect("the client holds it");
            client
                .send_chat(project, first, vec![], None)
                .await
                .expect("the first message is sent");
            project
        });
        let ceo = client.project(project).unwrap().project().unwrap().ceo;
        run.rt
            .block_on(client.wait_for(WAIT, |s| {
                let p = s.projects.get(&project)?;
                done(p).then_some(())
            }))
            .expect("the CEO has done what it was sent");
        idle(&run, &client, project, ceo);
        let (window, mut vcx) = run.open(client.clone());
        pump_until(&mut vcx, "the main panel opens", |vcx| {
            bounds(vcx, "tab-Settings").is_some()
        });
        Open {
            run,
            client,
            project,
            ceo,
            window,
            vcx,
        }
    }

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

    /// Opens Settings and returns its Reflection part.
    fn reflection_part(&mut self) -> Entity<ReflectionPart> {
        click(&mut self.vcx, "tab-Settings");
        let main = self.main();
        let settings: Entity<SettingsView> = self
            .vcx
            .read(|cx| main.read(cx).tab_view(Tab::Settings))
            .expect("the Settings tab");
        self.vcx.read(|cx| settings.read(cx).reflection().clone())
    }

    /// Clicks Reflect now and waits for what came back.
    fn reflect_now(&mut self, part: &Entity<ReflectionPart>) -> Reflected {
        click(&mut self.vcx, "reflect-now");
        let mut said = None;
        pump_until(&mut self.vcx, "Reflect now says what came back", |vcx| {
            said = vcx.read(|cx| part.read(cx).reflected().cloned());
            !matches!(said, None | Some(Reflected::Said(Outcome::Working)))
        });
        said.unwrap()
    }
}

/// Waits until the CEO has nothing waiting and no turn or lab driver is running.
fn idle(run: &Run, client: &Client, project: ProjectId, ceo: AgentId) {
    run.rt
        .block_on(client.wait_for(LONG, |s| {
            let p = s.projects.get(&project)?;
            let busy = p.drivers().any(|d| {
                d.state.is_live()
                    && (d.state == DriverState::Running || d.driver.purpose != Purpose::Work)
            });
            (p.agent(ceo)?.inbox.is_empty() && !busy).then_some(())
        }))
        .expect("the project goes quiet");
}

fn brief_done(p: &ProjectState) -> bool {
    p.tasks().any(|t| matches!(t.state, TaskState::Done(..)))
}

#[test]
fn trial_before_commit_and_reflect_now_reach_the_daemon_and_the_trials_show_on_the_ceos_page() {
    let mut o = Open::start("BRIEF", brief_done);
    let part = o.reflection_part();

    // Off by default, in the daemon and in the tab.
    assert!(!o.state().project().unwrap().settings.trial_before_commit);
    pump_until(&mut o.vcx, "the setting is shown", |vcx| {
        vcx.read(|cx| part.read(cx).trial_before_commit()) == Some(false)
    });
    assert!(
        o.vcx
            .read(|cx| part.read(cx).meaning())
            .starts_with("Off: reflection's changes are committed directly")
    );

    // On, as the daemon records it, and the control follows with nothing said beside it.
    click(&mut o.vcx, "trial-before-commit");
    let (client, project) = (o.client.clone(), o.project);
    pump_until(&mut o.vcx, "the daemon holds the setting on", move |_| {
        let p = client.project(project).unwrap();
        p.project().unwrap().settings.trial_before_commit
    });
    pump_until(&mut o.vcx, "the control says so", |vcx| {
        vcx.read(|cx| part.read(cx).trial_before_commit()) == Some(true)
    });
    o.vcx.read(|cx| {
        let p = part.read(cx);
        assert_eq!(p.outcome(), None);
        assert!(
            p.meaning().starts_with("On: each skill change"),
            "{}",
            p.meaning()
        );
    });

    // Reflect now: the CEO is between tasks, so the daemon opens the reflection task for it.
    let Reflected::Opened(task) = o.reflect_now(&part) else {
        panic!("the task was not opened");
    };
    let (client, project, ceo) = (o.client.clone(), o.project, o.ceo);
    let opened = move |s: &ProjectState, task: TaskId| {
        let t = s.task(task)?;
        (t.task.title == "Reflect on the last piece of work"
            && t.state.assignment()?.assignee == ceo)
            .then_some(())
    };
    pump_until(&mut o.vcx, "the CEO has the reflection task", move |_| {
        opened(&client.project(project).unwrap(), task).is_some()
    });
    assert_eq!(
        o.vcx.read(|cx| part.read(cx).opened_line(task, cx)),
        "Opened \u{201c}Reflect on the last piece of work\u{201d} for the CEO."
    );

    // The CEO reflects, and the lab tries both skills.
    let (client, project) = (o.client.clone(), o.project);
    o.run
        .rt
        .block_on(client.wait_for(LONG, |s| {
            let p = s.projects.get(&project)?;
            let done = matches!(p.task(task)?.state, TaskState::Done(..));
            let ended = p.trials().count() == 2
                && p.trials()
                    .all(|t| !matches!(t.state, TrialState::Proposed | TrialState::Running));
            (done && ended).then_some(())
        }))
        .expect("the reflection is done and its trials have ended");
    idle(&o.run, &o.client, o.project, o.ceo);

    // The CEO has no agents under it, so its page is the Team tab's first.
    click(&mut o.vcx, "tab-Team");
    let team = o.vcx.read(|cx| o.main().read(cx).team().clone());
    let reflection: Entity<Reflection> = o
        .vcx
        .read(|cx| team.read(cx).reflection())
        .expect("the CEO's page");
    let mut trials: Vec<TrialCard> = Vec::new();
    pump_until(&mut o.vcx, "the CEO's page lists both trials", |vcx| {
        trials = vcx.read(|cx| reflection.read(cx).listed(cx).trials);
        trials.len() == 2 && trials.iter().all(|t| t.state != "Running")
    });
    let card = |name: &str| {
        trials
            .iter()
            .find(|t| t.change == format!("A new skill, {name}"))
            .unwrap_or_else(|| panic!("no trial of {name}: {trials:#?}"))
    };
    let (helpful, unhelpful) = (card("greeting"), card("noise"));
    assert_eq!(helpful.state, "Committed", "{helpful:#?}");
    assert_eq!(
        helpful.without,
        "Without the change: 3 of 6 checks passed over 3 runs, $0.01"
    );
    assert_eq!(
        helpful.with,
        "With the change: 6 of 6 checks passed over 3 runs, $0.01"
    );
    assert_eq!(helpful.why, None);
    assert_eq!(unhelpful.state, "Dropped", "{unhelpful:#?}");
    assert_eq!(
        unhelpful.without,
        "Without the change: 3 of 6 checks passed over 3 runs, $0.01"
    );
    assert_eq!(
        unhelpful.with,
        "With the change: 3 of 6 checks passed over 3 runs, $0.01"
    );
    // The reason is the daemon's, word for word.
    let why = o
        .state()
        .trials()
        .find(|t| t.state == TrialState::Discarded)
        .and_then(|t| t.why.clone())
        .expect("the daemon says why it dropped the change");
    assert!(why.starts_with("no better on any measure"), "{why}");
    assert_eq!(unhelpful.why, Some(format!("Dropped: {why}")));
}

#[test]
fn reflect_now_shows_the_daemons_refusal_while_the_ceo_is_on_the_builders_task() {
    let held = |p: &ProjectState| {
        p.tasks().any(|t| {
            t.task.title.starts_with("Hold the line")
                && matches!(t.state, TaskState::InProgress(..))
        }) && p.agents().any(|a| !a.waits.is_empty())
    };
    let mut o = Open::start("HOLD", held);
    let part = o.reflection_part();
    let before = o.state().tasks().count();

    let said = o.reflect_now(&part);
    let Reflected::Said(Outcome::Failed(why)) = said else {
        panic!("Reflect now was not refused: {said:?}");
    };
    assert!(
        why.starts_with("The CEO is still working on") && why.ends_with("reflect once it is done"),
        "{why}"
    );
    assert_eq!(o.state().tasks().count(), before, "no task was opened");
}
