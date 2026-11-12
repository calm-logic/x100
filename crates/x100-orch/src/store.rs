//! The store blocks on SQLite and git, so the async turn loop reaches it through the blocking pool.

use x100_core::event::{Envelope, ProjectEvent};
use x100_core::fold::ProjectState;
use x100_core::party::Actor;
use x100_store::ProjectStore;

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("a blocking task panicked")
}

pub(crate) async fn append(
    store: &ProjectStore,
    by: Actor,
    event: ProjectEvent,
) -> x100_store::Result<Envelope<ProjectEvent>> {
    let log = store.log.clone();
    blocking(move || log.append(by, event)).await
}

/// Reads the folded state under the log's lock. The lock is also held while an append writes, so
/// this goes through the blocking pool too.
pub(crate) async fn read<T: Send + 'static>(
    store: &ProjectStore,
    f: impl FnOnce(&ProjectState) -> T + Send + 'static,
) -> T {
    let log = store.log.clone();
    blocking(move || log.with_state(f)).await
}
