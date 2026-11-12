//! What is due, as a pure function of the jobs and the time. The scheduler reads the log's state
//! into these and acts on the answer, so the decision itself needs no log, clock or runtime.

use x100_core::id::JobId;
use x100_core::job::JobState;
use x100_core::time::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Due {
    /// First so that the derived order is the order of firing.
    pub due: Timestamp,
    pub job: JobId,
}

/// The jobs due at `now`, in the order they fire: earliest first, and by id when two are due
/// together. A repeating job appears once however many times it has come round.
pub fn due(jobs: impl IntoIterator<Item = (JobId, JobState)>, now: Timestamp) -> Vec<Due> {
    let mut due: Vec<Due> = jobs
        .into_iter()
        .filter_map(|(job, state)| match state {
            JobState::Scheduled { due, .. } if due <= now => Some(Due { due, job }),
            _ => None,
        })
        .collect();
    due.sort();
    due
}

/// When the next job falls due, which may be in the past.
pub fn next_due(jobs: impl IntoIterator<Item = (JobId, JobState)>) -> Option<Timestamp> {
    jobs.into_iter()
        .filter_map(|(_, state)| match state {
            JobState::Scheduled { due, .. } => Some(due),
            _ => None,
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use x100_core::job::Interval;

    fn at(ms: u64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn scheduled(ms: u64) -> JobState {
        JobState::Scheduled {
            due: at(ms),
            every: None,
        }
    }

    fn id(n: u128) -> JobId {
        JobId::from_u128(n)
    }

    #[test]
    fn only_scheduled_jobs_at_or_past_their_time_are_due_and_in_firing_order() {
        let every = Interval::new(std::time::Duration::from_secs(1)).unwrap();
        let jobs = [
            (id(1), scheduled(300)),
            (id(2), scheduled(100)),
            (id(3), JobState::Done),
            (id(4), JobState::Cancelled),
            (id(5), scheduled(100)),
            (id(6), scheduled(301)),
            (
                id(7),
                JobState::Scheduled {
                    due: at(50),
                    every: Some(every),
                },
            ),
        ];
        let order: Vec<_> = due(jobs, at(300)).into_iter().map(|d| d.job).collect();
        assert_eq!(order, [id(7), id(2), id(5), id(1)]);
        assert_eq!(next_due(jobs), Some(at(50)));
    }

    #[test]
    fn with_nothing_scheduled_nothing_is_due() {
        let jobs = [(id(1), JobState::Done), (id(2), JobState::Cancelled)];
        assert!(due(jobs, at(u64::MAX)).is_empty());
        assert_eq!(next_due(jobs), None);
    }
}
