# x100 v1 plan

Status: the build plan, revised 2026-11-11 with the owner's answers. Depends on `docs/PRODUCT.md` (what) and
`docs/ARCHITECTURE.md` (how). This is a new codebase with no users, so nothing here keeps an old shape alive:
when a design changes, the code changes with it and the old form is deleted.

## What v1 is

One builder, on one machine, gives a brief to a CEO and watches a real team of agents carry it out safely,
from the desktop app or from the terminal.

v1 is done when all of this is true on a clean machine, on Linux, on macOS and under WSL:

1. Create a project, write a brief with an attachment, and send it.
2. The CEO replies, writes a BRD, forms a team, and leads delegate to workers. At least two different
   providers' harnesses do real work in the same project.
3. The Team tab shows the tree live, and lists every driver with its harness, model, effort and machine.
   Opening any agent shows its own agents or its live output.
4. Specs, Tasks, Files, Budget, Log and Settings show real data and update in place.
5. A destructive command inside an agent harms nothing outside its sandbox.
6. A credential is used by an agent and its value appears in no transcript, log or event.
7. A high-risk action is held, shown, and resumes on Approve or is dropped on Deny.
8. A lead stops starting work at its own budget, and the project stops at its total.
9. A second task sent to an agent soon after its first runs on the same driver and keeps its context.
10. A driver killed mid-task is replaced by a compatible driver from another provider, which finishes the
    task from where the work stood. The agent's view shows one continuous output with the handoff marked. The
    same holds when the daemon is killed mid-task.
11. An agent schedules a job and waits on it, a lead waits on its worker's report, and the CEO waits on the
    builder's answer. Each is woken when its wait ends, and the ledger shows nothing spent while it waited.
12. Reflect now tests before it changes anything. It tries a changed skill on past tasks with and without the
    change, commits it only when the trial shows it helps, drops one that does not, and shows both trials with
    their scores. The next task loads the committed skill. Nothing the trials did touched the project.
13. Closing and reopening a client loses nothing, and agents kept working meanwhile.
14. Points 1 to 13 hold in the desktop app and in the terminal interface.
15. Published memory and CPU figures for a stated workload, and sandbox timings for both backends.

Not in v1: Preview inside the window, the Infra map, the mobile app, more than one device, remote machines
(so no handoff between machines), scheduled reflection, sharing skills across projects, the database storage
engine, theming.

## How the work is run

- One orchestrator session plans, briefs and verifies. Subagents write the code.
- Each milestone is one subagent's job. It owns the crates named, and no one else edits them meanwhile.
- `x100-core` and `x100-proto` are the contract, and M1 and M2 define all of it, including requests that later
  milestones make real. After those two are accepted, a change to either goes through the orchestrator.
- A subagent finishes by showing the output of its check. The orchestrator then runs the check itself, plus
  `cargo build`, `cargo test`, `cargo clippy -- -D warnings` and `cargo fmt --check` for the whole workspace,
  before the next milestone starts.
- One subagent at a time, except for the pairs marked parallel, which own disjoint crates.
- A milestone whose crate the daemon must call also owns the calls to it in `x100-orch` and `x100d`. Where its
  parallel partner owns one of those crates, that wiring is a short step of its own after both are accepted,
  and the milestone's check is run again through the daemon.
- Opus where the table says so. Sonnet for the rest.
- Everything is testable without a paid model: the scripted fake harness from M3 stands in for real ones, and
  the fake clock from M1 stands in for time. Tests that call a real provider run only when asked for.

## Milestones

| # | Milestone | Crates | Depends on | Model |
|---|---|---|---|---|
| M0 | Workspace and CI | all, empty | | Sonnet |
| S1 | GPUI spike | `spikes/gpui`, thrown away after | M0 | Opus |
| M1 | Domain, events and state machines | `x100-core` | M0 | Opus |
| M2 | Store and protocol | `x100-store`, `x100-proto` | M1 | Sonnet |
| M3 | Drivers and harnesses | `x100-drivers` | M1 | Opus |
| M3T | Terminal drivers on local logins | `x100-drivers` | M3, M6 | Opus |
| M4 | Jobs and the clock | `x100-jobs` | M2 | Sonnet |
| M5 | Daemon and client library | `x100d`, `x100-client`, the turn loop in `x100-orch` | M2, M3, M4 | Sonnet |
| M6 | Orchestration: agents, delegation and waiting | `x100-orch` | M5 | Opus |
| M7 | Terminal interface | `x100-tui` | M6 | Sonnet |
| M8 | Router, budgets and driver reuse | `x100-router` | M6 | Opus |
| M9 | Handoff | `x100-orch` | M8 | Opus |
| M10 | Sandbox | `x100-sandbox` | M6 | Opus |
| M11 | Vault and proxy | `x100-vault` | M10 | Opus |
| M12 | Gate | `x100-gate` | M6 | Sonnet |
| M12I | Integrations, with Homebox first | `x100-integrations` | M11, M12 | Opus |
| M13 | Memory and skills | `x100-memory` | M6 | Sonnet |
| M14 | Lab: evals and trials | `x100-lab` | M8, M10, M13 | Opus |
| M15 | Desktop app: shell, chat and Team | `x100-app` | M7, S1 | Opus |
| M16 | Desktop app: remaining tabs | `x100-app` | M15 | Sonnet |
| M17 | Hardening and release | all | everything | Opus |

Parallel pairs: S1 with M1. M3 with M2, then with M4. M7 with M8. M3T beside M7 to M9. M9 with M10. M11 with M12. M15 may run beside
M8 to M14.

The terminal interface comes before the desktop app on purpose. It is the cheaper client, it proves the client
library and every tab against the real daemon, and it gives the orchestrator a way to drive the product by hand
from M7 on.

### M0. Workspace and CI

A Cargo workspace with every crate from `ARCHITECTURE.md` as an empty library or binary. Lints, formatting,
the licence, a README that says what x100 is, and CI that runs build, test, clippy and fmt on Linux and macOS.

Check: a fresh clone builds and the four commands pass. CI is green.

### S1. GPUI spike

A throwaway window that shows the three things the desktop app depends on: a list that updates in place
without flicker, a Markdown view, and a live terminal fed by a running process.

Check: it runs on Linux, on macOS and under WSL. A short report says what GPUI cannot do or does badly on any
of the three. If the report says GPUI cannot carry the app, stop and bring that to the owner before M15.

### M1. Domain, events and state machines

All domain types, ids and events, agents and drivers among them. The functions that fold events into the state
each tab shows, so that the store and every client compute it with the same code. The six state machines as
pure functions `next(state, event) -> Result`. The `Clock` trait and a fake clock that tests move by hand. No
I/O.

Check: unit tests for every transition. Stateright proves the invariants listed in `ARCHITECTURE.md`. A
deliberately broken transition makes a proof fail.

### M2. Store and protocol

Append and read the event log and agents' transcripts in SQLite. File storage. Checkpointing a worktree to a
hidden ref and restoring it. Every request, response and the event subscription as types with a documented wire
format, covering every resource in the API section of `ARCHITECTURE.md`, including those M6 to M14 will make
real.

Check: replaying a recorded log gives identical tab state. A subscriber that connects late gets a snapshot,
then every later event exactly once. Killing the process mid-write loses no committed event. A checkpoint of a
worktree with uncommitted and untracked changes restores the same files.

### M3. Drivers and harnesses

The harness trait and running a driver: start it, stream its events, give it a further turn, report how full
its context is, compact it, stop it, and notice when it fails (its process exits, its stream breaks, it reports
a provider error, or it is silent past the stall timeout). A scripted fake harness driven by a script file,
which can be told to crash or stall at a set point and can pose as more than one provider. Real harnesses for
Claude Code and Codex, run headless.

First task: confirm how each provider's agent is run headless today, what structured output it gives, whether
it can take a further turn in the same process, whether it can compact, whether it has a permission callback,
how it is given the daemon's tools over MCP, and which of its errors mean the provider is failing. Report before
writing the harnesses.

Check: the fake replays a script deterministically. When asked to, each real harness completes a one-file edit
in a temporary directory and reports usage. A second turn given to the same driver keeps the first turn's
context. Stopping a driver stops its child process. A fake told to crash is reported Failed with the reason.
One told to stall is reported Failed once the stall timeout passes on the fake clock, and not before.

### M3T. Terminal drivers on local logins

Each harness's own interactive program run in a pseudo-terminal as a driver, on the builder's local login:
start it, give it the start pack and later turns, give it the daemon's tools, learn of turn ends, usage and
context fill, compact it, stop it, and notice when it fails. Its terminal recorded per driver and followed
live through the protocol.

First task: confirm how `claude` and `codex` are driven as terminal programs today: how a prompt is given at
start and typed later, which hook or signal says a turn ended, where usage and context fill can be read, how
MCP servers and settings are given for one run without loading the builder's own hooks and memory files while
keeping the builder's login, how first-run and trust prompts are avoided, and what the harness's own terms
say. Report before building.

Check: against a fake provider on localhost, each real program in a pseudo-terminal completes a one-file
edit, calls one of the daemon's tools, takes a second turn that keeps the first turn's context, and is
stopped with nothing left running. The recorded terminal replays to the same screen. A client following the
terminal sees output as it arrives. When asked to, the same on the builder's local login for each harness.

### M4. Jobs and the clock

The real clock behind the `Clock` trait. Jobs: once at a time, after a delay, or on a repeat; cancelling one;
firing as an event; the minimum repeat. A job that fell due while the daemon was stopped fires once on start.

Check: on the fake clock, a one-off job fires once at its time, a repeating job fires at each interval, and a
cancelled job never fires. A job due across a restart fires once, not once per missed interval. Property tests
over random schedules, cancels, restarts and clock moves hold the job invariants in `ARCHITECTURE.md`.

### M5. Daemon and client library

`x100d` serving the protocol on a local socket, surviving its clients and restarting cleanly from the store,
with the job clock running inside it. `x100-client`: connect, take a snapshot, follow events, send requests, and
hold the state each tab draws from. The turn loop in `x100-orch`, which M6 builds on: an inbox item starts a
driver in the agent's worktree, the driver's turn is recorded, and its reply is an event.

Check: an integration test starts the daemon, creates a project and sends a message through the client library
with the fake harness, and sees the reply arrive as events. Restarting the daemon while a driver is running
marks every driver that was mid-turn as Failed, with the reason, ends every warm one, and never leaves one in
a live state.

### M6. Orchestration: agents, delegation and waiting

The CEO, lead and worker behaviour. The tools the daemon serves to agents. Delegation down the tree, with each
report going back to the delegator. The inbox, where an item counts as handled only when its turn completes.
Waits with deadlines, on jobs, reports, replies and decisions. `schedule`, and jobs that wake an agent or
delegate a task. The start pack, from which every driver starts. An agent with work and no running driver gets
one. Lazy leads. Lead to lead messages. The CEO messaging the builder and waiting for the answer without
blocking other work. One chat per project: a builder's message goes to the CEO only.

Check: with the fake harness and a script modelled on the Fernleaf demo, the event log shows a BRD written,
three leads formed, workers started under two of them, one worker with its own workers, a lead to lead message,
a question to the builder answered, tasks moving through their statuses, and a final report. No lead is started
before it has work. On the fake clock: a worker waits on a job it scheduled and is woken when it fires; a lead
waits on its worker's report and resumes when it arrives; a repeating job delegates a fresh task to a lead at
each firing; a wait past its deadline wakes the agent and says so. An inbox item given to a driver that is
stopped before its turn completes reaches the next driver. An agent whose driver is stopped during a long wait
resumes on a new driver started from its start pack, with its progress note.

### M7. Terminal interface

A full client in the terminal: projects, the first-run brief, chat with the CEO, all the tabs, the drivers list,
opening an agent to watch its live output, its waits and jobs, its memories, skills and trials, held actions
with approve and deny, settings and credentials. It
also runs single commands without the full screen, for scripts and tests: create a project, send a message,
follow the log.

Check: driven against the daemon with the M6 script, every tab matches the state the client library holds.
Opening an agent three levels down shows its output as it arrives, and keeps showing it across a change of
driver. Quitting and reopening shows the same project. The single commands work in a pipeline.

### M8. Router, budgets and driver reuse

Harnesses' models with effort and price. An effort range per agent. Allowed harnesses and models. Finding a
driver as `ARCHITECTURE.md` describes: the agent's warm driver if it is compatible, else the cheapest compatible
new one on a machine under its cap, else a wait for a driver with the reason. Cool-offs after provider failures.
The ledger, with a budget per lead inside the project's total, charged as `ARCHITECTURE.md` describes. Warm
drivers: compact after an accepted task, retire by context size, task count and age.

Check: table tests for the choice across ranges, allow-lists, cool-offs and full machines, including nothing
compatible, where the agent waits, its parent is told why, and it starts when a setting changes. A lead at its
budget starts nothing while other leads carry on. A project at its total starts nothing. Changing a range or a
lead's budget changes the next decision. A task arriving while a compatible driver is warm reuses it. One
arriving after the warm period, or needing more effort than the warm driver has, gets a new driver. A driver
past the context threshold between tasks is retired, and what the agent needed from it is in its memory. A
harness and model that fail with a provider error are skipped until their cool-off ends.

### M9. Handoff

Handoff between drivers as `ARCHITECTURE.md` describes. Takeover when a driver fails, when the builder replaces
it, and after a daemon restart. Planned handoff when the context fills mid-task, when the task's effort changes,
and when the builder disallows the driver's harness or model or narrows the agent's range below it. Fencing, so the daemon refuses calls from any
driver but the agent's current one. A checkpoint at every turn end. A start pack that says why the last driver
stopped and what was in flight. The limit of three failures in a row.

Read the sections of `ARCHITECTURE.md` from "Agents and drivers" to "Delegation, the inbox and waiting" first.

Check: with the fake harness posing as two providers, a driver killed mid-task is replaced by one from the
other provider at a compatible effort, which finishes the task. The worktree holds the first driver's changes.
The new driver's start pack lists every daemon tool call the first one made in the task, and each delegation,
message and job exists once in the log. The agent's transcript is one record with the handoff marked. A driver
killed with a daemon tool call in flight: the call takes effect once, and its result is in the start pack. A
call from a replaced driver is refused. A stalled driver is replaced once the stall timeout passes. Each planned
reason hands off after the old driver brings its progress note up to date. Three failures in a row report the
task failed to its delegator. Killing the daemon mid-task: on restart every working agent carries on. When
asked to, the kill test passes with Claude Code and Codex as the two harnesses, in both directions.

### M10. Sandbox

Create, run in and destroy a per-agent sandbox holding a git worktree on its own branch, with egress only
through a given proxy address. Two backends behind one trait: bubblewrap for Linux and WSL, containers for
macOS. Every driver an agent has runs in its one sandbox.

Check: from inside, deleting the home directory, writing outside the worktree and opening a direct outbound
connection all fail or affect only the sandbox. Two agents' sandboxes cannot see each other's files. Merging
an agent's branch brings its work into the project. A new driver started in an agent's sandbox sees the files
the last one left, and a server the last one started in the background is still serving. The same tests pass on
both backends. For each backend, record the time to create a sandbox, the time to start a command in one, and
the overhead on a full build of this workspace against running it unsandboxed.

### M11. Vault and proxy

An encrypted store keyed from the OS keychain. References. The egress proxy that swaps references for values
on allowed hosts. The wrapper for named command-line tools that need a credential in their environment.
Redaction of everything a driver emits. An event for every use.

Read the vault section of `ARCHITECTURE.md` first, including the wrapper's limits.

Check: an agent makes an authenticated request using only a reference, and succeeds. Searching the event log,
the agent's transcript and the sandbox's files for the value finds nothing, including base64 and URL-encoded
forms. A request to a host the credential is not allowed on goes out without it.

### M12. Gate

Rules from action kind and target to risk. A model classifier for what the rules miss. The "Ask me about"
levels. Hold, approve, deny, resume. A held action does not hold up the driver.

Check: table tests for the rules. End to end with the fake harness: at "High risk only", a production deploy
is held and a file edit is not; Approve resumes it; Deny drops it and the agent is told; at "Nothing" neither
is held; at "Every decision" both are. An Approve that arrives after the agent's driver was replaced reaches
the new driver.

### M12I. Integrations, with Homebox first

Integrations as a part of x100 the builder connects once and projects then use: a list of supported
integrations, one consistent way to connect each (OAuth where the service offers it, a pasted key only where
it does not), the credential kept in the vault, and each call to the service made by the daemon so no agent
holds the credential. A project setting that links the project to something in an integration. Tools that let
the CEO do what the builder can: list integrations, link the project ("connect this to the Homebox project
called foobar"), and deploy. Homebox, the owner's own self-hosted platform, is the first. It connects in one of two ways: "use local",
the default when a Homebox server is found on this machine, which uses Homebox's own direct local access and
needs no key; or a host address and an API key, since Homebox is self-hostable and may be elsewhere. It has
no OAuth of its own yet. The project is linked to a Homebox project and deployed to it. Homebox deploys only
from a hosted repository, so GitHub is the second integration. It connects in the same two ways: "use local", the default
when the `gh` command line is installed and logged in on this machine, in which case the daemon runs `gh`
for the few things it needs and never reads or stores its token; or GitHub's own OAuth, which needs no
pasted token. Connected, it lets x100 create a private repository for a project and push to it. Until GitHub is connected a project's
repository stays local and cannot be deployed through Homebox, and x100 says so. A deploy
is an action the gate rates as high risk.

Check: with a stand-in for Homebox's API, "use local" is offered first when a local server is found and
connects with no key, and without one the builder gives a host and a key. Connecting with a key stores it in
the vault and the key is in no event,
transcript or reply. The builder links a project from Settings, and the CEO links one by name from the chat.
A deploy asked for in the chat is held at "High risk only", runs on Approve, and reports the deployed
address. A deploy that fails says why. When asked to, the second part of the end-to-end run on the builder's
logins and the builder's own Homebox: after the todo app is built, a follow-up message asks for a small
edit, then for a deploy through Homebox, and the edited page is served at the deployed address.

### M13. Memory and skills

Memories and skills as Markdown in folders under git, with a main branch that agents load from and branches
for changes on trial. `remember` and `load_skill`. No other crate reads or writes these files: they go through
this crate's interface, which is what a later storage engine will stand behind.

Notes cite the events they came from, a bounded index is what a start pack carries, agents can search the
log, and each note read is an event (see "Memory, skills and reflection").

Check: an agent's `remember` and `load_skill` calls round-trip through the files. A start pack holds the
index and not the notes, and stays under its size limit however many notes exist. A search of the log finds a
past message. A driver started after a
skill changes on the main branch receives the new skill, and one started while a change is only on a trial
branch does not. Every change is a git commit and an event.

### M14. Lab: evals and trials

Reflect now, gated by trials. Eval sets built from the project's own accepted past tasks, from checks the CEO
writes, and from probe questions for memories. The lab: a fresh sandbox from a checkpoint for each run, lab
drivers with no credentials and no outbound writes, a lab line in the ledger with its own cap. Scoring, the
rule for committing, and merging or deleting the trial branch.

Check, with the fake harness scripted so that one skill change helps and another does not: the helpful change
is committed and the other is discarded, each after both arms ran three times, and both trials are in the log
with their scores. A memory compaction is committed only when every probe question is still answered. A note
that contradicts the log is corrected and cites the events that show it. A compaction that leaves the memory
larger than it was, or the index over its limit, is discarded. After
the trials, the project's worktree, files and non-lab ledger lines are byte-for-byte what they were. A lab
driver's attempt to use a credential or write outbound is refused. A trial that would pass the lab's cap is
stopped and recorded as such. A proposed team change arrives as a held action with its telemetry attached.

### M15. Desktop app: shell, chat and Team

The window, the projects sidebar, the first-run state with the CEO and the brief box, chat with attachments,
the tab bar with change dots, and the Team tab: rows with task, progress, driver, effort and range, the drivers
list, opening an agent, the trail back, and an agent's own agents or its live output. Built on `x100-client`,
like the terminal interface.

Check: driven against the daemon with the M6 script, a brief can be sent, the tree fills in live, rows do not
flicker when other rows change, and three levels down there is a live terminal showing output as it arrives. A
change of driver shows as a marked change in that same terminal. Killing and reopening the app shows the same
project. A screenshot matches the layout of the page's mock.

### M16. Desktop app: remaining tabs

Specs, Tasks, Files, Budget, Log, Settings with its Credentials part, the Preview link, an agent's memories,
skills and trials in its view, and held actions with Approve and Deny.

Check: each tab matches the state the client library holds for the M6 script. Removing a file logs it.
Changing an effort range, a lead's budget or the allowed harnesses and models reaches the router. Approving a
held action resumes it.

### M17. Hardening and release

Run the whole definition of v1 on clean machines with real providers. Fix what fails. Tune the thresholds for
reuse, retirement, stalls and the start pack against real providers. Measure memory and CPU with a stated
workload and write the numbers down. Package for Linux and macOS. Write the install and first-project guide,
including WSL.

Check: the fifteen points at the top of this file, each with evidence.

## After v1

Rough order, each with its own plan when it comes up:

1. Scheduled reflection, as a repeating job, and the CEO applying team changes itself.
2. Preview inside the window.
3. Remote machines as runners for drivers, handoff between machines, and the Machines list.
4. A second device on the same project, and the mobile app.
5. The database storage engine, with several writers.
6. The Infra map.
7. Sharing skills across projects.
8. Theming.
