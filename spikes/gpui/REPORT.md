# S1. GPUI spike report

Run on 2026-11-11 under WSL2 only (Ubuntu 22.04, WSLg, Mesa 23.2.1, an RTX 4090 on the Windows side), Rust
1.99.1, release build. Written by the orchestrator from the spike's findings and its own re-run.

## Verdict

Under WSL, GPUI can carry the app, with these limits.

1. **The app must use X11 under WSLg, not Wayland.** Over Wayland, EGL offers no config and GPUI falls back to
   llvmpipe, a software renderer, which the spike measured at 17 to 20 cores. Over X11, GL reaches the GPU
   through Mesa's D3D12 driver. The app can choose X11 in code: `gpui_platform::linux(WindowingModes::X11)`.
2. **About 31 to 34 frames a second under XWayland**, with the process at a quarter of one core and the display
   at 60 Hz. The cause was not found.
3. **About 25% of one core and 210 to 218 MiB resident** with 3 list rows changing every 20 ms and a terminal
   printing about 50 lines a second. Idle is about 2%.
4. **x100 writes its own terminal and Markdown views.** Zed's `terminal`, `terminal_view`, `markdown`, `ui` and
   `theme` crates are GPL-3.0-or-later and cannot be used. The spike's views are on `alacritty_terminal`
   (Apache-2.0) and `pulldown-cmark` (MIT).
5. **GPUI comes from a pinned git revision of Zed** (`3e23a0c48201a9cc34ad4a3a4225910247de41f4`), crates `gpui`
   and `gpui_platform`, both Apache-2.0. The crates.io `gpui` 0.2.2 renders through Vulkan only, which under WSL
   means llvmpipe, and it no longer builds against current `libc`.

## What the orchestrator observed itself

A 20 second run with `SPIKE_WINDOWING=x11`: the log shows `Selected GPU ... D3D12 (NVIDIA GeForce RTX 4090)
(Gl)`. Final counters: `frames=642 row_renders=2014 row_updates=1824 row_unprompted=100`, so about 32 frames a
second, and no row rendered without a change after the one full refresh at start-up. 25% of one core over ten
seconds from `/proc`, 218 MiB resident, exit 0. `screenshot.png` (taken by the spike with `xwd`) shows the
list, the Markdown document with its table and code block, and the coloured terminal.

## Rules for M15

- A view that holds often-changing children must not itself be a cached view. A dirty cached view re-renders
  its whole subtree and ignores its children's caches. Cache the rows, not the list.
- Paint terminal cells directly. One text run per line drifts on wide characters, and re-laying out the grid on
  every wakeup costs about a quarter of a core for one busy terminal.
- A git dependency does not inherit Zed's `[patch.crates-io]`. Copy the patches GPUI's graph uses whenever the
  pin moves.
- Build needs `libxkbcommon-x11-dev`. It is missing here, and `build.rs` works around it with a symlink.
- WSLg's Wayland socket is at `/mnt/wslg/runtime-dir/wayland-0`, not under `XDG_RUNTIME_DIR`. Do not panic
  when a display connection fails.
- `gpui-component` (Apache-2.0) has ready widgets but ties the GPUI version to a third party. Not tried.
- Add a `cargo deny` licence check. The spike read declared licences only.

## Not verified

macOS. Native Linux. The window as seen on the real screen, since the Windows session was locked, so flicker
and smoothness were not judged by eye. Any input: clicks, keys, scrolling, link opening. More than one
terminal at once. Long runs. Images.
