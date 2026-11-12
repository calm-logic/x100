//! Choosing the display protocol, and failing with a message rather than a panic when there is
//! no display to reach.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Global};

pub fn under_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|r| r.to_ascii_lowercase().contains("microsoft"))
}

/// Whether the desktop has a file chooser the app can open. Under WSL it has none: GPUI's call to
/// open one never returns, so files are added by typing their path instead. Set once by
/// [`crate::init`], and by a test for the desktop it stands for.
pub struct FileChooser(pub bool);

impl Global for FileChooser {}

impl FileChooser {
    /// This desktop's.
    pub fn here() -> Self {
        Self(!under_wsl())
    }
}

pub fn has_file_chooser(cx: &App) -> bool {
    cx.try_global::<FileChooser>().is_none_or(|c| c.0)
}

/// A path with a leading `~` made the home directory.
pub fn expand_home(path: &str) -> String {
    match (path.strip_prefix('~'), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{home}{rest}")
        }
        _ => path.to_owned(),
    }
}

/// Set once the window is open. Until then a panic is GPUI failing to reach the display.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Called when the window is open: from then on a panic is a panic.
pub fn started() {
    STARTED.store(true, Ordering::SeqCst);
}

/// Runs the app until it quits. Failing to reach the display is an error with GPUI's reason,
/// not a panic: GPUI panics when the display named in the environment does not answer.
pub fn run(start: impl FnOnce(&mut App) + 'static) -> Result<(), String> {
    use std::panic;
    use std::sync::{Arc, Mutex};

    let message = Arc::new(Mutex::new(None::<String>));
    let previous: Arc<dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync> =
        Arc::from(panic::take_hook());
    panic::set_hook({
        let message = Arc::clone(&message);
        let previous = Arc::clone(&previous);
        Box::new(move |info| {
            if STARTED.load(Ordering::SeqCst) {
                return previous(info);
            }
            let payload = info.payload();
            let text = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()));
            *message.lock().unwrap_or_else(|e| e.into_inner()) = text;
        })
    });
    let ran = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        application().map(|app| app.run(start))
    }));
    match ran {
        Ok(result) => result,
        Err(payload) if STARTED.load(Ordering::SeqCst) => panic::resume_unwind(payload),
        Err(_) => {
            let text = message.lock().unwrap_or_else(|e| e.into_inner()).take();
            Err(format!(
                "could not connect to the display: {}",
                text.unwrap_or_else(|| "no reason given".into())
            ))
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn display_named() -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn application() -> Result<gpui::Application, String> {
    Ok(gpui_platform::application())
}

/// Under WSLg only X11 reaches the GPU: over Wayland GPUI is given a software renderer
/// (spike S1). `X100_WINDOWING=x11` or `wayland` overrides the choice.
#[cfg(target_os = "linux")]
fn modes() -> gpui::WindowingModes {
    use gpui::WindowingModes;
    match std::env::var("X100_WINDOWING").as_deref() {
        Ok("x11") => WindowingModes::X11,
        Ok("wayland") => WindowingModes::WAYLAND,
        _ if under_wsl() => WindowingModes::X11,
        _ => WindowingModes::X11 | WindowingModes::WAYLAND,
    }
}

/// Whether the environment names a display the app may use, checked before anything starts.
#[cfg(target_os = "linux")]
pub fn display_named() -> Result<(), String> {
    use gpui::WindowingModes;
    let modes = modes();
    let named = |var: &str| std::env::var_os(var).is_some_and(|v| !v.is_empty());
    if (modes.contains(WindowingModes::X11) && named("DISPLAY"))
        || (modes.contains(WindowingModes::WAYLAND) && named("WAYLAND_DISPLAY"))
    {
        Ok(())
    } else if modes == WindowingModes::X11 {
        Err("no display: DISPLAY is not set".into())
    } else {
        Err("no display: neither DISPLAY nor WAYLAND_DISPLAY is set".into())
    }
}

#[cfg(target_os = "linux")]
fn application() -> Result<gpui::Application, String> {
    display_named()?;
    Ok(gpui_platform::linux(modes()))
}
