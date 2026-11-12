//! What the daemon's own views show after its log: machines, harnesses and cool-offs,
//! credentials, projects, settings and jobs.

use std::time::Duration;

use x100_core::fold::{DaemonPart, DaemonState};
use x100_core::harness::HarnessKind;
use x100_core::job::JobState;
use x100_core::settings::DaemonSettings;
use x100_core::testing::{CLAUDE, CODEX, FERNLEAF, daemon_log, gpt, sonnet};
use x100_core::time::Timestamp;

fn state() -> DaemonState {
    DaemonState::fold(&daemon_log(Timestamp::from_millis(0))).unwrap()
}

#[test]
fn machines_and_harnesses_with_their_models_and_cool_offs() {
    let s = state();
    let machines: Vec<_> = s.machines().collect();
    assert_eq!(machines.len(), 1);
    assert_eq!((machines[0].name.as_str(), machines[0].cap), ("laptop", 6));

    assert_eq!(s.harness(CLAUDE).unwrap().kind, HarnessKind::ClaudeCode);
    assert_eq!(s.harness(CLAUDE).unwrap().models.len(), 2);
    // The update replaced Codex's record, its model's cache lifetime with it.
    let codex = s.harness(CODEX).unwrap();
    assert_eq!(codex.models[0].cache_lifetime, Duration::from_secs(3_600));

    // Codex's cool-off ended by its job; Sonnet's is still in force.
    assert!(s.cooling_off(&gpt()).is_none());
    let sonnet_cool_off = s.cooling_off(&sonnet()).unwrap();
    assert_eq!(
        sonnet_cool_off.cool_off.until,
        Timestamp::from_millis(3_600_000)
    );
    assert_eq!(s.cool_offs().len(), 1);
}

#[test]
fn credentials_by_reference_projects_settings_and_jobs() {
    let s = state();
    let credentials: Vec<_> = s.credentials().collect();
    assert_eq!(credentials.len(), 1);
    let cloudflare = credentials[0];
    assert_eq!(cloudflare.credential.name, "Cloudflare token");
    assert!(
        cloudflare
            .credential
            .scope
            .hosts
            .contains("api.cloudflare.com")
    );
    assert!(cloudflare.credential.scope.commands.contains("wrangler"));

    let projects: Vec<_> = s.projects().collect();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].0, FERNLEAF);
    assert_eq!(projects[0].1.name, "Fernleaf");

    assert_eq!(
        *s.settings(),
        DaemonSettings {
            stall_timeout: Duration::from_secs(600),
            ..DaemonSettings::default()
        }
    );

    let states: Vec<_> = s.jobs().map(|j| (j.state.name(), j.fired)).collect();
    assert_eq!(states, vec![("Done", 1), ("Scheduled", 0)]);
    assert!(matches!(
        s.jobs().nth(1).unwrap().state,
        JobState::Scheduled { .. }
    ));
}

#[test]
fn each_daemon_event_says_what_it_changed() {
    let mut s = DaemonState::default();
    for e in daemon_log(Timestamp::from_millis(0)) {
        let changes = s.apply(&e).unwrap();
        assert_eq!(changes.parts.len(), 1, "{:?}", e.event);
        if matches!(e.event, x100_core::event::DaemonEvent::SettingChanged(_)) {
            assert!(changes.parts.contains(&DaemonPart::Settings));
        }
    }
}
