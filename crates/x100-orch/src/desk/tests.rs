//! Closing the desk on a driver while a call it made is still being carried out. The project's
//! writer lock, which carrying out a call needs, holds the call there for as long as the test
//! likes, so nothing here depends on timing.

use std::future::Future;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::Poll;

use x100_core::action::{ActionState, Request};
use x100_core::dice::SeededDice;
use x100_core::event::ProjectEvent;
use x100_core::id::ProjectId;
use x100_core::money::Percent;
use x100_core::settings::DaemonSettings;
use x100_core::task::Progress;
use x100_core::testing::fernleaf;
use x100_core::time::{Clock, FakeClock};
use x100_core::tool::ToolCall;
use x100_jobs::Scheduler;
use x100_store::ProjectStore;
use x100_vault::Redactor;

use super::Desk;
use crate::ctx::ProjectCtx;
use crate::tools::Tool;

fn note(call: &ToolCall) -> Option<&str> {
    match call {
        ToolCall::UpdateTask { progress, .. } => Some(&progress.note),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_close_waits_for_a_call_admitted_before_it_which_takes_effect_once() {
    let f = fernleaf();
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeClock::new(f.log[0].at);
    let clock: Arc<dyn Clock> = Arc::new(fake.clone());
    let store = ProjectStore::open(dir.path(), Arc::clone(&clock)).unwrap();
    for e in &f.log {
        fake.set(e.at).unwrap();
        store.log.append(e.by, e.event.clone()).unwrap();
    }
    let jobs = Scheduler::new(store.log.clone(), Arc::clone(&clock));
    let dice = Arc::new(SeededDice::new(7));
    let ctx = ProjectCtx::new(ProjectId::from_u128(1), store, clock, dice, jobs);
    let desk = Desk::new(Arc::new(DaemonSettings::default), Redactor::for_values(&[]));
    // The backend's driver, live and acting for it at the end of the scenario.
    let driver = f.backend_driver;
    let token = desk.issue(driver, Arc::clone(&ctx)).token;
    let update = |text: &str| ToolCall::UpdateTask {
        task: f.api,
        progress: Progress {
            note: text.into(),
            done: Some(Percent::new(90).unwrap()),
        },
        effort: None,
    };

    let (locked, is_locked) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let holder = std::thread::spawn({
        let ctx = Arc::clone(&ctx);
        move || {
            let _writer = ctx.writer();
            locked.send(()).unwrap();
            let _ = released.recv();
        }
    });
    is_locked.recv().unwrap();

    let reply = desk.call(driver, &token, update("IN-FLIGHT"));
    let close = desk.close(driver);
    tokio::pin!(close);
    let first = std::future::poll_fn(|cx| Poll::Ready(close.as_mut().poll(cx))).await;
    assert!(
        first.is_pending(),
        "the close returned while a call it admitted was still being carried out: {first:?}"
    );
    let late = desk.call(driver, &token, update("LATE")).await.unwrap_err();
    assert!(late.contains("stopped acting"), "{late}");

    release.send(()).unwrap();
    holder.join().unwrap();
    let (action, tool) = close
        .await
        .expect("the close names the call that reached the log");
    reply
        .await
        .expect("the call admitted before the close is carried out");
    assert_eq!(tool, Tool::UpdateTask);

    let s = ctx.store.log.state();
    let record = s.action(action).expect("the call's action");
    assert_eq!(record.state, ActionState::CarriedOut);
    assert!(
        matches!(&record.action.request, Request::Tool { call, .. } if note(call) == Some("IN-FLIGHT"))
    );
    assert_eq!(
        s.task(f.api)
            .unwrap()
            .progress
            .as_ref()
            .map(|p| p.note.as_str()),
        Some("IN-FLIGHT")
    );
    let proposed: Vec<_> = ctx
        .store
        .log
        .read_after(None, 100_000)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.event {
            ProjectEvent::ActionProposed(a) => match a.request {
                Request::Tool { call, .. } => note(&call)
                    .filter(|n| ["IN-FLIGHT", "LATE"].contains(n))
                    .map(str::to_owned),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        proposed,
        ["IN-FLIGHT"],
        "once, and the late call not at all"
    );
}
