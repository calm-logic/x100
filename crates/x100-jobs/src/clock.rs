use std::time::{SystemTime, UNIX_EPOCH};

use x100_core::time::{Clock, Timestamp};

/// The system's wall clock, behind core's [`Clock`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        // A system clock set before 1970 reads as the epoch rather than failing: nothing is
        // scheduled that early.
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Timestamp::from_millis(u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_the_current_time_in_milliseconds() {
        // 2025-01-01 in milliseconds, which any machine running this is past.
        assert!(SystemClock.now().as_millis() > 1_735_689_600_000);
        assert!(SystemClock.now() <= SystemClock.now() + std::time::Duration::from_secs(1));
    }
}
