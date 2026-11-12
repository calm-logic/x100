//! The store blocks on SQLite and git, so the async turn loop reaches it through the blocking pool.

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("a blocking task panicked")
}
