//! Start packs as drivers were given them, for `drivers.start_pack`.
//!
//! Core has nowhere to record one: no event or transcript item carries it. So each is kept as a
//! file in the project's `.x100/packs/`, named by its driver. The right home is a field of the
//! driver's record or a transcript item of its own, which is a change to `x100-core`.

use std::path::PathBuf;

use x100_core::id::DriverId;
use x100_store::ProjectStore;

fn path(store: &ProjectStore, driver: DriverId) -> PathBuf {
    store
        .git
        .project_dir()
        .join(".x100")
        .join("packs")
        .join(format!("{driver}.md"))
}

pub(crate) fn keep(store: &ProjectStore, driver: DriverId, text: &str) -> std::io::Result<()> {
    let path = path(store, driver);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

pub(crate) fn read(store: &ProjectStore, driver: DriverId) -> Option<String> {
    std::fs::read_to_string(path(store, driver)).ok()
}
