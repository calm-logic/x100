# x100 build status

How the orchestrator, or a session that replaces it, picks the build up. Read this and `git log` first.

## Accepted milestones

- **M0, workspace and CI.** Check run by the orchestrator in a fresh copy of the tree on rustc 1.99.1:
  `cargo build --workspace` exit 0, `cargo test --workspace` 29 result lines all ok with 0 tests,
  `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` exit 0. Binaries built:
  `x100`, `x100-app`, `x100d`. The CI file is written and has not run, since nothing is pushed.

## In progress

- S1, GPUI spike. Opus subagent, owns `spikes/gpui`.
- M1, split in two because it is the contract and large. M1a: ids, domain types, events, the six state
  machines, the clock, and the Stateright proofs. M1b, after it: the folds into tab state. Opus subagent, owns
  `crates/x100-core`.

## Waiting on the owner

1. **Pushing.** Asked once M0 is accepted. Until then everything is local.
2. **Real-provider runs.** Needed by M3 (one-file edit and second turn, each harness), M9 (kill test, both
   directions) and M17 (the fifteen conditions). Rough count given before the first run. None made yet.
3. **Credentials in Settings.** Added to `PRODUCT.md` as v1, because condition 6 cannot be met unless the
   builder can add a credential. Say if it should be done another way.
4. **Open questions 1 and 2 in `PRODUCT.md`.** The build uses the recommended answers: the CEO may invent any
   role with a charter, and team changes need the builder's agreement.

## Toolchain, checked 2026-11-11

| Assumed | Found |
|---|---|
| Rust 1.88 is enough | Zed builds GPUI on 1.98.1. Stable is 1.99.1. The workspace pins 1.99.1 in `rust-toolchain.toml`, installed beside 1.88. |
| GPUI is a crate | `gpui` 0.2.2 is on crates.io, last published 2025-10. Zed's repository is far ahead. S1 picks the source. |
| bubblewrap works unprivileged | Yes, 0.6.1. `--unshare-all` runs as the user, and `--unshare-net` blocks direct egress. |
| Docker works unprivileged | Yes, Docker Desktop 29.9.2 through WSL. `hello-world` ran. Podman is also installed. |
| Claude Code runs headless | 2.1.322. `-p` with `--output-format stream-json`, `--input-format stream-json` for further turns, `--mcp-config` with `--strict-mcp-config`, `--permission-prompt-tool`, `--effort`, `--model`. M3 confirms the behaviour. |
| Codex runs headless | 0.167.1. `codex exec --json`, `codex exec resume` for a further turn, `codex mcp add`, and an experimental `app-server`. M3 confirms the behaviour, further turns in one process above all. |
| An OS keychain | None here: no secret service under this WSL. The vault falls back to a key file. See decisions. |
| A GPU window under WSL | WSLg is up (Wayland and X). Vulkan drivers are present, untested. S1 finds out. |
| Node 22 | The shell's default is Node 18. Node is not needed by the build. |

## Decisions that changed a document

All in **proposed** parts, made in the review before M0. Reasons in brief.

- **Six state machines, not five.** The plan said five and the architecture lists six.
- **Task and Trial machines.** A Task line had slipped under Trial. Task now says what `report`, accepting and a
  failed merge do. Trial gained Stopped, which M14's check needs.
- **Driver machine.** Handing off is entered from Warm, since the prose ends the turn first. "Live" is defined
  as any state that is not final. A restart fails mid-turn drivers and ends warm ones, and M5's check says the
  same.
- **Action machine.** Gained Carried out and Failed, so "a held action never runs before it is approved" can be
  stated.
- **Budget invariant.** "Spend never exceeds the budget" cannot hold, since cost is known after a turn. Now: no
  turn starts once its budget is reached. The daemon, not a CEO turn, posts the budget notices.
- **Missing tools.** `review` (accept or send back), `set_budget`, `set_preview`, and the CEO's `read_log`,
  `read_memories` and `propose_change`. `form_team` is defined, and later team changes are always held.
- **Merging.** Accepting a task merges the assignee's branch into the delegator's. The CEO's report merges into
  main. This is what Promoting means.
- **The builder's tasks.** The brief and Reflect now are tasks delegated by the builder. Other messages are
  messages.
- **Handoff skip rule.** Skipping every harness that failed on the task could leave nothing to try before the
  three-failure limit. The skip is now dropped when it leaves nothing.
- **Daemon log.** Cool-offs, machines, harnesses and credentials belong to no project, so they have a log of
  their own. One subscription covers it and every project. Transcripts have their own subscription.
- **One-off calls.** The gate's classifier and the lab's judge need a model without an agent. Defined, with
  where each is charged.
- **Provider logins and TLS.** A harness inside a sandbox reaches its provider through the proxy with a vault
  reference. The proxy ends TLS with its own authority. Without a keychain the vault key is a file.
- **Sandbox endpoints.** A sandbox has no network. The backend maps in the proxy and the driver's tool address.
  Before M10 a sandbox is a plain worktree.
- **Layout.** `.x100/` holds the store, files and worktrees and is excluded from the project's repository.
  Binaries are `x100d`, `x100` and `x100-app`. Crate dependencies are stated.
- **Wiring.** The plan now says who wires a crate into `x100-orch` and `x100d`. M5 owns the turn loop in
  `x100-orch` so its check needs no throwaway code.
- **"Still missing" list.** Its four items are settled into the design and the list is removed.
- **M16 to M17** for where thresholds are tuned.

## Could not be verified, and why

- Anything on macOS. There is no Mac, and nothing is pushed, so hosted CI has not run.
