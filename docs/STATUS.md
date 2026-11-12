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
- **M2, store and protocol.** The orchestrator's run with `-p x100-store -p x100-proto` (M3 was mid-build in
  its own crate): store 25 tests and proto 20 tests plus 3 doctests pass, clippy and fmt clean. Replay of the
  670-event log gives identical state. Six late subscribers under four writer threads each get a snapshot,
  then every later event once. A helper process killed with SIGKILL 40 times loses no reported event. A
  checkpoint of a worktree with staged, unstaged, untracked, deleted and executable files restores the same
  files without moving the branch or index. Git is the command line, SQLite is rusqlite bundled. 37 protocol
  methods, each checked against `crates/x100-proto/PROTOCOL.md` by a test.
- **M4, jobs and the clock.** 19 tests. On the fake clock a one-off job fires once at its time, a repeat fires
  at each interval, a cancelled job never fires, and a job due across a restart fires once. Property tests
  over random schedules, cancels, restarts and clock moves hold the job invariants against a real store.
- **M3, drivers and harnesses, except its paid check.** 31 tests. The fake harness replays a script
  deterministically, poses as two providers, and is reported Failed with the reason when told to crash, and
  when told to stall only once the timeout passes on the fake clock. The real Claude Code 2.1.322 and Codex
  0.167.1 binaries, against a fake provider on localhost: each edits a file, reports usage, keeps context on a
  second turn, compacts, asks before a command, is stopped with nothing left running, and reports 401, 429,
  500 and 529 as the right provider failure. **Not run:** "when asked to, each real harness completes a
  one-file edit" against a real provider. The tests exist and are ignored by default.
- **Whole workspace after M3 and M4**, the orchestrator's run, twice: 197 tests pass, 0 fail. Build, clippy
  with warnings denied and fmt are clean. No harness process was left running.
- **M5, daemon and client library.** The orchestrator's run, three times: 219 tests pass, 0 fail, clippy and
  fmt clean, no daemon or harness process left. With the daemon as a real child process and the client library
  the only way in: a brief to the CEO is answered as events on the fake harness. Killing the daemon with
  SIGKILL while one driver is mid-turn and another is warm, then restarting: the first is Failed with
  DaemonRestarted, the second Ended, none live, the leftover process group is gone, and new messages get
  replies on new drivers. Also: two clients agree, a reconnect resumes with no gap or repeat, closing every
  client stops nothing, a second message reuses the warm driver. 21 protocol methods are real. The other 16
  answer "not available yet" with their milestone, from one table a test keeps complete.
- **M6a, the organisation's behaviour.** The orchestrator's run, three times: 246 tests pass, 0 fail, clippy
  and fmt clean, no process left. The plan's M6 check passes on fake-harness child processes, real stores and
  git, the orchestrator in-process, the fake clock: a BRD written, three leads formed, workers under two, a
  worker with its own worker, a lead to lead message, a question to the builder answered while the CEO kept
  working, tasks through every status, a final report, and the workers' files on the project's main branch.
  No lead's driver starts before its first inbox item. A worker waits on its own job and is woken on a new
  driver whose start pack holds its progress note. A lead resumes on its worker's report. A repeating job
  delegates a fresh task at each firing. A wait past its deadline wakes the agent and says so. An item given
  to a driver stopped mid-turn reaches the next. A crashed driver's task is finished by the next driver, and
  three crashes or three turns without progress report the task failed. The run's event kinds are identical
  over ten runs. **Limit:** scripts reach ids through a test adapter, see in progress. Real harnesses have no
  tools until M6b.
- **M6b, the tools over MCP, which completes M6.** The orchestrator's run, twice, with the in-progress
  `x100-app` excluded: 270 tests pass, 0 fail, clippy and fmt clean. Against the fake provider, the real Claude
  Code and Codex binaries, each started by the orchestrator as the CEO's driver, call `write_spec` and `report`
  over MCP, and the log shows the BRD and the report. Each driver has its own address and token. A call is
  accepted only from the agent's current live driver, judged on the folded log under the writer's lock: a
  replaced driver's call and a made-up token are refused and change nothing. A CEO-only tool is not in a lead's
  list, and later milestones' tools are not listed. Every tool's schema is held to its serde form by tests.
  Shutdown now waits for every driver task, which fixes the race CI found on macOS, with a test that fails
  without the fix. The MCP server is `rmcp` 3.9.0.
- **The terminal stream in the contract.** Core has the chunk type and `Driver.view` (terminal or headless),
  the store records and follows a terminal per driver, and the protocol has `terminal.read`,
  `terminal.subscribe` and the `terminal.chunk` notice. Served from M3T on.

## In progress

- M15a, the desktop app's shell, chat and Team tab. Opus subagent, owns `crates/x100-app` and
  `crates/x100-client`. M15b follows with the agent's live terminal view.
- M3T, terminal drivers on local logins. Opus subagent, owns `crates/x100-drivers`.
- M8, router and budgets. Opus subagent, owns `crates/x100-router`.
- Owed when `x100-app` joins the workspace: GPUI turns on `serde_json`'s `preserve_order` for the whole build,
  and two tests that compare exact JSON text then fail (`x100-drivers` `the_fake_replays_a_script_deterministically`,
  `x100-proto` `the_worked_example_is_a_valid_conversation`). They must compare values, not text.

## Waiting on the owner

Nothing blocks. Open questions 1 and 2 in `PRODUCT.md` are built on their recommended answers: the CEO may
invent any role with a charter, and team changes need the builder's agreement.

## The current goal

Set by the owner on 2026-11-12: carry on without stopping until the GPUI desktop app builds, matches the
demo, and an end-to-end test works in full on a simple project, such as "create a todo app as a proof of
concept". The orchestrator reads this as the say-so for real runs on the builder's local logins where that
goal needs them (M3T's check and the end-to-end run), on the cheapest models, kept few and counted here.

Order to get there: the contract fixes, M6b, then M3T, M7, M8 and M15 side by side on their own crates, then
M16 and the end-to-end run. M15 starts beside M7 rather than after it, to reach the goal sooner. M9 to M14
follow, and none of them is needed for the first end-to-end run.

## Owner's decisions during the build

- **2026-11-12. Push to `origin`.** Allowed. Pushed after each accepted milestone from now on.
- **2026-11-12. Local subscriptions.** x100 must run the builder's local agents on their own subscriptions,
  as Orca, herdr and T3 Code do, each in a dedicated terminal reachable from the Team tab by drilldown. This
  replaced "API keys only". It added milestone M3T and the two ways of running a harness in ARCHITECTURE.
- **2026-11-12. Cap per login.** At most 4 terminal drivers at once on one login, by default.
- **2026-11-12. The login inside the sandbox** may be writable. No objection.
- **2026-11-12. An agent's terminal** is for watching only, for now.

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

- **Logins.** Headless drivers take API keys only: Claude Code's isolated mode reads no subscription token and
  Codex's ChatGPT login is OAuth with refresh. Subscriptions are served by terminal drivers, see the owner's
  decisions.
- **Background processes.** A harness kills its own on stop, so `start_process` and `stop_process` are daemon
  tools and the sandbox owns what they start.
- **Sandbox endpoints.** Neither harness takes a Unix socket for MCP or its proxy, so bubblewrap needs a
  loopback forwarder inside the sandbox.
- **Permission hooks pause the turn.** The daemon answers a held command at once with a denial.
- **Cost.** Codex reports tokens only. Prices gained cache-write lifetimes and a long-prompt tier.
- **Warm period** follows each model's cache lifetime, not one setting.
- **Retries and compaction.** Harnesses' own retries are set low and their automatic compaction is off.
- **Codex** is driven through `codex app-server`, not `codex exec`.

From M6a:

- **Contract fixes after M6a**, verified by the orchestrator (255 tests pass twice, all 7 models proved and 10
  broken variants refuted). Scripts read ids from labelled lines of the turn input and start pack
  (`{{input:Task}}`, `{{pack:Your id}}`), and the test adapter is gone. The Fernleaf script now runs through
  the real daemon as a child process. For that the daemon has a fake-clock mode (`--fake-clock`, only with a
  scripts directory) that runs one turn at a time, and a protocol method `daemon.set_clock` to move it. The
  start pack is the first transcript entry of each driver. An agent may hold several open waits
  (`StillWaiting` moves it back to Waiting after a turn). The store commits a worktree to its branch. The
  scheduler can be woken.

- **Seams left for later milestones** in `x100-orch`: `DriverChooser` (M8), `Sandbox::wrap` (M10), `Gate::rule`
  (M12). Tool calls and merges both pass the gate.
- **An existing agent's branch is refreshed** just before the turn that is given the task, never mid-turn.
- **Three failures in a row** are counted in memory and lost on a restart. M9 makes that durable.
- **Any agent may message any other.** Only the CEO sets a new worker's range. `form_team` is refused once a
  lead exists, since later team changes are held actions.
- **The flaky macOS test** now waits for both clients to reach the same sequence numbers before comparing.

From M5:

- **Contract fixes after M5**, verified by the orchestrator (224 tests pass twice): the store appends several
  events as one unit, so a message and its delivery, or a turn's checkpoint, reply and end, are written
  together or not at all. The store's error is small. A driver's pid is recorded at spawn, so one killed with
  the daemon during its handshake is reaped on restart. A refused turn-end batch now fails the turn and its
  items go to the next driver.

- **Protocol limits.** A line is at most 16 MiB. A subscriber whose queue has no room for 10 seconds is cut
  off with `subscription.closed` and resumes from its cursor. A project created later reaches every
  subscriber from its first event.
- **Two bugs the subagent found under load and fixed**: the client dropped a notice read in the same pass as
  its subscribe result, and a lag test that did not guarantee the cut-off.
- **Leftover drivers.** Each driver's pid and start time are written down. On start the daemon kills every
  recorded process group whose leader is the same process or is gone.
- **Open for M6:** `chat.send` opens no task yet. Stall detection in the daemon runs on the real clock only.

From M2 to M4:

- **The transcript entry type** was defined in both the store and the protocol. It now lives once in
  `x100-core`, with a new item for a tool's live output. 198 tests pass after the move.

- **A flaky test in M2, fixed by the orchestrator.** Under load the kill test's helper was killed before its
  first commit, and the test demanded one. It was a wrong assertion, not lost data: the message was "reported
  event 0, stored 0". The first round now waits for one commit. It then passed six runs with every core busy.
- **Codex approvals** are asked for on the thread, not in its config file, and a denial carries no reason.
- **The minimum repeat** is passed to the scheduler by the daemon, since a project's log does not hold it.

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

- macOS by hosted CI only. First run, 2026-11-12, of commit 2c3a3e3: Linux and macOS both green, which covers
  the store's git plumbing, the kill tests and the daemon restart test there. The next push failed on macOS in
  one test, `two_clients_see_the_same_events`, a race in the test between two followers. Being fixed with M6a.
  The sandbox, the keychain and GPUI are not covered by CI.
- Durability against power loss. Only a killed process was tested.
- A crash between deciding to fire a job and appending it. The append is the commit, so it follows from the
  store's kill test, but no test kills the scheduler there.
- This machine's wall clock steps: M4 measured it 16 seconds off the monotonic clock within 30 seconds under
  WSL2. Jobs are timed on the wall clock, since they must survive a restart, so here a job can fire that much
  early or late. The runner re-reads the clock at least every 30 seconds.
- S1 on macOS and native Linux. Under WSL nobody saw the window on the real screen (the Windows session was
  locked) and no input was sent to it, so flicker by eye, clicks, keys and scrolling are unchecked.
