use x100_core::event::{DaemonEvent, ProjectEvent};
use x100_core::fold::{DaemonState, JobRecord, ProjectState};
use x100_core::id::JobId;
use x100_core::job::{DaemonJob, Job, JobEvent, ProjectJob};
use x100_store::Folded;

/// A folded state that holds jobs: a project's, or the daemon's. It says how to read the jobs out
/// of the state and how to wrap a job's events, so one scheduler serves both logs.
pub trait JobHost: Folded {
    /// What a job in this log does.
    type Does: Clone + Send + 'static;

    fn records(&self) -> impl Iterator<Item = &JobRecord<Self::Does>>;
    fn scheduled(job: Job<Self::Does>) -> Self::Event;
    fn job_event(job: JobId, event: JobEvent) -> Self::Event;
}

impl JobHost for ProjectState {
    type Does = ProjectJob;

    fn records(&self) -> impl Iterator<Item = &JobRecord<ProjectJob>> {
        self.jobs()
    }

    fn scheduled(job: Job<ProjectJob>) -> ProjectEvent {
        ProjectEvent::JobScheduled(job)
    }

    fn job_event(job: JobId, event: JobEvent) -> ProjectEvent {
        ProjectEvent::Job { job, event }
    }
}

impl JobHost for DaemonState {
    type Does = DaemonJob;

    fn records(&self) -> impl Iterator<Item = &JobRecord<DaemonJob>> {
        self.jobs()
    }

    fn scheduled(job: Job<DaemonJob>) -> DaemonEvent {
        DaemonEvent::JobScheduled(job)
    }

    fn job_event(job: JobId, event: JobEvent) -> DaemonEvent {
        DaemonEvent::Job { job, event }
    }
}
