# Sandbox measurements

Taken 2026-11-12 on the build machine: Windows with WSL2 (Ubuntu 22.04, kernel 5.15.167.4), 32 logical
CPUs, bubblewrap 0.6.1, util-linux 2.37.2, Docker Desktop 29.9.2 (engine in WSL2), Podman 3.4.2 rootless. Other
agents were building other crates at the same time: the load average was 8 to 12 throughout, so every number
here carries that noise.

Rerun with `cargo test -p x100-sandbox --test measure -- --ignored --nocapture --test-threads 1`.
`X100_MEASURE=bubblewrap|container` picks a backend, `X100_CONTAINER_RUNTIME=podman` picks Podman, and
`X100_MEASURE_ROUNDS` sets the number of build rounds (2 by default).

## Creating a sandbox and starting a command

`create_and_start`: ten creates and destroys of an empty sandbox, then thirty runs of `/bin/sh -c true`
through the sandbox's wrapper, each spawned and waited on as a driver is, against the same command run
directly. Medians.

| Backend | Create | Destroy | `sh -c true` inside | Outside |
|---|---|---|---|---|
| bubblewrap | 19.7 ms | 0.5 ms | 6.8 ms | 0.4 ms |
| Docker | 179 to 199 ms | 78 to 103 ms | 79 to 80 ms | 0.5 ms |
| Podman | 225 ms | 245 ms | 326 ms | 0.4 ms |

Create is timed until the helper inside has said it is ready, so the sandbox can run commands and its
endpoints are mapped. Starting a command costs one `nsenter` and one `bwrap` under bubblewrap, and one
`<runtime> exec` plus the helper on each side under a container. A driver starts once per turn or less, so
even Podman's third of a second does not slow an agent's work; a build that runs many short commands runs them
inside one driver's process, not through the wrapper.

## A full build of this workspace

`workspace_build`: the workspace as committed at HEAD (da0d87a), unpacked with `git archive`, built with
`cargo build --offline --workspace --exclude x100-app`. **The desktop app is left out**: it pulls in GPUI
and is the slow part of a full build. Every build starts from an empty target directory, with the dependency
sources already downloaded (a warm `~/.cargo/registry` and `~/.cargo/git`), so each compiles all 149 crates
from source. Two rounds, the second in the opposite order, so a machine growing busier or quieter favours no
side. Inside a sandbox the toolchain is mounted read-only: `~/.cargo/bin`, `~/.cargo/registry`,
`~/.cargo/git` and `~/.rustup`, with `CARGO_HOME` and `RUSTUP_HOME` set to them.

| Where | Round 1 | Round 2 | Mean | Against outside |
|---|---|---|---|---|
| Outside any sandbox | 22.9 s | 20.6 s | 21.7 s | |
| bubblewrap | 20.0 s | 20.2 s | 20.1 s | -7.6% |
| Docker | 21.5 s | 20.4 s | 21.0 s | -3.5% |

The differences are inside the spread of the unsandboxed runs alone (20.6 to 22.9 s), so the measured overhead
of either backend on a build is zero within this machine's noise. A sandbox runs compiled code natively, with
no layer over the file system for the worktree or the toolchain, so none was expected. An earlier single
round gave outside 22.5 s, bubblewrap 23.1 s, Docker 25.5 s: the same noise, in the other direction. Podman's
build was not measured.
