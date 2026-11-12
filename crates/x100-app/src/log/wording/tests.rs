use super::*;
use x100_core::testing::{daemon_log, fernleaf};

#[test]
fn every_event_of_the_fernleaf_run_reads_as_a_sentence() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).expect("the scenario folds");
    let daemon = DaemonState::fold(&daemon_log(x100_core::testing::START)).expect("folds");
    let files = HashMap::new();
    let names = Names {
        project: &state,
        daemon: &daemon,
        files: &files,
    };
    let lines: Vec<Line> = f.log.iter().map(|e| line(e, &names)).collect();
    assert_eq!(lines.len(), f.log.len());
    for (e, l) in f.log.iter().zip(&lines) {
        assert!(
            !l.who.is_empty() && l.text.len() > 8,
            "event {} gave {l:?}",
            e.seq.0
        );
        assert!(
            !l.text.contains("an agent") && !l.text.contains("a task"),
            "event {} named something the state does not hold: {l:?}",
            e.seq.0
        );
    }
    let all: Vec<String> = lines
        .iter()
        .map(|l| format!("{} {}", l.who, l.text))
        .collect();
    let has = |needle: &str| {
        assert!(
            all.iter().any(|l| l.contains(needle)),
            "no line says {needle:?}"
        );
    };
    // The things the Log tab is for: driver choices and handoffs, jobs, workers, files,
    // settings, held actions.
    has("Chose ");
    has("taking over from");
    has("Scheduled ");
    has("Fired ");
    has("Started a Backend agent under Tech");
    has("Added ");
    has("Held for the builder: ");
    has("Changed Ask me about to");
    has("Brought in a Tech lead");
}

#[test]
fn who_is_the_actor() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let daemon = DaemonState::default();
    let files = HashMap::new();
    let names = Names {
        project: &state,
        daemon: &daemon,
        files: &files,
    };
    let by_builder = f.log.iter().find(|e| e.by == Actor::Builder).unwrap();
    assert_eq!(line(by_builder, &names).who, "You");
    let by_daemon = f.log.iter().find(|e| e.by == Actor::Daemon).unwrap();
    assert_eq!(line(by_daemon, &names).who, "x100");
    let by_agent = f.log.iter().find(|e| e.by == Actor::Agent(f.tech)).unwrap();
    assert_eq!(line(by_agent, &names).who, "Tech");
}

#[test]
fn a_removed_file_is_named_from_the_events_that_added_it() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let daemon = DaemonState::default();
    let mut files = HashMap::new();
    files.insert(FileId::from_u128(7), "moodboard.png".to_owned());
    let names = Names {
        project: &state,
        daemon: &daemon,
        files: &files,
    };
    let removed = ProjectEvent::FileRemoved {
        file: FileId::from_u128(7),
    };
    assert_eq!(
        text(&removed, &names),
        "Removed moodboard.png to save space"
    );
    let unknown = ProjectEvent::FileRemoved {
        file: FileId::from_u128(8),
    };
    assert_eq!(text(&unknown, &names), "Removed a file to save space");
}
