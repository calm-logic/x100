//! The rule for committing, on scores made up to sit on either side of it.

use std::time::Duration;

use x100_core::id::AgentId;
use x100_core::memory::{SkillName, SkillRef, SkillScope};
use x100_core::money::Money;
use x100_core::trial::{Arm, RunScore, TrialChange, Verdict};
use x100_lab::rule::{ItemScore, MemorySize, compaction, rule, run_score, would_pass_cap};

fn skill() -> TrialChange {
    TrialChange::EditSkill {
        skill: SkillRef {
            scope: SkillScope::Shared,
            name: SkillName("deploy".into()),
        },
        body: String::new(),
    }
}

fn compact() -> TrialChange {
    TrialChange::CompactMemory {
        agent: AgentId::from_u128(1),
        notes: Vec::new(),
    }
}

fn score(arm: Arm, run: u8, passed: u32, total: u32, cost: u64) -> RunScore {
    RunScore {
        arm,
        run,
        checks_passed: passed,
        checks_total: total,
        tool_errors: 0,
        turns: 1,
        duration: Duration::from_secs(60),
        cost: Money::from_micros(cost),
        judged: None,
    }
}

/// Three runs of each arm, the baseline passing `base` of 2 checks and the change `changed`.
fn runs(base: u32, changed: u32, base_cost: u64, changed_cost: u64) -> Vec<RunScore> {
    (1..=3)
        .flat_map(|r| {
            [
                score(Arm::Baseline, r, base, 2, base_cost),
                score(Arm::Changed, r, changed, 2, changed_cost),
            ]
        })
        .collect()
}

#[test]
fn a_change_that_passes_more_checks_is_supported() {
    let r = rule(&skill(), &runs(1, 2, 100, 100), 3);
    assert_eq!(r.verdict, Verdict::Supports, "{}", r.why);
    assert!(r.why.contains("6 of 6 checks against 3 of 6"), "{}", r.why);
}

#[test]
fn a_change_no_better_by_the_margin_is_discarded() {
    let r = rule(&skill(), &runs(2, 2, 100, 95), 3);
    assert_eq!(r.verdict, Verdict::DoesNotSupport);
    assert!(
        r.why.contains("no better on any measure, by 10%"),
        "{}",
        r.why
    );
}

#[test]
fn a_cheaper_change_with_the_same_checks_is_supported() {
    let r = rule(&skill(), &runs(2, 2, 100, 80), 3);
    assert_eq!(r.verdict, Verdict::Supports, "{}", r.why);
    assert!(r.why.contains("micro-dollars"), "{}", r.why);
}

#[test]
fn a_change_worse_on_checks_is_discarded_however_much_cheaper() {
    let r = rule(&skill(), &runs(2, 1, 100, 1), 3);
    assert_eq!(r.verdict, Verdict::DoesNotSupport);
    assert!(r.why.starts_with("worse on checks"), "{}", r.why);
}

#[test]
fn a_trial_whose_runs_did_not_all_finish_is_discarded() {
    let mut all = runs(1, 2, 100, 100);
    all.pop();
    let r = rule(&skill(), &all, 3);
    assert_eq!(r.verdict, Verdict::DoesNotSupport);
    assert!(r.why.starts_with("not every run finished"), "{}", r.why);
}

#[test]
fn a_compaction_needs_every_probe_answered_in_every_run() {
    assert_eq!(
        rule(&compact(), &runs(2, 2, 10, 10), 3).verdict,
        Verdict::Supports
    );
    let mut one_missed = runs(2, 2, 10, 10);
    one_missed[3].checks_passed = 1;
    let r = rule(&compact(), &one_missed, 3);
    assert_eq!(r.verdict, Verdict::DoesNotSupport);
    assert!(r.why.contains("in run 2"), "{}", r.why);
}

#[test]
fn a_compaction_must_make_the_memory_smaller_and_fit_its_index() {
    let size = |bytes, unlisted| MemorySize { bytes, unlisted };
    assert!(compaction(size(1000, 3), size(600, 0)).is_ok());
    assert!(compaction(size(1000, 0), size(1000, 0)).is_err());
    assert!(compaction(size(1000, 0), size(1200, 0)).is_err());
    let over = compaction(size(9000, 10), size(8000, 2)).unwrap_err();
    assert!(over.contains("over its limit"), "{over}");
}

#[test]
fn a_run_adds_up_its_items() {
    let item = |passed, judged| ItemScore {
        passed,
        total: 2,
        tool_errors: 1,
        turns: 2,
        duration: Duration::from_secs(5),
        cost: Money::from_micros(7),
        judged,
    };
    let r = run_score(
        Arm::Changed,
        2,
        &[item(2, Some(80)), item(1, Some(60)), item(0, None)],
    );
    assert_eq!((r.checks_passed, r.checks_total), (3, 6));
    assert_eq!((r.tool_errors, r.turns), (3, 6));
    assert_eq!(r.duration, Duration::from_secs(15));
    assert_eq!(r.cost, Money::from_micros(21));
    assert_eq!(r.judged, Some(70));
}

#[test]
fn a_run_that_would_pass_the_cap_is_not_started() {
    let m = Money::from_micros;
    assert!(!would_pass_cap(m(0), m(100), m(0)));
    assert!(!would_pass_cap(m(60), m(100), m(40)));
    assert!(would_pass_cap(m(61), m(100), m(40)));
    assert!(would_pass_cap(m(100), m(100), m(0)));
}

#[test]
fn a_measure_better_on_the_total_but_not_in_every_run_is_noise() {
    // Faster by a third over the runs, but one run with the change was slower than one without:
    // time varies, and that is what a change that does nothing looks like.
    let seconds = |arm, run, s| RunScore {
        duration: Duration::from_secs(s),
        ..score(arm, run, 2, 2, 100)
    };
    let noisy = vec![
        seconds(Arm::Baseline, 1, 90),
        seconds(Arm::Baseline, 2, 100),
        seconds(Arm::Baseline, 3, 110),
        seconds(Arm::Changed, 1, 40),
        seconds(Arm::Changed, 2, 50),
        seconds(Arm::Changed, 3, 95),
    ];
    assert_eq!(rule(&skill(), &noisy, 3).verdict, Verdict::DoesNotSupport);
    let mut clear = noisy.clone();
    clear[5].duration = Duration::from_secs(80);
    let r = rule(&skill(), &clear, 3);
    assert_eq!(r.verdict, Verdict::Supports, "{}", r.why);
    assert!(r.why.contains("ms against"), "{}", r.why);
}
