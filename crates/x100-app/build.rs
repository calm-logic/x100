// The linker asks for `libxkbcommon-x11.so`, which only the `-dev` package provides. Where just the
// runtime library is installed (this project's WSL machine), point the linker at a symlink of our
// own instead of changing the system. Wherever the `-dev` package is installed this does nothing.
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dir = Path::new("/usr/lib/x86_64-linux-gnu");
    let (dev, runtime) = (
        dir.join("libxkbcommon-x11.so"),
        dir.join("libxkbcommon-x11.so.0"),
    );
    if dev.exists() || !runtime.exists() {
        return;
    }
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let link = Path::new(&out).join("libxkbcommon-x11.so");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&runtime, &link).expect("a symlink in OUT_DIR");
    println!("cargo:rustc-link-search=native={out}");
}
