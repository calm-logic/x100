//! Process-wide counters, so the stats bar and the stderr log read one source.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub static ROOT_RENDERS: AtomicU64 = AtomicU64::new(0);
pub static LIST_RENDERS: AtomicU64 = AtomicU64::new(0);
pub static ROW_RENDERS: AtomicU64 = AtomicU64::new(0);
pub static ROW_UPDATES: AtomicU64 = AtomicU64::new(0);
pub static MARKDOWN_RENDERS: AtomicU64 = AtomicU64::new(0);
pub static TERM_RENDERS: AtomicU64 = AtomicU64::new(0);
pub static TERM_WAKEUPS: AtomicU64 = AtomicU64::new(0);
pub static FRAMES: AtomicU64 = AtomicU64::new(0);
/// Renders of a row that had not been updated since its previous render. Only a full
/// window refresh (first frames, a resize) should cause these, in blocks of 100.
pub static ROW_UNPROMPTED: AtomicU64 = AtomicU64::new(0);

pub fn bump(c: &AtomicU64) {
    c.fetch_add(1, Relaxed);
}

pub fn get(c: &AtomicU64) -> u64 {
    c.load(Relaxed)
}

pub fn line() -> String {
    format!(
        "frames={} root={} list={} row_renders={} row_updates={} row_unprompted={} markdown={} term_renders={} term_wakeups={}",
        get(&FRAMES),
        get(&ROOT_RENDERS),
        get(&LIST_RENDERS),
        get(&ROW_RENDERS),
        get(&ROW_UPDATES),
        get(&ROW_UNPROMPTED),
        get(&MARKDOWN_RENDERS),
        get(&TERM_RENDERS),
        get(&TERM_WAKEUPS),
    )
}
