//! How agents look, against a real daemon on the fake clock, without a display: the first run opens
//! on the box for the first message with no face and plays its opening once, skippably, and not
//! under reduced motion; the CEO is drawn in Team by the same face as its leads, opens its own page,
//! and its look changes from a popover on that page, showing in Team and the Log and surviving a
//! restart of the daemon; leads the CEO forms take its colour and outlines of their own; and the
//! primary actions are drawn by the glass button.

mod common;

use std::collections::BTreeSet;

use common::*;
use gpui::{Entity, VisualTestContext, WindowHandle};
use x100_app::chat::Opening;
use x100_app::chat::StartView;
use x100_app::log::LogView;
use x100_app::shell::Shell;
use x100_app::tabs::{MainPanel, Tab};
use x100_app::team::{LookTab, TeamView};
use x100_client::{Client, Options};
use x100_core::appearance::{Appearance, Hue, Outline};
use x100_core::fold::ProjectState;
use x100_core::id::ProjectId;
use x100_proto::{CreateProject, GetProject};

fn start_view(window: WindowHandle<Shell>, vcx: &VisualTestContext) -> Entity<StartView> {
    vcx.read(|cx| window.read(cx).unwrap().start().cloned())
        .expect("the start of a new project")
}

#[test]
fn the_first_run_opens_on_the_box_with_no_face_and_plays_its_opening_once() {
    let mut run = Run::start();
    let client = run.client();
    let (window, mut vcx) = run.open(client.clone());
    pump_until(&mut vcx, "the message box is drawn", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });
    assert!(
        bounds(&mut vcx, "face").is_none(),
        "no face on the first run"
    );
    let start = start_view(window, &vcx);
    let opening = vcx.read(|cx| start.read(cx).opening());
    assert!(matches!(opening, Opening::Playing(_)), "{opening:?}");
    // A click anywhere ends it, and the box is there under it.
    click(&mut vcx, "composer-input");
    assert_eq!(vcx.read(|cx| start.read(cx).opening()), Opening::Off);

    // Under reduced motion it never plays.
    run.cx.update(|cx| cx.set_reduce_motion(true));
    let (second, mut vcx2) = run.open(client.clone());
    pump_until(&mut vcx2, "the second window's box", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });
    let start = start_view(second, &vcx2);
    assert_eq!(vcx2.read(|cx| start.read(cx).opening()), Opening::Off);
    run.cx.update(|cx| cx.set_reduce_motion(false));

    // Once a project exists it is not the first run: no opening, and a project's chat before its
    // first message is the box alone, with no face either.
    let project = run
        .rt
        .block_on(client.request(CreateProject {
            dir: None,
            settings: None,
        }))
        .expect("created")
        .project;
    let (third, mut vcx3) = run.open(client.clone());
    pump_until(&mut vcx3, "the third window's box", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });
    // The window may open on the project itself; either way nothing plays.
    let opening = vcx3.read(|cx| {
        third
            .read(cx)
            .unwrap()
            .start()
            .map(|s| s.read(cx).opening())
    });
    assert!(matches!(opening, None | Some(Opening::Off)), "{opening:?}");
    let session = vcx3.read(|cx| third.read(cx).unwrap().session().clone());
    vcx3.update(|_, cx| session.update(cx, |s, cx| s.select(project, cx)));
    pump_until(&mut vcx3, "the project's chat is shown", |vcx| {
        let shown = vcx.read(|cx| third.read(cx).unwrap().project(project).is_some());
        shown && bounds(vcx, "composer-input").is_some()
    });
    assert!(bounds(&mut vcx3, "face").is_none(), "no face in the chat");
}

struct Open {
    run: Run,
    client: Client,
    project: ProjectId,
    window: WindowHandle<Shell>,
    vcx: VisualTestContext,
}

fn fernleaf() -> Open {
    let mut run = Run::start();
    let client = run.client();
    let project = run.fernleaf_project(&client);
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
        let (window, project) = (self.window, self.project);
        self.vcx.read(|cx| {
            window
                .read(cx)
                .unwrap()
                .project(project)
                .unwrap()
                .main
                .clone()
        })
    }

    fn team(&self) -> Entity<TeamView> {
        let main = self.main();
        self.vcx.read(|cx| main.read(cx).team().clone())
    }

    fn has(&mut self, selector: String) -> bool {
        bounds(&mut self.vcx, Box::leak(selector.into_boxed_str())).is_some()
    }

    fn ceo_look(&self) -> Appearance {
        let s = self.state();
        s.agent(s.project().unwrap().ceo).unwrap().appearance
    }

    fn until_ceo_is(&mut self, look: Appearance) {
        let (client, project) = (self.client.clone(), self.project);
        pump_until(
            &mut self.vcx,
            "the CEO's new look is in the state",
            move |_| {
                let s = client.project(project).unwrap();
                s.agent(s.project().unwrap().ceo).unwrap().appearance == look
            },
        );
    }
}

#[test]
fn the_ceo_is_drawn_like_its_leads_and_its_look_changes_from_its_page() {
    let mut o = fernleaf();
    let s = o.state();
    let ceo = s.project().unwrap().ceo;

    // Leads formed by the CEO: its colour (none), outlines other than its circle, none shared.
    assert_eq!(o.ceo_look(), Appearance::default());
    let leads: Vec<Appearance> = s
        .children(ceo)
        .into_iter()
        .map(|a| s.agent(a).unwrap().appearance)
        .collect();
    assert_eq!(leads.len(), 3);
    assert!(
        leads
            .iter()
            .all(|l| l.hue.is_none() && l.outline != Outline::Circle)
    );
    let outlines: BTreeSet<_> = leads.iter().map(|l| l.outline).collect();
    assert_eq!(outlines.len(), 3, "{leads:?}");
    for (_, look) in s.agents().map(|a| (a.id, a.appearance)) {
        assert!(Outline::ALL.contains(&look.outline));
    }

    // The top level: the CEO's head and each lead's row draw the same face, each in its own look.
    pump_until(&mut o.vcx, "the CEO's head is drawn", |vcx| {
        bounds(vcx, "head").is_some()
    });
    assert!(o.has(x100_app::face::selector(o.ceo_look())));
    for look in &leads {
        assert!(o.has(x100_app::face::selector(*look)), "{look:?}");
    }

    // Clicking the CEO opens its page, the drilldown any agent has, with the way back to the team.
    click(&mut o.vcx, "head");
    let team = o.team();
    assert_eq!(o.vcx.read(|cx| team.read(cx).focus()), Some(ceo));
    assert!(o.vcx.read(|cx| team.read(cx).drilled()));
    assert!(o.has("crumb-0".into()), "the way back");
    assert!(o.has("drill-head".into()));

    // Its face opens the popover, on the Outline tab; a choice applies at once.
    click(&mut o.vcx, "drill-head");
    assert!(o.has("look-popover".into()));
    let picker = o.vcx.read(|cx| team.read(cx).picker()).expect("open");
    assert_eq!(o.vcx.read(|cx| picker.read(cx).tab()), LookTab::Outline);
    assert!(o.has("look-outline-hexagon".into()));
    click(&mut o.vcx, "look-outline-hexagon");
    o.until_ceo_is(Appearance {
        outline: Outline::Hexagon,
        hue: None,
    });
    click(&mut o.vcx, "look-tab-color");
    assert_eq!(o.vcx.read(|cx| picker.read(cx).tab()), LookTab::Color);
    let teal_hexagon = Appearance {
        outline: Outline::Hexagon,
        hue: Some(Hue::Teal),
    };
    click(&mut o.vcx, "look-color-teal");
    o.until_ceo_is(teal_hexagon);
    pump_until(&mut o.vcx, "the face in the corner follows", |vcx| {
        bounds(vcx, "face-hexagon-teal").is_some()
    });
    // Escape closes it.
    o.vcx.simulate_keystrokes("escape");
    pump(&mut o.vcx);
    assert!(o.vcx.read(|cx| team.read(cx).picker().is_none()));
    assert!(!o.has("look-popover".into()));

    // The Log says so, and a row the CEO acted in shows its new face.
    let tab = Box::leak(format!("tab-{}", Tab::Log.name()).into_boxed_str());
    click(&mut o.vcx, tab);
    let main = o.main();
    let log: Entity<LogView> = o
        .vcx
        .read(|cx| main.read(cx).tab_view::<LogView>(Tab::Log))
        .expect("the Log");
    pump_until(&mut o.vcx, "the Log has the change", |vcx| {
        vcx.read(|cx| {
            log.read(cx)
                .lines()
                .iter()
                .any(|l| l.text == "Made CEO a teal hexagon")
        })
    });
    pump_until(&mut o.vcx, "a row by the CEO shows its face", |vcx| {
        bounds(vcx, "face-hexagon-teal").is_some()
    });

    // It is in the log, so it outlives the daemon.
    let last = o.state().seq();
    let at = o
        .run
        .rt
        .block_on(o.client.read_events_before(o.project, None, 1))
        .expect("the newest event")[0]
        .at;
    assert!(last.is_some());
    o.run.daemon.restart(at);
    let mut options = Options::new(o.run.daemon.socket(), "test-after");
    options.startup_timeout = WAIT;
    let after = o
        .run
        .rt
        .block_on(Client::connect(options))
        .expect("the daemon is back");
    let state = o
        .run
        .rt
        .block_on(after.request(GetProject { project: o.project }))
        .expect("the project");
    assert_eq!(
        state
            .agent(state.project().unwrap().ceo)
            .unwrap()
            .appearance,
        teal_hexagon
    );
}

/// Every primary action is the glass button, and still does what it did.
#[test]
fn the_primary_actions_are_glass_buttons() {
    let mut o = fernleaf();
    assert!(o.has("primary-send".into()), "Send in the composer");
    let tab = Box::leak(format!("tab-{}", Tab::Settings.name()).into_boxed_str());
    click(&mut o.vcx, tab);
    pump_until(&mut o.vcx, "Settings is drawn", |vcx| {
        bounds(vcx, "decision-connect").is_some()
    });
    assert!(o.has("primary-decision-connect".into()));
    // Send still sends.
    let (window, project) = (o.window, o.project);
    let composer = o.vcx.read(|cx| {
        let chat = window
            .read(cx)
            .unwrap()
            .project(project)
            .unwrap()
            .chat
            .clone();
        chat.read(cx).composer().clone()
    });
    o.vcx
        .update(|window, cx| composer.update(cx, |c, cx| c.focus(window, cx)));
    o.vcx.simulate_input("Forest greens.");
    let before = o.state().messages().count();
    click(&mut o.vcx, "send");
    let (client, project) = (o.client.clone(), o.project);
    pump_until(&mut o.vcx, "the message reaches the daemon", move |_| {
        client.project(project).unwrap().messages().count() > before
    });
}
