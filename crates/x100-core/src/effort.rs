//! Effort: how much capability a task needs and a driver gives.

use serde::{Deserialize, Serialize};

/// Low to Max, in order, so efforts compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 4] = [Effort::Low, Effort::Medium, Effort::High, Effort::Max];
}

/// The lowest and highest effort an agent's drivers may run at. The lowest is never above the
/// highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawRange", into = "RawRange")]
pub struct EffortRange {
    min: Effort,
    max: Effort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an effort range from {min:?} to {max:?} is empty")]
pub struct EmptyRange {
    pub min: Effort,
    pub max: Effort,
}

impl EffortRange {
    pub const FULL: EffortRange = EffortRange {
        min: Effort::Low,
        max: Effort::Max,
    };

    pub fn new(min: Effort, max: Effort) -> Result<Self, EmptyRange> {
        if min <= max {
            Ok(Self { min, max })
        } else {
            Err(EmptyRange { min, max })
        }
    }

    pub fn min(self) -> Effort {
        self.min
    }

    pub fn max(self) -> Effort {
        self.max
    }

    pub fn contains(self, effort: Effort) -> bool {
        self.min <= effort && effort <= self.max
    }

    /// The effort a task needs once it is brought inside this range. A task that asks for more than
    /// the agent may ever use runs at the agent's top, and one that asks for less runs at its
    /// floor.
    pub fn clamp(self, needed: Effort) -> Effort {
        needed.clamp(self.min, self.max)
    }

    /// Rule 1 of "Finding a driver": a driver's effort is at least what the task needs, once that
    /// is clamped into the agent's range, and is itself inside the range.
    pub fn admits(self, driver: Effort, task_needs: Effort) -> bool {
        self.contains(driver) && driver >= self.clamp(task_needs)
    }
}

#[derive(Serialize, Deserialize)]
struct RawRange {
    min: Effort,
    max: Effort,
}

impl TryFrom<RawRange> for EffortRange {
    type Error = EmptyRange;

    fn try_from(raw: RawRange) -> Result<Self, EmptyRange> {
        EffortRange::new(raw.min, raw.max)
    }
}

impl From<EffortRange> for RawRange {
    fn from(r: EffortRange) -> Self {
        RawRange {
            min: r.min,
            max: r.max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn effort() -> impl Strategy<Value = Effort> {
        prop::sample::select(Effort::ALL.to_vec())
    }

    #[test]
    fn an_empty_range_cannot_be_made() {
        assert!(EffortRange::new(Effort::High, Effort::Low).is_err());
        assert!(serde_json::from_str::<EffortRange>(r#"{"min":"max","max":"low"}"#).is_err());
        let ok: EffortRange = serde_json::from_str(r#"{"min":"low","max":"high"}"#).unwrap();
        assert_eq!(ok, EffortRange::new(Effort::Low, Effort::High).unwrap());
    }

    #[test]
    fn a_task_above_the_range_runs_at_the_top_of_it() {
        let range = EffortRange::new(Effort::Low, Effort::Medium).unwrap();
        assert!(range.admits(Effort::Medium, Effort::Max));
        assert!(!range.admits(Effort::High, Effort::Max));
        assert!(!range.admits(Effort::Low, Effort::Max));
    }

    proptest! {
        #[test]
        fn an_admitted_driver_is_in_range_and_meets_the_clamped_need(
            a in effort(), b in effort(), driver in effort(), need in effort()
        ) {
            let range = EffortRange::new(a.min(b), a.max(b)).unwrap();
            let admitted = range.admits(driver, need);
            prop_assert_eq!(admitted, range.contains(driver) && driver >= range.clamp(need));
            if admitted {
                prop_assert!(range.min() <= driver && driver <= range.max());
                prop_assert!(driver >= need || driver == range.max());
            }
            // Some driver in the range is always admitted, so a task never asks for the impossible.
            prop_assert!(range.admits(range.max(), need));
        }
    }
}
