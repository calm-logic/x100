use x100_core::job::Interval;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A repeat shorter than the daemon's `minimum_repeat` setting. Nothing was written.
    #[error("a repeat of {:?} is shorter than the minimum of {:?}", .asked.get(), .minimum.get())]
    RepeatTooShort { asked: Interval, minimum: Interval },
    /// The log refused the event or could not store it. `Store(Rejected)` on a cancel means the
    /// job is unknown or has already ended.
    #[error(transparent)]
    Store(#[from] x100_store::Error),
}
