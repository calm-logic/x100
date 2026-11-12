//! Time as the rest of x100 sees it: a timestamp, the `Clock` trait, and a fake clock for tests.
//!
//! Everything time-based in x100 reads one clock, so a test that moves the fake clock by hand
//! drives jobs, deadlines, warm periods and stall detection the same way real time would. The real
//! clock is in `x100-jobs`, since reading the system time is I/O.

use std::ops::{Add, Sub};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch, in UTC.
///
/// Milliseconds match the ULIDs the ids are built from and are fine enough for jobs and deadlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const EPOCH: Timestamp = Timestamp(0);

    pub const fn from_millis(ms: u64) -> Self {
        Self(ms)
    }

    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// The time from `earlier` to `self`, or zero if `earlier` is later.
    pub fn since(self, earlier: Timestamp) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl Add<Duration> for Timestamp {
    type Output = Timestamp;

    fn add(self, d: Duration) -> Timestamp {
        Timestamp(self.0.saturating_add(millis(d)))
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Timestamp;

    fn sub(self, d: Duration) -> Timestamp {
        Timestamp(self.0.saturating_sub(millis(d)))
    }
}

/// The one source of the current time.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// A clock that stands still until a test moves it.
///
/// Clones share one time, so a test keeps a clone and moves the clock that the code under test
/// holds.
#[derive(Debug, Clone, Default)]
pub struct FakeClock {
    now: Arc<AtomicU64>,
}

impl FakeClock {
    pub fn new(start: Timestamp) -> Self {
        Self {
            now: Arc::new(AtomicU64::new(start.0)),
        }
    }

    pub fn advance(&self, by: Duration) {
        let by = millis(by);
        // try_update cannot fail with a closure that always returns Some.
        let _ = self
            .now
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |t| {
                Some(t.saturating_add(by))
            });
    }

    /// Moves the clock to `to`. Time never runs backwards in x100, so a time in the past is
    /// refused.
    pub fn set(&self, to: Timestamp) -> Result<(), ClockWentBack> {
        self.now
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |t| {
                (to.0 >= t).then_some(to.0)
            })
            .map(|_| ())
            .map_err(|t| ClockWentBack {
                now: Timestamp(t),
                asked: to,
            })
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.now.load(Ordering::SeqCst))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the clock is at {now:?} and cannot be set back to {asked:?}")]
pub struct ClockWentBack {
    pub now: Timestamp,
    pub asked: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_clock_stands_still_until_moved() {
        let clock = FakeClock::new(Timestamp::from_millis(1_000));
        assert_eq!(clock.now(), Timestamp::from_millis(1_000));
        assert_eq!(clock.now(), Timestamp::from_millis(1_000));
        clock.advance(Duration::from_secs(2));
        assert_eq!(clock.now(), Timestamp::from_millis(3_000));
    }

    #[test]
    fn clones_share_one_time() {
        let clock = FakeClock::new(Timestamp::EPOCH);
        let held_by_code = clock.clone();
        clock.advance(Duration::from_millis(5));
        assert_eq!(held_by_code.now(), Timestamp::from_millis(5));
    }

    #[test]
    fn the_fake_clock_never_goes_back() {
        let clock = FakeClock::new(Timestamp::from_millis(10));
        assert!(clock.set(Timestamp::from_millis(20)).is_ok());
        assert_eq!(
            clock.set(Timestamp::from_millis(15)),
            Err(ClockWentBack {
                now: Timestamp::from_millis(20),
                asked: Timestamp::from_millis(15)
            })
        );
        assert_eq!(clock.now(), Timestamp::from_millis(20));
    }

    #[test]
    fn it_works_behind_the_trait() {
        let clock: Box<dyn Clock> = Box::new(FakeClock::new(Timestamp::from_millis(7)));
        assert_eq!(clock.now(), Timestamp::from_millis(7));
    }

    #[test]
    fn timestamp_arithmetic_saturates() {
        let t = Timestamp::from_millis(100);
        assert_eq!(t + Duration::from_millis(50), Timestamp::from_millis(150));
        assert_eq!(t - Duration::from_secs(1), Timestamp::EPOCH);
        assert_eq!(
            t.since(Timestamp::from_millis(40)),
            Duration::from_millis(60)
        );
        assert_eq!(Timestamp::from_millis(40).since(t), Duration::ZERO);
    }
}
