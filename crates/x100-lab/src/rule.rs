//! Scoring and the rule for committing.
//!
//! A run of an arm is every item of the eval set, each by a lab driver in a fresh sandbox. Its
//! score adds up the items: checks passed of checks run (a replayed task reporting itself
//! completed is one check, each script another, each probe answered another), tool errors, turns,
//! time and cost, and the judge's mark where a rubric was used.
//!
//! The rule, across every run of both arms: the changed arm must be no worse on any check, and
//! better on at least one measure (checks passed, judged mark, fewer tool errors, fewer turns,
//! less time, less cost). Better means two things at once: by [`MARGIN_PERCENT`] of the baseline
//! over all the runs, and in every run, so that each run with the change is at least as good as
//! each run without it. The second is what keeps noise out: time and cost vary from run to run,
//! and a change that does nothing would otherwise win on them by chance about as often as not. A compaction is held to more: it
//! is tried only once it makes the memory smaller with an index that lists every note
//! ([`compaction`]), and is committed only when every probe question is answered in every run of
//! the changed arm. Anything else is discarded, with the reason.

use std::time::Duration;

use x100_core::money::Money;
use x100_core::trial::{Arm, RunScore, TrialChange, Verdict};

use crate::MARGIN_PERCENT;

/// What one item of one run came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemScore {
    pub passed: u32,
    pub total: u32,
    pub tool_errors: u32,
    pub turns: u32,
    pub duration: Duration,
    pub cost: Money,
    pub judged: Option<u8>,
}

/// One run of one arm: its items added up.
pub fn run_score(arm: Arm, run: u8, items: &[ItemScore]) -> RunScore {
    let judged: Vec<u32> = items
        .iter()
        .filter_map(|i| i.judged.map(u32::from))
        .collect();
    RunScore {
        arm,
        run,
        checks_passed: items.iter().map(|i| i.passed).sum(),
        checks_total: items.iter().map(|i| i.total).sum(),
        tool_errors: items.iter().map(|i| i.tool_errors).sum(),
        turns: items.iter().map(|i| i.turns).sum(),
        duration: items.iter().map(|i| i.duration).sum(),
        cost: items.iter().fold(Money::ZERO, |a, i| a + i.cost),
        judged: (!judged.is_empty())
            .then(|| u8::try_from(judged.iter().sum::<u32>() / judged.len() as u32).unwrap_or(100)),
    }
}

/// The verdict, and why, in words the builder and the CEO read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    pub verdict: Verdict,
    pub why: String,
}

impl Ruling {
    fn no(why: impl Into<String>) -> Ruling {
        Ruling {
            verdict: Verdict::DoesNotSupport,
            why: why.into(),
        }
    }
}

/// One arm's runs added up.
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    runs: u32,
    passed: u64,
    total: u64,
    tool_errors: u64,
    turns: u64,
    millis: u64,
    cost: u64,
    judged: Option<u64>,
}

fn tally(runs: &[RunScore], arm: Arm) -> Tally {
    let mut t = Tally::default();
    for r in runs.iter().filter(|r| r.arm == arm) {
        t.runs += 1;
        t.passed += u64::from(r.checks_passed);
        t.total += u64::from(r.checks_total);
        t.tool_errors += u64::from(r.tool_errors);
        t.turns += u64::from(r.turns);
        t.millis += u64::try_from(r.duration.as_millis()).unwrap_or(u64::MAX);
        t.cost += r.cost.micros();
        if let Some(j) = r.judged {
            t.judged = Some(t.judged.unwrap_or(0) + u64::from(j));
        }
    }
    t
}

/// One measure's value in each run of an arm.
fn per_run(runs: &[RunScore], arm: Arm, of: fn(&RunScore) -> u64) -> Vec<u64> {
    runs.iter().filter(|r| r.arm == arm).map(of).collect()
}

/// Whether the changed arm is better on a measure where more is better: by the margin over all
/// its runs, and with its worst run at least as good as the baseline's best.
fn higher(changed: &[u64], base: &[u64]) -> bool {
    let (c, b): (u64, u64) = (changed.iter().sum(), base.iter().sum());
    let worst = changed.iter().min();
    let best = base.iter().max();
    c > b && (c - b) * 100 >= b * MARGIN_PERCENT && worst >= best
}

/// The same where less is better.
fn lower(changed: &[u64], base: &[u64]) -> bool {
    let (c, b): (u64, u64) = (changed.iter().sum(), base.iter().sum());
    let worst = changed.iter().max();
    let best = base.iter().min();
    c < b && (b - c) * 100 >= b * MARGIN_PERCENT && worst <= best
}

/// Whether the trial supports its change, from every run of both arms. `runs_per_arm` is how
/// many each arm was to run.
pub fn rule(change: &TrialChange, runs: &[RunScore], runs_per_arm: u8) -> Ruling {
    let (base, changed) = (tally(runs, Arm::Baseline), tally(runs, Arm::Changed));
    let want = u32::from(runs_per_arm);
    if base.runs < want || changed.runs < want {
        return Ruling::no(format!(
            "not every run finished: {} of {want} without the change, {} with it",
            base.runs, changed.runs
        ));
    }
    if let TrialChange::CompactMemory { .. } = change {
        let missed = runs
            .iter()
            .filter(|r| r.arm == Arm::Changed && r.checks_passed < r.checks_total)
            .map(|r| r.run.to_string())
            .collect::<Vec<_>>();
        if !missed.is_empty() {
            return Ruling::no(format!(
                "a probe question went unanswered after compaction, in run {}",
                missed.join(", ")
            ));
        }
        return Ruling {
            verdict: Verdict::Supports,
            why: format!(
                "every probe question was answered in all {want} runs with the compacted memory"
            ),
        };
    }
    let checks = |t: &Tally| format!("{} of {} checks", t.passed, t.total);
    // Rates, compared without division: the arms may differ in how many checks they ran.
    if changed.passed * base.total < base.passed * changed.total {
        return Ruling::no(format!(
            "worse on checks: {} passed with the change, {} without it",
            checks(&changed),
            checks(&base)
        ));
    }
    if let (Some(c), Some(b)) = (changed.judged, base.judged)
        && c < b
    {
        return Ruling::no(format!(
            "the judge marked it lower: {} against {} over the runs",
            c / u64::from(want),
            b / u64::from(want)
        ));
    }
    let measure = |of: fn(&RunScore) -> u64| {
        (
            per_run(runs, Arm::Changed, of),
            per_run(runs, Arm::Baseline, of),
        )
    };
    let up = |of| {
        let (c, b) = measure(of);
        higher(&c, &b)
    };
    let down = |of| {
        let (c, b) = measure(of);
        lower(&c, &b)
    };
    let judged = changed.judged.is_some() && base.judged.is_some();
    let better = [
        (
            up(|r| u64::from(r.checks_passed)),
            format!("{} against {}", checks(&changed), checks(&base)),
        ),
        (
            judged && up(|r| u64::from(r.judged.unwrap_or(0))),
            "a higher mark from the judge".to_owned(),
        ),
        (
            down(|r| u64::from(r.tool_errors)),
            format!(
                "{} tool errors against {}",
                changed.tool_errors, base.tool_errors
            ),
        ),
        (
            down(|r| u64::from(r.turns)),
            format!("{} turns against {}", changed.turns, base.turns),
        ),
        (
            down(|r| u64::try_from(r.duration.as_millis()).unwrap_or(u64::MAX)),
            format!("{} ms against {}", changed.millis, base.millis),
        ),
        (
            down(|r| r.cost.micros()),
            format!("{} micro-dollars against {}", changed.cost, base.cost),
        ),
    ];
    match better.into_iter().find(|(b, _)| *b) {
        Some((_, how)) => Ruling {
            verdict: Verdict::Supports,
            why: format!("better with the change over {want} runs of each arm: {how}"),
        },
        None => Ruling::no(format!(
            "no better on any measure, by {MARGIN_PERCENT}% and in every run, over {want} runs \
             of each arm: {} with the change, {} without it",
            checks(&changed),
            checks(&base)
        )),
    }
}

/// A memory's size, as the rule for compactions reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySize {
    pub bytes: usize,
    /// Notes the index has no room to list.
    pub unlisted: usize,
}

/// Whether a compaction is worth trying: it must leave the memory smaller than it was, with an
/// index that lists every note. `Err` is why not, and the trial is discarded without a run.
pub fn compaction(before: MemorySize, after: MemorySize) -> Result<(), String> {
    if after.bytes >= before.bytes {
        return Err(format!(
            "the compaction leaves the memory at {} bytes, and it was {}: a compaction must \
             make it smaller",
            after.bytes, before.bytes
        ));
    }
    if after.unlisted > 0 {
        return Err(format!(
            "after the compaction the index is over its limit: {} notes would not be listed",
            after.unlisted
        ));
    }
    Ok(())
}

/// Whether the next run would pass the lab's cap: the cap is reached, or what has been spent
/// plus the dearest run so far would go past it.
pub fn would_pass_cap(spent: Money, cap: Money, dearest_run: Money) -> bool {
    spent >= cap || spent.micros().saturating_add(dearest_run.micros()) > cap.micros()
}
