# x100 build status

How the orchestrator, or a session that replaces it, picks the build up. Read this and `git log` first.

## Accepted milestones

- **M0, workspace and CI.** Check run by the orchestrator in a fresh copy of the tree on rustc 1.99.1:
  `cargo build --workspace` exit 0, `cargo test --workspace` 29 result lines all ok with 0 tests,
  `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --check` exit 0. Binaries built:
  `x100`, `x100-app`, `x100d`. The CI file is written and has not run, since nothing is pushed.

- **S1, GPUI spike.** Verdict: GPUI can carry the app under WSL, with limits (X11 only under WSLg, about 32
  frames a second, own terminal and Markdown views because Zed's are GPL, GPUI from a pinned git revision).
  The orchestrator's own 20 second run on X11: GPU adapter selected, 642 frames, 2014 row renders for 1824
  updates with 100 unprompted (the start-up refresh), 25% of one core, 218 MiB, exit 0. Report and rules for
  M15 in `spikes/gpui/REPORT.md`. The spike stays in the tree until M15 has taken what it needs.
- **M1a, core types, events, machines and proofs** (M1 is split in two: it is the contract and large). The
  orchestrator's run: 67 tests pass, clippy and fmt clean. Stateright 0.31.0 proves seven models, the largest
  (takeover) over 431,299 states, and ten deliberately broken transitions are each refuted with a
  counterexample. 86 accepted transitions and 204 rejected pairs are table-tested. Invariants this crate
  cannot state, and who must check them: fencing (M9), allow-lists, cool-offs and caps (M8), waits closed by
  the orchestrator (M6), jobs on the real clock (M4), merges only on a committed trial (M13, M14), lab
  isolation (M10, M11, M14).
- **M1b, the folds, which completes M1.** The orchestrator's run: 99 tests pass, clippy and fmt clean. A
  670-event scenario modelled on the Fernleaf demo folds into the state every tab reads, with assertions per
  tab. Property tests: a snapshot sent through JSON at any split point plus the later events equals folding
  the whole log. A rejected event is an error naming the record. Stateright still proves all seven models
  (takeover now 288,835 states after the agent machine change) and refutes all ten broken variants.
  `x100-core` is now the contract: only the orchestrator changes it. Later crates get the scenario builder
  through the `testing` feature.

## In progress

- M2, store and protocol. Sonnet subagent, owns `crates/x100-store` and `crates/x100-proto`.
- M3, drivers and harnesses. Opus subagent, owns `crates/x100-drivers`. Real harnesses are tested against a
  local fake provider. The paid runs wait for the owner.

## Waiting on the owner

1. **Pushing.** Asked once M0 is accepted. Until then everything is local.
2. **Real-provider runs.** None made yet. M3 needs about six short runs per harness on the cheapest models
   (Haiku 5.5, gpt-6-luna at low effort), each under about a cent: roughly 25 cents in all with retries. They
   need an Anthropic API key and an OpenAI API key in the environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`).
   The orchestrator does not read key files. M9 and M17 need more, counted when they come.
5. **Subscription logins.** v1 supports provider API keys only, see decisions. Say if a Claude or ChatGPT
   subscription must work in v1.
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

From M3's first task, the research in `docs/HARNESSES.md`, where it contradicted the architecture:

- **Logins.** API keys only in v1. Claude Code's isolated mode reads no subscription token, Anthropic's terms
  restrict third-party use of claude.ai login, and Codex's ChatGPT login is OAuth with refresh.
- **Background processes.** A harness kills its own on stop, so `start_process` and `stop_process` are daemon
  tools and the sandbox owns what they start.
- **Sandbox endpoints.** Neither harness takes a Unix socket for MCP or its proxy, so bubblewrap needs a
  loopback forwarder inside the sandbox.
- **Permission hooks pause the turn.** The daemon answers a held command at once with a denial.
- **Cost.** Codex reports tokens only. Prices gained cache-write lifetimes and a long-prompt tier.
- **Warm period** follows each model's cache lifetime, not one setting.
- **Retries and compaction.** Harnesses' own retries are set low and their automatic compaction is off.
- **Codex** is driven through `codex app-server`, not `codex exec`.

From M1b:

- **The state holds no log.** The subagent kept every event in the project state, so a snapshot grew with the
  project. The orchestrator removed it. The Log tab pages events from the store.
- **Defaults** for the daemon's settings: retire at 70% context, after 10 tasks or 4 hours. Stall 5 minutes.
  Wait deadline 24 hours. Minimum repeat 5 minutes. Start pack half the window. Last handoff turn 3 minutes.
  Three turns that neither report nor wait. Cool-off 10 minutes. Tuned in M17.
- **Open for M6:** whether the brief is recorded as both a message and a task, as the scenario does.

From M1a, where the documents were silent:

- **Agent machine.** Planned to Retired, for a planned lead a team change removes. Waiting to Idle when the
  wait for a driver ends in a failed task.
- **Task effort.** Changed by the delegator when sending back, or asked for by the assignee in `update_task`.
- **Lead budgets** together never exceed the project's total.
- **Thresholds** are the daemon's settings, in the daemon's log.
- **Repeats** are fixed intervals, not calendar rules.
- **Lab drivers** are not counted in "one driver acts for an agent".

## Could not be verified, and why

- Anything on macOS. There is no Mac, and nothing is pushed, so hosted CI has not run.
- S1 on macOS and native Linux. Under WSL nobody saw the window on the real screen (the Windows session was
  locked) and no input was sent to it, so flicker by eye, clicks, keys and scrolling are unchecked.
