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
- **M15a, the desktop app's shell, chat and Team tab.** The orchestrator's run: 285 tests pass across the
  whole workspace with the app in it, clippy and fmt clean on the app, client and protocol, the app's tests
  twice, no process left. The window tests run with no display against a real daemon on the fake clock with
  the Fernleaf script: a brief with two attachments is typed and sent and the main panel opens; the three
  leads' rows match the client's state; tab dots appear and clear; the drivers list matches; clicking down
  three levels shows each agent's own agents and the trail; an answer to the CEO redraws one row and not the
  others; a new client shows the same project. The subagent also drove the real window under X11 through the
  X server's test extension and captured it (`crates/x100-app/screenshots/`). The orchestrator compared
  `first-run.png` and `team.png` with a render of the page's mock: same three areas, proportions, colours, tab
  bar with dots, CEO orb, lead rows with meter, driver pill and effort pips. **Differs from the mock:** no
  animations, Lato not Manrope, no provider logo in the driver pill, no drawn window bar, a real composer with
  Attach and "By path". **Not verified:** macOS, a real file chooser (WSL has no portal, so "By path" exists),
  the licence check (`cargo-deny` is not installed here, CI runs it).
- **M8, router, budgets and driver reuse.** Verified by the orchestrator in a clean copy of HEAD with only
  M8's three crates added, because M3T's unfinished work in `x100-drivers` stops the main tree building: 291
  tests pass twice, clippy and fmt clean. The choice is a pure function with one rule table: effort in range,
  allowed, not cooling off, room on the machine and login, budget. Cheapest is the price of three input tokens
  to one output token, ties broken deterministically. Table tests cover ranges, allow-lists, cool-offs and
  full machines. In-process with the fake harness posing as two providers: nothing compatible makes the agent
  wait and tells its parent, and a setting change starts it; a lead at its budget starts nothing while others
  carry on; a project at its total starts nothing; a warm compatible driver is reused; a task after the warm
  period or needing more effort gets a new driver; a driver past the context threshold between tasks is
  retired and the next start pack carries the agent's recent tasks; a provider failure cools the model off.
  `agents.set_budget`, `agents.set_effort`, `machines.set_cap` and `harnesses.rescan` are real. **Known
  loss on retiring until M13:** what the agent read in handled messages and in accepted workers' reports.
- **M3T, terminal drivers on local logins.** 53 tests in `x100-drivers`. Against the fake provider, the real
  `claude` and `codex` programs, each in a pseudo-terminal: a one-file edit, a daemon tool call with the
  bearer token, a second turn that kept context, an interrupt, compaction, a held command, each provider error
  as the right failure, and a stop with nothing left running, a background command in its own session
  included. A recording replays to the live screen, across a resize. Chunks arrive during a turn. Codex runs
  as its app-server with the real terminal attached. **On the owner's real logins, by the subagent: 4 model
  turns per harness** (claude-haiku-5-5 and gpt-6-luna, low effort): both started with no login or trust
  screen, made the edit, and kept context on a second turn. Not yet verified by the orchestrator's own real
  run: that comes with the end-to-end run. **Not verified:** macOS (the Keychain with a changed home), a token
  refresh on a real login, the daemon serving the terminal.
- **M15b, the agent's live view, and M16, the remaining tabs.** Verified together in the main tree.
- **Whole workspace after M3T, M15b and M16**, the orchestrator's run, twice: 355 tests pass, 0 fail, 5
  ignored (the real-provider and real-login tests). Build, clippy with warnings denied and fmt clean. No
  process left. The orchestrator looked at `research-handoff.png` (one continuous log across two drivers with
  the takeover marked, the drivers it has had, waits, jobs, inbox) and `budget.png`.
  - M15b: a terminal widget that paints cells itself, replays 46 real recordings cell for cell against a bare
    emulator, never writes back, and scales down to fit a smaller pane. Output in one view redraws only that
    view. Opening Migrations three levels down shows its log. Research's view is one output across two drivers.
    Cancelling a job from the view reaches the daemon. Measured by the subagent: 8% of one core replaying at
    recorded speed, 22 to 34% at maximum speed, 0.3% idle, under load from other builds.
  - M16: Specs, Tasks, Files, Budget, Log, Preview link, Infra line, Settings with Credentials, held actions as
    a strip above the composer, and an agent's memories, skills and trials. Each tab matches the client's state
    after the Fernleaf script. The Log lays out fewer than 60 rows of 20,000 events. Every event has wording,
    checked at compile time.
  - **Rough or unseen:** the held strip, a preview address and the memories view were never seen in a real
    window, since the script produces none. The log wraps mid-word. Facts truncate. A terminal driver has not
    been seen inside the app, since the daemon does not serve terminals yet.
- **Terminal drivers wired through the daemon.** The orchestrator's run, twice: 363 tests pass, 0 fail, 5
  ignored, clippy and fmt clean, no process left. Through the real daemon as a child process, a brief to a
  project that allows only the Claude Code terminal harness (real `claude`, dummy login, fake provider) ends
  with the BRD written and the task reported over MCP, and the same for Codex. `terminal.subscribe` delivers
  chunks during the turn, a late subscriber gets everything from the first chunk then live with none twice,
  and a replay matches `terminal.read`. With the cap at 1 a second agent waits with `LoginFull` (in-process,
  fake harness). A killed daemon's Codex terminal driver, both processes, is gone after restart. In the app,
  with no display, the CEO's view shows the terminal's screen equal to a bare replay. This completes M3T's
  "a client following the terminal sees output as it arrives".
- **The real end-to-end run.** Whole workspace on the orchestrator's run: 365 tests pass, 0 fail, 6 ignored,
  clippy and fmt clean. See "The owner's goal" for the real run itself.

## In progress

Running, each on its own crates:

- **How a project begins** (the owner's change): starting a conversation creates the project, uninitialized.
  The CEO sets its name and intent with a tool when it knows enough, which creates the code directory under
  `~/x100/` and opens the builder's task. A project's own state moves to the daemon's data directory. The
  name-and-directory form is removed. Also in that step: the facts table above an agent's output is deleted,
  and memories, skills and trials are hidden until built. Opus subagent, edits most crates.
- **M10, the sandbox**, as a crate on its own (`crates/x100-sandbox`). Opus subagent.
- **M11, the vault and proxy**, as a crate on its own (`crates/x100-vault`). Opus subagent.
- **M12, the gate**, as a crate on its own (`crates/x100-gate`). Sonnet subagent.
- **Homebox research**, read-only: how its API, keys, projects and deploys work. Opus subagent.

Queued, in this order:

1. **App follow-ups from the owner's review of the recording.** The terminal at the pane's size: the app
   tells the daemon the pane's size and the program redraws itself, so text shows at the normal font size
   (today the whole screen is shrunk to fit). One "+" icon button for adding files, replacing Attach and By
   path (under WSL it opens a path field, since there is no file chooser). An agent's drilldown is the
   terminal filling the view, with the agent's head and label small in the top-left of the row that holds the
   Agents and Drivers switch. An `executive-summary` skill the CEO respects in its messages to the builder
   only, never in its own work or the driver's output. The chat shows only what the CEO sends with `message`
   and its report, not every text of its turn.
2. **Wiring M10, M11 and M12** into the orchestrator and daemon, one after another, each with its
   end-to-end check. With the sandbox in, Claude Code drivers can run commands.
3. **M12I, integrations with Homebox first**, then the two-part end-to-end run.
4. M7, M9, M13, M14, M17.

## Waiting on the owner

Nothing blocks. Open questions 1 and 2 in `PRODUCT.md` are built on their recommended answers: the CEO may
invent any role with a charter, and team changes need the builder's agreement.

## The owner's goal of 2026-11-12: met

"A working GPUI that matches the demo version with integration/e2e test working in full on a simple project."

- **The test:** `X100_REAL_LOGINS=1 cargo test -p x100-app --test real_e2e -- --ignored --nocapture`. The app
  with no display, a real `x100d` on the real clock with default settings, the builder's local logins. It
  creates a project, types "Create a todo app as a proof of concept: one index.html ..." into the first-run
  box, and waits for the CEO's final report. It then checks: the CEO replied in the chat, a BRD exists, the
  task is Done, `index.html` is on main with an input, a list and `localStorage`, the Team tab showed the CEO
  on a terminal driver with recorded chunks, Budget shows spend, the Log has the driver choice and the report,
  nothing is left running, and in headless Chromium a todo can be added and survives a reload.
- **The orchestrator's own run**, on the Codex login only (`claude` taken off the PATH, since the Claude login
  was at 91% of its five-hour window): passed in 60 seconds. One turn on gpt-6-luna at Medium, 21,594 input and
  2,258 output tokens, 0.0042 dollars at list price. The CEO worked alone. `index.html` (5,326 bytes) and the
  BRD are on the project's main branch. No process left.
- **The subagent's runs:** one full pass of the test on Claude Code (claude-haiku-5-5, 21 seconds, one turn),
  two runs of the real window on Codex for the screenshots in `crates/x100-app/screenshots/e2e/`, and one run
  stopped when the Claude login passed 90% of its five-hour window. 4 turns and about 29 model requests in all.
- **Real turns used on the owner's logins so far, everything counted:** Claude Code 6 turns, Codex 7 turns.
- **Not shown by this:** a team. The brief is small, so the CEO worked alone, as rule 4 allows. Leads and
  workers have met only scripted and fake models. The final code was not confirmed end to end on Claude Code,
  since that run was stopped by the usage guard.
- **Rough in the real window:** a terminal driver's screen is scaled down to fit and is hard to read at
  1400 by 900. On Codex the CEO's first chat line arrives only when the turn ends, after the report card.

## Owner's decisions during the build

- **2026-11-12. Push to `origin`.** Allowed. Pushed after each accepted milestone from now on.
- **2026-11-12. Local subscriptions.** x100 must run the builder's local agents on their own subscriptions,
  as Orca, herdr and T3 Code do, each in a dedicated terminal reachable from the Team tab by drilldown. This
  replaced "API keys only". It added milestone M3T and the two ways of running a harness in ARCHITECTURE.
- **2026-11-12. Cap per login.** At most 4 terminal drivers at once on one login, by default.
- **2026-11-12. The login inside the sandbox** may be writable. No objection.
- **2026-11-12. An agent's terminal** is for watching only, for now.
- **2026-11-12. How a project begins.** No form. Starting a conversation creates an uninitialized project,
  and the CEO initializes it with a name and intent once it has context, which may be deferred ("hi").
- **2026-11-12. The agent view.** The terminal must be readable, not shrunk. A drilldown is the terminal
  filling the view, with the head and label in the top-left corner. No drivers, waiting, jobs and inbox table.
  Memories and skills hidden until M13. One "+" button for files.
- **2026-11-12. The CEO's messages to the builder** are an executive summary: the minimum, unless more is
  asked for or likely to be useful. This applies to its messages only, not to the driver's own output.
- **2026-11-12. Integrations.** Homebox is the first supported integration, with a project setting that links
  a project to a Homebox project, and CEO tools for x100's own actions such as that link. OAuth as much as
  possible, a pasted token as a last resort. Homebox uses an API key for now, since it has no OAuth yet. The
  end-to-end run gains a second part: a follow-up edit, then a deploy through Homebox.
- **2026-11-12. Build M11 and M12 now**, with what they depend on (M10). Homebox follows in its place in the
  plan, not ahead of it.

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

From the real end-to-end run:

- **A guide at the head of every start pack** (`x100-orch/src/pack/guide.rs`), one each for the CEO, a lead and
  a worker: answer first, write the BRD before building, a small project needs no team, keep the progress
  note, end every turn with `report` or `wait`, use only ids you were given. Nothing had told a real model how
  x100 works. Both programs then did the brief in one turn.
- **The workspace rules are told up front**, and a refused command reads as an instruction to write the files
  directly, not as an error to retry.
- **Tool schemas no longer carry example ids** a model could copy. "Share done" is a percentage from 0 to 100.
- **The Team tab** has a switch between an agent's own agents and its output, so the CEO's terminal stays
  reachable once it has leads.
- **Spend under a cent** shows as "<$0.01". Driver warnings, the usage-window warning among them, are printed
  by the daemon.
- **Open:** which program the CEO gets is decided by a tie on price between Haiku and luna, broken by harness
  id, so it differs per machine. Claude Code on a subscription wrote the one-hour cache, while the model table
  assumes five minutes for price and warm period.

From the wiring of terminal drivers:

- **Contract.** Harness kinds for the two terminal harnesses. A login on the harness record. The cap per login
  as a daemon setting. `Incompatible::LoginFull`. `RetireReason::Unfit` for a warm driver that no longer fits.
  A deadline routine for an agent's wait for a driver. An event when a machine's harness list changes, with
  the harnesses passed over and why, which is how a missing login shows to a client.
- **Defaults**, in `crates/x100d/src/defaults.rs`: a new project allows each terminal harness's cheapest model
  (claude-haiku-5-5, gpt-6-luna) and no headless model, since a key can only come through the vault. The CEO's
  range is Low to Medium. Budget 50 dollars at list price.
- **What a real driver may do, with no sandbox until M10**, one setting in `x100-orch/src/spec.rs`. Claude Code
  runs in its accept-edits mode: edits inside the worktree go through, and every command and every edit
  outside it is put to the daemon and refused, since the gate is M12. So a Claude Code driver cannot run
  commands at all yet. Codex runs commands in its own sandbox: writes only to the worktree and /tmp, no
  network. **Remaining risk:** both run as the builder and can read anything the builder can, the provider
  login included, and what they read can reach the provider in the model's context.
- **Known gap:** an agent with agents under it shows no output, so the CEO's terminal cannot be reached from
  the Team tab once it has leads.

From M3T:

- **The login is reached without x100 opening it.** Claude Code: the driver's own home has `.claude` as a link
  to the builder's `~/.claude`, with the builder's settings, hooks, plugins, MCP servers and memory files
  switched off. It writes there: a session file and a state backup per run, the prompts in `history.jsonl`, and
  transient entries. Each driver start can push out one of the builder's own five state backups. Codex: a
  `CODEX_HOME` per driver whose `auth.json` is a link to the builder's, which Codex follows when it rewrites
  the login. Nothing else of the builder's `~/.codex` is read or changed.
- **Three outside connections the terminal programs make** that nothing switches off: Claude Code fetches the
  account profile and refreshes its token, and Codex's terminal fetches an announcement at every start.
- **A broken commit.** 012610c (M8) picked up a file rename M3T had staged, so `x100-drivers` did not build at
  that commit. Fixed by the M3T commit. From now on the orchestrator commits with explicit paths only after
  checking the index is empty.

From M15a:

- **GPUI changes JSON key order for the whole build.** It turns on `serde_json`'s `preserve_order`, and cargo
  merges features across a workspace build. Two tests compared exact JSON text and failed. The orchestrator
  made them compare values. A daemon built with the workspace writes keys in a different order from one built
  alone, with the same meaning.
- **Fetching GPUI on this machine** needs `CARGO_NET_GIT_FETCH_WITH_CLI=true` the first time, because of the
  machine's git rule that rewrites https to ssh.
- **The app starts a daemon itself** only for the default socket or with `--daemon`.
- **Seams:** `team/output.rs` `AgentOutput` for M15b, and one match arm per tab in `tabs.rs` for M16.

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

- CI, 2026-11-12: Linux and the licence check are green on 8877094. macOS failed one app test that pressed
  Ctrl+C to copy where macOS uses Cmd+C. Fixed in the test.
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
