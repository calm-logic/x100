// This machine has the runtime libxkbcommon-x11.so.0 but not the libxkbcommon-x11-dev package
// that provides the unversioned name the linker asks for. Point the linker at a symlink of our
// own instead of changing the system. `apt install libxkbcommon-x11-dev` makes this a no-op.
use std::path::Path;

fn main() {
    let dev = Path::new("/usr/lib/x86_64-linux-gnu/libxkbcommon-x11.so");
    let runtime = Path::new("/usr/lib/x86_64-linux-gnu/libxkbcommon-x11.so.0");
    if dev.exists() || !runtime.exists() {
        return;
    }
    let out = std::env::var("OUT_DIR").unwrap();
    let link = Path::new(&out).join("libxkbcommon-x11.so");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(runtime, &link).unwrap();
    println!("cargo:rustc-link-search=native={out}");
}
