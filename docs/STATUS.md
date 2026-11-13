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
- **How a project begins** (the owner's change of 2026-11-12). The orchestrator's run of the whole workspace:
  496 tests pass, 0 fail, 9 ignored, clippy and fmt clean. Starting a conversation creates an uninitialized
  project. "hi" gets an answer and opens no task. A brief leads the CEO to call `set_up_project`, after which
  the code directory exists under the projects root with the slugged name, the task is open with the CEO's
  wording and the attachments, the scratch driver is retired, and a new driver in the worktree does the work.
  Two projects with one name get different directories. The Fernleaf script runs from the new beginning,
  in-process and through the daemon. The real `claude` and `codex` do the same against the fake provider. The
  app shows the message box with no projects and no form, "New project" until named, and a small control for
  an existing folder. The orchestrator looked at `new-project.png` and `new-project-hi.png`. Also in this
  step: "CEO" under the CEO's orb removed, the facts table deleted, and Memories, Skills and Trials not
  mounted until built (`REFLECTION_SERVED` in `crates/x100-app/src/team/mod.rs`). **Not run:** the gated
  real-login test and `capture-e2e.sh`, both updated to the new beginning. **Flake seen once by the subagent:**
  a Codex test in `x100d/tests/terminal.rs` reported no chunk during a turn.
- **App follow-ups from the owner's review.** The orchestrator's run: 498 tests pass, 0 fail, 9 ignored, clippy
  and fmt clean. The orchestrator looked at `drilldown.png` and `composer.png`.
  - The terminal is drawn at the normal font size and never shrunk. A viewer tells the daemon its pane's size
    in cells (`terminal.resize`) and the program redraws itself. Through the real daemon a resize reaches the
    real `claude` and `codex` (fake provider) and is recorded among the chunks. A driver starts at 100 by 40.
    With two viewers the last to send wins. A resize is sent once the pane's size has held for 250 ms. An ended
    driver's screen keeps its recorded size and scrolls.
  - Opening an agent fills the view with its output. Its head and name are small in the top-left of the row
    that holds the Agents and Drivers switch, with the trail back. A CEO with no leads opens straight to it.
  - One "+" button for files. Under WSL it opens a path field in place.
  - The `executive-summary` skill, at `crates/x100-memory/skills/executive-summary/SKILL.md`, is in the CEO's
    start pack only and governs only its messages to the builder and the summary of its report.
  - The chat shows only what the CEO sends with `message` and its report on a builder's task. A turn's text is
    no longer a chat message. A turn that ends with no message shows nothing in the chat.
  - A project not yet named shows as "Untitled".
  - **Not seen on screen:** a live terminal driver in the drilldown and a resize driven from the real window,
    since the scripted harness is headless. Covered by tests only until the next real run.

- **The sandbox and vault, wired in (M10 and M11 complete).** The orchestrator's run: 509 tests pass, 0 fail,
  10 ignored, clippy and fmt clean, nothing of x100 left running, nothing in the real `~/x100`. Through the
  daemon as a child process, on bubblewrap. `x100d/tests/sandbox.rs`: a command that tries to delete a
  stand-in home, write outside its worktree, connect to the host's loopback and reach 1.1.1.1 fails at each;
  one project cannot see another's worktree; a server started with `start_process` serves the agent's next
  driver; a reported task merges; terminating the daemon leaves no sandbox. `sandbox_real.rs`: the same with
  the real `claude` and `codex` against the fake provider. `vault.rs`: a credential's value reaches only its
  allowed host, the other host gets the reference, both uses are in the log, the echoed value comes back as
  the reference, and a byte search of the data directory and project for the value in raw, hex,
  percent-encoded and base64 forms finds nothing; it survives a restart and stops working when removed.
  - Claude Code runs in `bypassPermissions`, Codex with approval `never` and its own sandbox off. The
    sandbox is the containment. `start_process`, `stop_process` and `credentials.*` are real.
  - With no working sandbox, harnesses are passed over with the reason. `run_without_sandbox` is the unsafe
    way round, off by default, with a toggle in Settings.
  - Measured once on a busy machine, message to first output on the fake harness: 129 ms in a sandbox,
    45 ms without, medians of 15.
  - **Limits:** every login directory is visible to every driver. A driver can read every committed object
    of its project and write sibling branch refs. The proxy refuses literal loopback but not a name that
    resolves to it. Process logs inside a sandbox and bytes given to `add_file` are not redacted. A process
    that exits by itself is not recorded as exited.
  - **Not run:** the daemon on docker or podman end to end, a machine that lacks bubblewrap, the OS
    keychain, macOS (where the helper must be a Linux binary in the container and Codex's socket cannot
    cross the VM).
  - Two `codex app-server` processes from an earlier test run, three hours old, were found and stopped by
    pid. Sandboxed drivers now die with the daemon, so the same leak cannot recur that way.

- **The gate, wired in (M12 complete).** The orchestrator's run: 517 tests pass, 0 fail, 10 ignored, clippy
  and fmt clean, nothing left running. `x100d/tests/gate.rs`, daemon as a child process, fake harness and
  clock, bubblewrap: at "High risk only" a read from a package registry and `write_spec` go through, and a
  write to a hosting service's API is held with nothing sent; Approve reaches a replaced driver, whose retry
  goes through once; the next retry is held again; Deny sends nothing and the agent is told; an unknown
  host goes to a scripted classifier call, which is charged. At "Nothing" neither is held, at "Every
  decision" both. A held action survives a daemon restart and can then be decided. In-process: a merge into
  main is held at Medium and up, Approve merges, Deny sends the task back. In the app against a real daemon,
  Approve and Deny work from the strip. `AllowAll` is deleted. `InboxItem::Decision` gained a result.
  - **Limit:** at
    "Every decision" every tool call is held, `wait` and `report` included.
  - **Not run:** the permission-hook path (unreachable with harness prompts off), the classifier on a real
    program, a crash between Approve and the carry-out (recovery code exists, untested).
  - A turn cut off by a restart is replayed on the next driver, and can use a grant approved meanwhile.

- **M12D, decision models for the classifier.** The orchestrator's run: 532 tests pass, 0 fail, 13 ignored,
  clippy and fmt clean, nothing left running. `x100d/tests/decision.rs`, daemon as a child process, with a
  stand-in HTTPS server speaking all three APIs: for Gemini, OpenAI and Jev, an unruled write is held or
  allowed by the answer, never below the floor; an answer under 70% confidence is raised a level; an error,
  a refusal and a hang-up each hold the action; each call is charged at the provider's price; a spent budget
  holds without a call; the stand-in got the key in the right header and a search of the data directory,
  both logs and every reply finds it nowhere. With none connected a read goes through, a write is held, and
  no driver starts. Settings has a Decision model part, tested against a real daemon. `oneoff.rs` is
  deleted.
  - **Not run:** any real call to a provider (three ignored tests, `X100_REAL_DECISIONS=1`), so each
    adapter's request shape is unconfirmed, Gemini's most of all. `cargo-deny` locally. The 20 second
    timeout and the retries end to end.
  - **Flake seen once by the subagent, not by the orchestrator:** `x100-app/tests/window.rs`
    `the_first_message_creates_the_project...` at line 254 under load.
  - Also fixed here: `program_paths` compared a resolved path with an unresolved one, which failed on macOS.

- **M12I, integrations: Homebox and GitHub.** The orchestrator's run: 549 tests pass, 0 fail, 13 ignored,
  clippy and fmt clean. All on stand-ins: a stand-in Homebox server, a stand-in `docker` and `gh`, and a bare
  git repository as the remote. `x100d/tests/integrations.rs`, daemon as a child process: "use local" is
  offered first and connects with no key typed, and the minted key is found nowhere outside the vault by a
  byte search; without a local Homebox the builder gives a host and a key, and a wrong key, a read key and
  plain HTTP to another host are refused; a project is linked from the protocol and by the CEO by name, and
  the link survives a restart; without GitHub a deploy is refused with the reason; a deploy from the chat is
  held at "High risk only", runs on Approve, and the CEO is told the address or why it failed; a project with
  no repository is created, pushed, added, linked and deployed in one request; both `auto_deploy` settings
  deploy once; a deploy cut off by the daemon stopping fails on restart. The app's Settings has Integrations
  and the project's Homebox project. A driver's route to an opened loopback port is still refused.
  - **Not run:** the real mint command, real `gh` and push, real Homebox calls from x100, the device flow
    against GitHub (no OAuth app exists), macOS, a deploy cut off by SIGKILL.
  - **Limits:** the minted key stays in Homebox after Disconnect, since an edit key cannot revoke itself. A
    name that resolves to loopback is still not refused by the proxy, so a driver could reach the local
    Homebox port, where it holds no key. `desk.rs` and two test files are a little over 600 lines.
  - The driver tests no longer assert the exact installed version of Claude Code or Codex. Claude Code
    updated itself to 2.1.323 here and 13 tests failed on that alone. A version other than the tested one is
    allowed its warning, and the tests run against what is installed.
- **Agent looks, the first-run opening, glass buttons and the threshold setting.** The orchestrator's run:
  564 tests pass, 0 fail, 13 ignored, clippy and fmt clean, nothing left running. The orchestrator looked at
  `look-color.png`, `glass-states.png` and `opening-2.png`.
  - Every agent has a look: one of eight outlines and no colour or one of eight. The CEO starts as a plain
    circle. A new agent takes its creator's colour and an outline that is not its creator's and, while unused
    ones remain, not a sibling's. The choice is rolled once and recorded in the creating event. The dice are
    seeded in tests (`x100d --seed`), and the Fernleaf run gives the same looks over ten runs.
  - `agents.set_appearance` changes a look. In the app, clicking the face on an agent's page opens a popover
    with Outline and Color. The CEO has a page like any agent and is drawn in Team by the same widget and
    style as a lead. The first-run screen and the chat draw no face. A look survives a restart.
  - The first run opens with a glass shape that squares off into the message box while a rim grows to the
    window's edges, about 750 ms, skipped by any click or key, by reduced motion, and once a project exists.
    **It is a shape morph, not liquid:** GPUI has no custom shaders, blur or radial gradient. In the debug
    build the captured frames start at the rim stage. No release build was looked at.
  - Primary actions are one glass button and every toggle is a sliding glass piece, after the mock's PAB.
    **Differs from the mock:** a linear highlight, not radial; the inner light drifts only under the pointer,
    since an always-on drift cost 43% of a core idle against 1.1% now (debug build, about ten buttons); only
    the glass squashes on a press, not the label; at rest the buttons are a little greyer than the mock's.
  - The classifier's confidence threshold is a daemon setting, 70 by default, valid from 50 to 95.
  - **Not verified:** the macOS and Windows reduced-motion reads, a release build, the toggle mid-slide.
    `x100-app/tests/integrations.rs` timed out once under load in the subagent's run and passed after.
    `team/mod.rs` and `settings/mod.rs` are a little over 600 lines.
- **M13, memory and skills.** The orchestrator's run: 587 tests pass, 0 fail, 13 ignored, clippy and fmt
  clean, nothing left running. `x100d/tests/memory.rs`, daemon as a child process on the fake harness: an
  agent's `remember`, `read_note` and `load_skill` round-trip through the files, and each change is a git
  commit and an event carrying that commit; a note cannot cite an event that is not in the log; a replaced
  driver's start pack carries the index and not the note; with 1,000 notes the pack's memory section is at
  most 4 KiB; `search_log` finds a past message; a driver started while a skill change is only on a trial
  branch gets main's skill, and one started after the write to main gets the new one. The app's agent page
  shows notes and skills from the daemon.
  - **Closed:** what an agent read in handled messages and accepted reports is no longer lost when a driver
    is retired, provided the agent remembered it. Nothing captures it automatically; the guide tells the
    agent to. Core no longer says skill changes are always committed by trials.
  - **Limits:** transcripts are not searched. The 1,000-note case writes notes through the crate, not through
    a thousand tool calls. Trials stay hidden in the app until M14, which also adds "trial before commit".
  - `x100-app/tests/integrations.rs` has timed out three times under full-workspace load, at this commit and
    the one before, and passed each time after. Not yet traced.
- **The marketing site is deployed by hand through Homebox** at `https://x100.dev`, as project `x100-site`
  in base mode, production only, dev paused. Not through x100.

## In progress

M12I, integrations with Homebox and GitHub. Opus subagent.

Queued, in this order:

1. The two-part end-to-end run on the owner's logins, and a new recording.
2. M7, M9, M13, M14, M17.

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

- **2026-11-12. The gate's classifier is a decision model**: Gemini Decision 1, OpenAI's Decisions API or
  TypeSafe AI's Jev, not a language model run in a harness. Research in `docs/DECISION_MODELS.md`, design in
  ARCHITECTURE's Gate section, milestone M12D in the plan.
- **2026-11-12. The decision model offered first is the cheapest that is good enough, and the confidence
  threshold is a setting.** Gemini Decision 1 is offered first in Settings: its free tier makes it the
  cheapest in use and its documentation is the most complete. Jev has the lowest list price but its access is
  unclear and a paper reports it following an option's name over its description. The threshold stays 0.7 by
  default and becomes a daemon setting the builder can change. Built: valid from 50 to 95, changed in Settings under Decision model.
- **2026-11-12. Trials are a toggle, off by default.** After Anthropic's post on agent memory, reflection
  commits its changes directly unless the project's "trial before commit" setting is on. Corrections of notes
  against the log are never put on trial. This changed the decided line in ARCHITECTURE's "Evals, trials and
  the lab", PRODUCT's reflection features and M14's check. `x100-core` still names skill changes as "committed
  by trials" in its fold; M13 and M14 change that with the setting. The memory design also gained log search,
  notes that cite events, a bounded index and recorded reads, all proposed.
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
- **2026-11-12. Homebox connects locally by default.** When a Homebox server is on this machine, "use local"
  is the default and needs no key. Otherwise the builder gives a host address and an API key.
- **2026-11-12. Homebox and GitHub, for M12I.**
  - "Use local" is built as one fixed command in the `homebox-admin` container that mints an edit key for
    x100, kept in the vault. A proper Homebox command line replaces it later.
  - GitHub must be connected to x100 before x100 creates and pushes a repository for a project. Until then a
    project's repository stays local, with commits as now, and cannot be deployed through Homebox.
  - GitHub also has a "use local" way, the default when the `gh` command line is installed and logged in:
    the daemon runs `gh` and never holds its token. GitHub's own OAuth is the other way, and needs an OAuth
    app registered for x100, which only an account owner can do in GitHub's settings.
  - A public address on Homebox's primary domain is acceptable for the test. x100 finds the primary domain
    and the GitHub account through Homebox's API. The same GitHub account is the one to connect to x100.
  - The orchestrator may change Homebox's repository. Its uncommitted static-site support is committed
    locally in `~/code/homebox` (18 tests pass) and not pushed, since a push there starts Homebox's own
    deploys.
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

From how a project begins:

- **A project's own state is in the daemon's data directory**, under its id. The code directory holds the
  repository, specs and worktrees.
- **`chat.brief` is removed.** A builder's message is a message. `projects.create` takes an optional existing
  folder. `projects.rename` is new.
- **An incident:** before one test had its own projects root, it wrote two folders under the real `~/x100`,
  which did not exist before. The subagent deleted `~/x100`. Every test daemon now panics unless it is given a
  projects root.
- **`xtest.py` sends keys with `XSendEvent`**: under WSLg, keys sent through XTEST went to whatever Windows
  had in the foreground.
- **CI after the project-start push** failed in two tests that assumed this machine: one sandbox test
  panicked where `claude` and `codex` are not installed, and one vault test used `base64 -w0` and `od`, which
  differ on macOS. Both fixed.
- **CI after the three crates' push** failed on both systems for two reasons, both fixed here: the gate crate
  was committed ahead of the core change it was built against, and the sandbox's helper used a Linux-only call
  that does not build on macOS. The sandbox's tests now pass over a backend the machine lacks, and Linux CI
  requires both.

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
- **What a real driver may do** is one setting in `x100-orch/src/spec.rs`, now allow-all inside the sandbox;
  see "The sandbox and vault, wired in" above.
## Could not be verified, and why

- Tests on a machine that cannot make a sandbox (macOS CI), 2026-11-12: containment tests print SKIPPED and
  the rest run with `run_without_sandbox` on, by one rule in `x100-sandbox/tests/common/available.rs`.
  Simulated here by hiding `bwrap` from the path: every daemon and app test passed. Three Docker tests of the
  sandbox crate failed in that simulation only ("Permission denied" removing the sandbox directory), cause
  not found. `real_e2e::the_same_run_goes_through_on_the_fake_provider` hangs when run uncontained, so it
  skips there; the hang itself is not fixed.
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
