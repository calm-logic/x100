# x100 architecture

Status: the design for the first build, revised 2026-11-11 with the owner's answers. Read `docs/PRODUCT.md`
first.

Every choice below is marked **decided** (the owner's, or it follows from the product) or **proposed** (the
orchestrator's default, built on unless the owner overrules it). Nothing waits on an answer. The proposed
defaults the owner is most likely to care about are listed at the end.

This is a new codebase with no users. Nothing is kept for compatibility: when a design changes, the code
changes with it and the old form is deleted.

Nothing here was verified against current library versions. The first task of any milestone that adopts a
library is to check that the library does what this document assumes, on the target operating systems.

## Constraints

- **decided** Interface and backend in Rust. No browser engine renders the app's own interface.
- **decided** Open source. Prefer existing open-source parts and the owner's own systems over new infrastructure.
- **decided** Agents never hold credential values. References only, injection outside the agent, redaction, audit.
- **decided** Explicit state machines are verified, not only tested.
- **decided** The interface is a client of a daemon from the first commit, so more devices and more machines can
  be added without a rewrite.
- **decided (owner, 2026-11-11)** Agents and drivers are separate, and both are first-class. No agent is tied to
  a driver: any compatible driver, from any provider, can carry on an agent's work, mid-task included.

## Shape

```
 x100 (desktop app, GPUI)          x100 (terminal interface)          V2: mobile app
          │                               │                                   │
          └────────────── local socket, JSON-RPC + event stream ──────────────┘
                                          │
                                   x100d (daemon, Rust)
        ┌──────────────┬──────────────┬───┴──────────┬──────────────┬──────────────┐
   orchestrator      router          jobs           gate          vault          store
     (agents,      (finds and      (clock,       (classify,    (references,   (event log,
   delegation,     allocates        wakes,         hold,          proxy)      transcripts,
      waits,        drivers,      deadlines)      resume)                     files, git)
     handoff)       budgets)
        │              │
        └──────┬───────┘
            drivers ── harnesses: Claude Code · Codex · custom
               │
   sandboxes (one per agent)
```

One daemon per machine owns all state and all agents. The desktop app holds no state it cannot rebuild from the
daemon. Closing the window does not stop the agents.

## Crates

**proposed** A Cargo workspace. Each crate is one subagent-sized area with a narrow public interface.

| Crate | Owns |
|---|---|
| `x100-core` | Domain types, ids, the events, the functions that fold events into what each tab shows, the state machines, and the `Clock` trait with a fake clock for tests. No I/O. Everything else depends on it. |
| `x100-store` | The event log and agents' transcripts in SQLite. File storage. Git operations, including worktree checkpoints. |
| `x100-proto` | The client protocol: requests, responses, event stream. Shared by daemon and clients. |
| `x100-drivers` | The harness trait, running a driver and noticing when it fails, a scripted fake harness for tests that can crash or stall on cue, and one harness per supported agent program. |
| `x100-router` | Finding and allocating drivers: effort ranges, allowed harnesses and models, cost table, cool-offs, machine caps, warm drivers, the ledger, budget enforcement. |
| `x100-jobs` | The real clock, scheduled jobs, and firing them. |
| `x100-sandbox` | Create, run in, and destroy an isolated workspace for one agent. |
| `x100-vault` | Encrypted credential store, references, the egress proxy, redaction. |
| `x100-gate` | Action classification, risk levels, the pause and resume flow. |
| `x100-orch` | Agents: the CEO, lead and worker behaviour, delegation, the inbox, waits, the start pack, handoff between drivers, the tools agents are given, messaging, reporting. |
| `x100-memory` | The memory and skill repositories. |
| `x100-lab` | Reflection: evals, trials, the lab they run in, scoring, and the rule for committing a change. |
| `x100d` | The daemon binary. Wires the crates together, runs the job clock and serves the protocol. |
| `x100-client` | What every interface shares: the connection to the daemon and the state each tab draws from. |
| `x100-app` | The desktop interface, in GPUI. |
| `x100-tui` | The terminal interface. |

## Domain model

**proposed** Event sourced. Every change is an event appended to the project's log. What a tab shows is
computed from events by functions in `x100-core`, and the daemon and every client use those same functions.
This gives the Log tab, sync between devices and "reading the log back" during reflection from the same data.

Core entities: `Project`, `Agent` (kind: CEO, lead or worker; parent; role; charter; effort range; budget for a
lead), `Driver` (one harness running one model at one effort level on one machine, for one agent), `Harness`,
`Machine`, `Task` (what is asked; delegator; assignee; effort needed; status; progress note; report),
`Message`, `Job`, `Wait`, `Spec` (BRD, PRD, ADD; revision; status), `FileRef`, `Ledger`, `Action` (what an
agent wants to do, with its classification and decision), `Skill`, `Memory`.

State machines to make explicit and verify:

- **Agent**: Planned → Working when its first inbox item arrives. Working → Waiting when it waits, or when no
  driver can be found for it; Waiting → Working when anything reaches its inbox. Working → Idle when it has no
  open task and nothing to wait for; Idle → Working when anything reaches its inbox. Idle → Retired. An agent
  has no Failed state: its drivers fail, and its tasks can end in a failure report.
- **Driver**: Starting → Running → Warm → Running again, or Warm → Compacting → Warm. Running → Handing off →
  Ended for a planned handoff. Warm → Ended when retired. Any live state → Ended when stopped, or → Failed.
  Ended and Failed are final and record why.
- **Job**: Scheduled → Done when a one-off job fires, or back to Scheduled with its next time when a repeating
  job fires. Scheduled → Cancelled.
- **Action**: Proposed → Classified → Allowed, or Held → Approved or Denied.
- **Task**: New idea → In progress → In review → Promoting → Done. A task delegated new starts In progress.
- **Trial**: Proposed → Running → Scored → Committed or Discarded.
  In review → In progress when the delegator sends it back, a failed task included.

Invariants worth proving:

- Every delegated task ends in a report to its delegator, even when the assignee's drivers fail.
- A Held action never runs before it is Approved.
- Spend never exceeds the budget.
- An agent has exactly one parent and the tree has one root.
- At most one driver acts for an agent at any time, and a driver acts for one agent only, for its whole life.
- Every driver is compatible when it starts: inside its agent's effort range, at or above its task's effort, on
  a harness and model the builder allows.
- Nothing delivered to an agent is lost: every inbox item is handled by a turn that completed.
- No agent waits forever: every wait has a deadline, and a wait on something that can no longer happen ends.
- A cancelled job never fires, and a one-off job fires at most once.
- No change from reflection reaches an agent's skills or memories without a Committed trial behind it.
- Nothing a lab driver does reaches the project: no merge, no file, no outbound write, no charge outside the
  lab's cap.

**decided (owner, 2026-11-11)** Verification is exhaustive model checking of the Rust state machines with
Stateright, plus property tests. The `pstack:create-formal-verification-skill` skill exists for this.

## Agents and drivers

This is the centre of the product.

**decided (owner, 2026-11-11)** An agent and a driver are different things, and the design keeps them apart
everywhere. Both are first-class: each has its own id, record, state machine, events, API resource and place in
the interface.

- An **agent** is a member of a project's team: the CEO, a lead or a worker. The work belongs to it.
- A **driver** is what does an agent's work: an agent harness running on a specific machine or VM, with a
  specific model at a specific effort level. A **harness** is the agent program: Claude Code, Codex, or a
  custom harness around an LLM.

**proposed** An agent is a record in the daemon: a role, a parent, a charter, an effort range, a budget if it
is a lead, a sandbox, a memory repository, and its tasks, inbox and waits. It is not a model and not a process.
It keeps its identity and its work while drivers come and go.

**decided (owner, 2026-11-11)** If a driver crashes mid-task, a new driver of compatible effort steps in and
takes over, even one from another provider. The daemon finds or allocates compatible drivers itself.

**decided** It follows that a driver's context is only a cache. Everything an agent's work needs in order to go
on lives in the daemon, in a form no provider owns, so any compatible driver can rebuild that context. Reusing a
warm driver keeps the cache, compaction shrinks it, retirement drops it, and a handoff rebuilds it on another
driver.

**proposed** A driver works for one agent for its whole life and is never moved to another, since its context
is that agent's. An agent has at most one driver at a time. Work that should run in parallel is delegated to
workers, each with its own sandbox and drivers.

## Harnesses

**proposed** The harness interface, in `x100-drivers`:

```rust
trait Harness {
    fn models(&self) -> Vec<ModelInfo>;     // each model, the effort levels it can run at, and the price
    fn supports(&self) -> Support;          // further turns, compaction, context fill, permission hook
    async fn start(&self, spec: DriverSpec) -> Result<DriverHandle>;
}
// DriverSpec: the model, the effort level, the sandbox to run in, the start pack, and this driver's own
// address for the daemon's tools.
// A DriverHandle yields a stream of DriverEvent (text, tool call, tool result, usage, turn ended, failed)
// and accepts input (a turn's input, an approval, stop).
// Where the harness supports it, the handle also takes further turns, reports how full the context is, and
// compacts.
```

A turn is one round of work: the driver is given input (the start pack, or new inbox items), works with its
tools, and ends the turn.

**proposed** Every harness must take the start pack as its instructions, call the daemon's tools, stream what
it does and what it spends, end a turn, and stop when told. The rest is optional, and the daemon works without
it: a harness that cannot take a further turn gets a new driver for each turn, and one that cannot compact is
retired instead.

**proposed** First harnesses: Claude Code and Codex, each run headless as a child process with structured
output. A custom harness around an LLM is one more implementation of the trait. Consider the Agent Client
Protocol as a third harness, since it would bring in every agent that speaks it.

## What lives outside a driver

**proposed** The daemon keeps, for every agent, everything a new driver needs to pick up its work:

- **The agent**: role, charter, parent, children, effort range, memories and skills.
- **Its tasks**: what each asks, who delegated it, the effort it needs, its status, and its progress note. The
  progress note is the driver's own short account of where the task stands: what is done, what is next, what
  was decided. Drivers keep it current with `update_task`, and `wait` cannot be called without one.
- **Its inbox**: everything delivered to it and not yet handled.
- **Its open business**: waits, jobs it scheduled, tasks it delegated and has not accepted, messages awaiting a
  reply, held actions.
- **Its transcript**: every turn of every driver it has had (text, tool calls, results, usage), recorded by the
  daemon in x100's own format as the driver streams it. No provider's own transcript is relied on.
- **Its work**: the worktree in its sandbox, which the daemon commits to a hidden checkpoint ref at the end of
  every turn, so the diff since the task began is always known.
- **Its spend**, in the ledger.

**proposed** Every driver starts from a **start pack** the daemon builds from those records:

1. The charter, memories and skills.
2. The current task and its progress note.
3. Every daemon tool call made in the task, with its result, so nothing is done twice.
4. As much of the transcript's tail as fits.
5. The worktree's diff since the task began, and any processes still running in the sandbox.
6. Open business and unhandled inbox items.
7. Why this driver is starting: a new task, a wake, a planned handoff and its reason, or a takeover with what
   failed and what was in flight.

The pack takes at most a set share of the new model's context window. Older transcript is what gets cut, which
is why the progress note matters.

A fresh task, a wake after a long wait, a planned handoff and a takeover after a crash all start a driver the
same way. They differ only in what the records hold.

**proposed** A takeover always rebuilds from the start pack, even when the new driver uses the same harness. A
provider's own resume would be a second path that is rarely exercised, and the start pack has to work across
providers anyway.

## Finding a driver

**decided (owner, 2026-11-11)** The daemon finds or allocates a compatible driver for all work. No agent and no
builder picks one by hand.

**proposed** Each harness publishes the models it runs, each at one or more effort levels (Low to Max) with a
price. The delegator states the effort a task needs when it delegates it. A driver is compatible with an
agent's work when all of these hold:

1. Its effort level is at least what the task needs, once that is clamped into the agent's range, and is itself
   inside the range.
2. The builder allows its harness and model in the project.
3. Its harness and model are not cooling off after a provider failure.
4. Its machine can run the agent's sandbox and is under its cap on live drivers. In v1 there is one machine.
5. The lead's budget and the project's both allow it.

The router first tries to find one: the agent's warm driver, if it is compatible. Failing that, it allocates
one: it starts the cheapest compatible harness, model and effort level. The choice and its cost go in the log
and the ledger. If nothing is compatible, the agent waits for a driver and its parent is told why. A change that
could help (a setting, a budget, a cool-off ending, a slot freeing) ends the wait. If the wait reaches its
deadline, the daemon reports the task failed to its delegator.

**proposed** A harness and model that fail for a reason that looks like the provider's (an outage, a rate
limit, a refused key) cool off for a set time, for every project, and are skipped meanwhile. The end of a
cool-off is a job.

**decided (owner, 2026-11-11)** Budgets are per lead inside a project total. The ledger holds the project's
budget and one budget per lead, set by the CEO or by the builder. A driver's cost is charged to the lead at the
top of its agent's branch of the tree, and the CEO's own drivers to the project. Before a driver starts, the
router checks both the lead's budget and the project's.

## Reuse, compaction and retirement

**decided (owner, 2026-11-11)** Drivers are reused where it pays.

- **Reuse.** When an agent finishes a task and another arrives soon after, the same driver takes it, so the
  agent keeps its context and the provider's prompt cache stays warm.
- **Retire.** A driver ends once its context is large and filled with many unrelated tasks.
- **Compact.** A driver can be compacted after each task is finished and the CEO has verified it.

**proposed** How that works:

- A driver whose turn ends goes to Warm instead of ending. A warm driver is kept for a short time, set to match
  the provider's cache lifetime. The end of that time is a job.
- The router prefers the agent's warm driver when it is compatible, as above.
- After the task's delegator accepts it (the CEO, for a lead's task), the driver is compacted if its harness
  can do that. If it cannot, it is retired, and the agent's next driver starts from the start pack, which
  carries the reports of its recent tasks.
- A driver is retired when its context passes a set share of the model's window, when it has carried more than
  a set number of unrelated tasks, or when it has been warm too long. Mid-task, passing the context threshold
  causes a planned handoff instead, since compaction waits until the task is accepted.
- Anything the agent must keep is written to its memory or its progress note before a driver is retired, so
  retiring loses nothing. A warm driver needs no extra turn for this: an idle agent has reported its task, and a
  waiting one wrote its progress note when it called `wait`.

The thresholds are settings with defaults, measured and tuned in M16.

## Handoff

**decided (owner, 2026-11-11)** Handoff between drivers is seamless. When a driver crashes mid-task, a
compatible driver takes over, even from another provider, and carries on from where the work stood.

**proposed** A handoff moves an agent's work from one driver to another. It runs the same way whatever the
reason, except for whether the old driver can help:

- **Takeover**, when the old driver cannot help. It failed: its process exited, its stream broke, it reported a
  provider error it cannot retry, or it was silent for longer than the stall timeout (longer while a command it
  started is still running). Or the builder replaced it from the agent's view. Or the daemon restarted.
- **Planned**, when the old driver is healthy but should not go on: its context passes the threshold mid-task,
  its task's effort changes and it no longer fits, or the builder disallows its harness or model or narrows the
  agent's range below it.

**proposed** A handoff runs in this order:

1. A planned handoff waits for the current turn to end, then gives the old driver one last turn, with a time
   limit, to bring its progress note up to date and remember what it should. If that turn fails, the handoff
   becomes a takeover.
2. The old driver is fenced off. It is marked Ended or Failed and stopped, with the command it was running in
   the foreground. Each driver reaches the daemon's tools at its own address, and the daemon refuses calls from
   any driver that is not the agent's current one, so a driver that is slow to die changes nothing.
3. The worktree is checkpointed as it stands.
4. The router finds a compatible driver as for any start, skipping a harness and model that already failed on
   this task or are cooling off.
5. The new driver starts from the start pack, which says why the last one stopped and what was in flight: a
   daemon tool call, which the daemon finishes and whose result is in the pack, or a command, whose outcome is
   unknown.
6. The handoff is an event. The new driver's record names the one it replaced and why, and the agent's
   transcript marks the change.

Throughout, the agent keeps its sandbox and its task, and stays Working unless no compatible driver can be
found, in which case it waits for one. Inbox items stay unhandled until a turn on the new driver completes.
Its workers, messages, jobs and held actions belong to the agent, not to the driver, so they carry on
untouched.

**proposed** After three failures in a row on one task, with no turn completed between them, the agent stops
trying and the task is reported failed to its delegator, with each failure.

**proposed** Drivers are child processes of the daemon and do not outlive it. When the daemon restarts, every
driver the log shows as live is marked Failed, warm ones are Ended, and every Working agent is taken over as
above.

## Delegation, the inbox and waiting

**decided (owner, 2026-11-11)** Agents act through delegation as well as on a schedule. Work delegated to an
agent makes it act, and the agent that delegated it can wait for the result.

**decided** Delegation goes down the tree: the builder to the CEO, the CEO to its leads, and a lead or a worker
to its own workers, starting a new one when needed. A report goes back to whoever delegated the task (rule 8 in
`PRODUCT.md`). Leads ask each other for help by message, not by delegation (rule 9).

**proposed** `delegate` gives a task to one of the agent's children, or starts a new worker to take it. The
task records the delegator, the assignee, what is asked, and the effort it needs. The assignee owns it until it
reports. The delegator then accepts the report or sends the task back. Meanwhile the delegator carries on with
other work, or waits for the report.

**proposed** Everything that can make an agent act reaches it through its **inbox**: a task delegated to it, a
message, a report on a task it delegated, a reply from the builder, a decision on one of its held actions, a job
firing. An agent with unhandled items and no running driver gets one. Items are given to a driver between
turns, never mid-turn. An item counts as handled only when the turn it was given in completes, so if the driver
fails first, the item goes to the next driver.

**proposed** An agent waits by calling `wait` with what it waits for, an optional deadline, and its progress
note. It can wait for a job, the report on a task it delegated, a reply to a message it sent, or the decision on
an action it proposed. A wait with no deadline gets the default one, which is a setting. The deadline is a job:
when it fires, the agent is woken and told the wait ran out. Any new inbox item wakes a waiting agent, not only
the one it waits for, so a member of the team can always be reached. Its other waits stay open.

A waiting agent has no running driver. Its driver is Warm and spends nothing. If the wait outlasts the warm
period, the driver retires, and when the agent wakes, it gets a new driver from its start pack.

**proposed** An agent never sits on a task with nothing to wait for. A turn that ends with the task neither
reported nor waited on is followed by another, which asks the agent to carry on, wait or report. After a set
number of such turns in a row, the task is reported failed.

**proposed** Agents act on the organisation through tools the daemon serves to each driver over MCP:
`delegate`, `report`, `message`, `wait`, `schedule`, `cancel_job`, `raise_task`, `update_task`, `write_spec`,
`add_file`, `load_skill`, `remember`. The CEO gets `form_team` and `set_effort` as well, and only the CEO can
`message` the builder. Waiting for the builder's answer is a wait on the reply. Every such call is an event.
This is also where the gate sits: a tool call is an `Action`.

**proposed** In v1 the gate covers the daemon's own tools, network egress, and a deny-list of commands
enforced through a harness's permission hook where the provider has one. It does not ask about every shell
command an agent runs inside its sandbox: the sandbox is what contains those.

## Scheduled jobs

**decided (owner, 2026-11-11)** The daemon runs scheduled jobs of its own. A job can wake an agent, and an agent
can wait for one.

**proposed** A job is a record in the project's log: who scheduled it, when it fires (once at a time, after a
delay, or on a repeat), and what it does then. It does one of three things:

- **Wake an agent** with a note, delivered to its inbox. An agent waiting on the job is woken by it, and an idle
  one starts work.
- **Delegate a task** to an agent: a fresh task from the same description at every firing, as if the job's owner
  had delegated it. "Run the smoke tests every morning" is a repeating job of the CEO that delegates to the QA
  lead.
- **Run one of the daemon's own routines**: end a warm driver's warm period, end a cool-off, end a wait at its
  deadline. Later, scheduled reflection is a repeating job that delegates a reflection task to the CEO.

**proposed** An agent schedules jobs with `schedule`, for itself and for its own children, the agents it may
delegate to. The builder asks the CEO for one, and can cancel any job an agent scheduled. The daemon schedules
its own routines. A repeat shorter than a set minimum is refused, and a repeating job that wakes agents is an
action the gate classifies as spend.

**proposed** One clock in the daemon fires every job, so everything time-based in x100 takes one path: it is in
the log, it survives a restart, and in tests it runs on a fake clock that the test moves by hand. The one timer
that is not a job is stall detection on a running driver, which `x100-drivers` keeps on the same clock. A job
that fell due while the daemon was stopped fires once when it starts, however many times it was due.

A job firing costs nothing. The work it causes is charged as usual.

## Sandbox

**proposed** One sandbox per agent, holding a git worktree of the project on its own branch. Work reaches the
project by merging the branch, which the agent's parent requests and the gate can hold.

**proposed** The sandbox belongs to the agent, not to a driver. Every driver the agent has runs in it, so a
driver that takes over finds the files as the last one left them. Processes the last driver started in the
background, such as a dev server, belong to the sandbox and keep running through a handoff. Destroying the
sandbox stops them.

**decided (owner, 2026-11-11)** Containers or bubblewrap, and it has to perform well.

**proposed** Two backends behind one trait. Bubblewrap is the default on Linux and under WSL: it needs no daemon
and starts in milliseconds. Containers are the default on macOS, where bubblewrap does not exist, and an option
on Linux. Performance is part of the acceptance check: record the time to create a sandbox, the time to start a
command in one, and the overhead on a build, for each backend.

| Option | Isolation | Cost |
|---|---|---|
| Containers (Docker or Podman) | Good, well understood | Needs a container runtime installed |
| OS sandboxing (Landlock and seccomp on Linux, Seatbelt on macOS) | Good, no dependency | Different code per OS, none for Windows |
| MicroVMs | Strongest | Heaviest, Linux-first |

MicroVMs are out of scope for v1.

Network: sandboxes reach the outside only through the vault's proxy. That is what makes the next section true.

## Vault and auth proxy

**proposed** Credentials are stored encrypted on the builder's machine, keyed from the OS keychain.
An agent sees names such as `ref_7f3a`. The sandbox's only route out is an HTTP proxy run by the daemon, which
replaces references in outbound requests with values for hosts the credential is allowed to reach. Everything
a driver emits is scanned for known values before it is stored or shown. Every use is an event.

Known limit to state plainly in the docs: command-line tools that need a credential in their environment
(for example a deploy tool) cannot be served by an HTTP proxy alone. For those, a wrapper run by the daemon
starts the one named command, gives the credential to that child process only, and scrubs the value from its
output, including encoded forms. Its limits:

- A command is allowed by name, so only specific named tools qualify. General runners that execute files an
  agent can edit, such as shells, `make`, `npm` scripts or interpreters, are refused.
- A tool that runs a build step from the project's own config would run that step with the credential, so the
  wrapper refuses while the agent's changes touch that config.
- This narrows the routes to a credential. It is not a second sandbox.

## Gate

**proposed** Two stages. Rules first: a table from action kind and target to a risk level (delete data, deploy
to production, spend, send to people outside, change credentials, force-push). Then, for actions the rules do
not cover, a small model classifies. The builder's "Ask me about" level is compared with the result. Held
actions are events; the app shows them; approval or denial is an event the orchestrator resumes on.

**proposed** A held action does not hold up a driver. The agent is told the action is held and can wait on the
decision. On Approve, the daemon carries out a held tool call itself, or, for a command stopped by a harness's
permission hook, allows that exact command once for that agent. The decision goes to the agent's inbox, so it
reaches the agent even if its driver has changed meanwhile.

## Memory, skills and reflection

**proposed** One git repository per agent under the project, plus one shared skills repository per project.

```
<project>/.x100/
  agents/<agent-id>/memory/*.md     one note per file, with front matter
  agents/<agent-id>/skills/<name>/SKILL.md
  skills/<name>/SKILL.md            shared in the project
  specs/BRD.md  PRD.md  ADD.md
  team.toml                         the team, written from the event log for people to read
```

Skills use the Agent Skills `SKILL.md` layout so that providers' agents can load them unchanged. Versions are
git history plus a `version` field. Reflect now gives the CEO a reflection task: the event log for a span of
work and the repositories, with tools to rewrite memories, edit skills and propose a new `team.toml`. A proposed
team change is a held action, which the builder approves or denies.

**decided (owner, 2026-11-11)** Markdown, git and folders are the base engine and stay supported. In V2 there
will also be a custom storage engine: a special-purpose database, fast in the way DuckDB or SQLite is, that
supports several simultaneous writers with concurrency controls.

**proposed** In v1 there is one engine and no abstraction over engines. What keeps V2 cheap is that no crate
other than `x100-memory` touches these files, so the second engine has one place to go. In v1 the daemon is the
only writer, which is why one writer at a time is enough. The need for several arrives with more than one
machine.

## Evals, trials and the lab

**decided (owner, 2026-11-11)** Reflection is driven and gated by evals. The CEO uses evals, data and checks to
decide which skills and team arrangements to use. Before it commits a change it tests it, for example by
running a lead with and without a skill and comparing what happens. The CEO has its own private environment
for this, and what it does there is visible to the builder.

**proposed** How that works:

- **Trial.** A trial is a record with a proposed change (a skill edit, a new skill, a memory compaction), an
  eval set, two arms (as things are, and with the change), the runs of each arm, their scores, a verdict and a
  decision. Every step is an event.
- **Eval set.** Tasks to run and how to score them. Three sources, in order of trust:
  1. Past tasks from this project's own event log that ended in an accepted result, replayed from the
     checkpoint they started at, scored by the checks that passed then (tests, builds, the gate's record).
  2. Checks the CEO writes for the trial: a script, or a short rubric for a judging model when no script can
     decide.
  3. For memory compaction, probe questions whose answers are in the notes before compaction, asked again after.
- **Lab.** Each run of each arm gets a fresh sandbox made from a checkpoint, and a driver started from the
  agent's start pack with or without the change. Lab drivers are marked as such. Their sandboxes have no route
  to credentials and no outbound writes, and nothing in them is ever merged. Their cost goes to a lab line in
  the ledger with its own cap, inside the project budget, and the router refuses a lab driver past that cap.
- **Scoring.** Checks first: did it pass, how many tool errors, how many turns, how long, how much it cost. A
  judged rubric only where no check exists. Each arm runs more than once, because results vary between runs.
- **Rule.** A change is committed only when the changed arm is no worse on any check and better on at least one
  measure by a set margin, across the repeated runs. Otherwise it is discarded. Either way the trial is kept.
- **Commit.** The proposed change lives on a branch of the memory or skills repository while it is on trial.
  Committing merges it. Discarding deletes the branch. Agents only ever load from the main branch.
- **Seeing it.** Trials appear in the Log, and each agent's view lists the trials that changed its skills or
  memories, with both arms' scores.

In v1, trials gate skills and memories. A team change is still proposed to the builder as a held action, now
with the telemetry that prompted it attached. Trials of team shape come later, since each one means running the
same work twice with two teams.

## Store

**proposed** SQLite per project for the event log and the agents' transcripts, beside the project's files.
Attachments and generated files in a content-addressed directory. The app's own settings in the user's config
directory.

## Protocol

**proposed** JSON-RPC over a Unix domain socket, with one subscription that streams
events after a given sequence number. A client starts by asking for a snapshot, then follows the stream. The
same protocol goes over a network transport later for other devices. The resources it serves are listed in the
next section.

## API resources

**proposed** The resources the design needs. The protocol is JSON-RPC, so a resource is a kind of record that
methods act on (`drivers.list`, `drivers.stop`) and that appears in the snapshot and the event stream. They are
written as paths here. The first four belong to the daemon, the rest to one project.

| Resource | What it is |
|---|---|
| `/machines/` | Computers or VMs that run drivers, each with the harnesses on it and its cap on live drivers. v1 has one. |
| `/harnesses/` | Agent programs x100 runs as drivers, with what each supports and its models at each effort level and price. |
| `/credentials/` | Credentials by reference: the hosts each may reach, the commands that may receive it, the projects that may use it. Values go in and never come out. |
| `/projects/` | Projects, with the sidebar's live state and agent count, and each project's settings: Ask me about, total budget, allowed harnesses and models. |
| `/events` | The subscription: a snapshot, then every later event. The Log tab is this stream, rendered. |
| `/agents/` | The team: role, parent, charter, effort range, a lead's budget, sandbox, state, current task, current driver. |
| `/agents/{id}/inbox` | What has been delivered to the agent and not yet handled. |
| `/agents/{id}/waits` | What the agent is waiting for, each with its deadline. |
| `/agents/{id}/transcript` | The agent's work across all its drivers, in x100's own format, with each handoff marked. Following it is the live view. |
| `/drivers/` | Every driver, live or ended: agent, harness, model, effort, machine, state, context fill, cost, its start pack, the driver it replaced, and why it ended. Stopping one hands its work on. |
| `/tasks/` | Raised and delegated work: what is asked, delegator, assignee, effort needed, status, progress note, report. |
| `/messages/` | Messages between agents, and between the builder and the CEO, which is the chat. |
| `/jobs/` | Scheduled jobs: owner, when and how often, what it does, next firing, state. |
| `/actions/` | Everything agents asked to do, with kind, risk and decision. Held ones take Approve or Deny. |
| `/specs/` | BRD, PRD and ADD, with author, revision and status. |
| `/files/` | Files the project holds outside the code, chat attachments included. |
| `/ledger/` | Every charge: driver, agent, lead, tokens, cost. With the budgets on projects and leads, it makes the Budget tab. |
| `/memories/` | Each agent's notes. |
| `/skills/` | Shared and per-agent skills, with versions. |
| `/trials/` | Reflection's tests: the proposed change, the eval set, both arms' runs and scores, the verdict and the decision. |

Agents reach the same resources through their tools: `delegate`, `raise_task`, `update_task` and `report` act
on `/tasks/`; `message` on `/messages/`; `wait` on an agent's waits; `schedule` and `cancel_job` on `/jobs/`;
`write_spec` on `/specs/`; `add_file` on `/files/`; `remember` on `/memories/`; `load_skill` on `/skills/`; and
the CEO's `form_team` and `set_effort` on `/agents/`. Every tool call is also an entry in `/actions/`.

Later, not v1: devices, for more than one device on a project; apps and their deployed versions, for the
Preview selectors; and the Infra map's providers and services.

**Still missing.** The design implies these but does not settle them. Each has a proposed default:

- **Credentials in the product.** The vault is designed, but nothing lets the builder add a credential, limit
  its hosts, name the commands that may receive it, or choose which projects use it. Proposed: `/credentials/`
  on the daemon as above, granted per project, managed from a Credentials part of Settings.
- **Where the Preview link comes from.** Nothing says how the daemon learns the address of the development
  build. Proposed: a field of `/projects/` that agents set with a tool.
- **Following every project at once.** The subscription follows one project's log, but the sidebar shows every
  project live. Proposed: one subscription with a cursor per project.
- **Memories and skills in the interface.** Reflection changes them, but no view shows them to the builder.
  Proposed: read-only in the agent's view.

**Not resources of their own.** Each of these looks like one, and another already covers it:

- Questions to the builder: a question is a message from the CEO, and waiting for the answer is a wait on the
  reply.
- Starting a worker: delegating to a new worker starts it, so there is one `delegate`, not a tool to start a
  worker and another to give it work.
- The chat: it is the messages between the builder and the CEO.
- The Log: it renders `/events`.
- Handoffs: a handoff is the new driver naming the one it replaced, and why.
- Reports: a report is how a task ends.
- Reflections: Reflect now is a task delegated to the CEO, its changes are commits and events, and a team
  change it proposes is a held action.
- `team.toml`: it describes the same team as `/agents/`. The event log is the truth. `team.toml` is written from
  it for people to read, and a proposed team change carries a new one.
- Settings: they are fields of the project. Lead budgets and effort ranges show in the Team, Budget and Settings
  tabs but live only on `/agents/`.
- Models: a model matters only as something a harness runs at an effort level, so models are listed under their
  harness.
- Sandboxes: there is one per agent, so a sandbox is a field of the agent.

Two look redundant and are not. The inbox looks like a view over messages, tasks, jobs and actions, but it
holds what they do not: which of them an agent has been given and not yet handled. Waits look like jobs, but
most waits are on something other than time; only a wait's deadline is a job.

## Interface

**decided (owner, 2026-11-11)** GPUI. Target platforms are Linux, macOS, and Windows through WSL. Native Windows
is not a target. The table below records why.

| Option | For | Against |
|---|---|---|
| GPUI (Zed's) | GPU-rendered, proven in a large app with terminals and editors in it, fast | Young API, docs thin |
| Iced | Mature, simple model | Text and complex layout less proven at this scale |
| egui | Fastest to build with | Immediate mode fights the polished look on the page |
| Slint | Solid, designer-friendly | Own language, licence terms to check |

GPUI was chosen because the product needs many live terminals and a dense, animated layout, which is what Zed
already does with it. Still verify its state on the three target platforms before building on it, WSL above all,
since a GPU-rendered window under WSL depends on WSLg.

The app needs: a terminal view (for an agent's live output, continuous across a change of driver), Markdown
rendering (specs, chat), a list that updates in place, images, and the hero-style orb only if wanted.

**proposed** Preview in v1 is a link that opens the builder's own browser. Showing a web app inside the window
needs a browser engine, which the page says x100 does not contain. After v1 the choice is between embedding the
operating system's own web view in a separate process for that tab only, and streaming screenshots from a
headless browser.

**proposed** The terminal interface is built with ratatui, on the same `x100-client` crate as the desktop app.
Neither interface holds logic of its own beyond drawing and input.

## Multi-device and remote machines (later)

**proposed** Each machine runs `x100d`. One is the project's home and holds the log. Other machines are runners
that the home daemon starts drivers and sandboxes on. Choosing a machine becomes part of finding a driver, and a
takeover can move an agent to another machine: its sandbox is rebuilt there from its last checkpoint, and work
done since that checkpoint on a machine that is gone is lost. Phones and other computers are clients of the
home daemon. Transport between machines over the builder's own private network. No x100-operated server.

Not decided, and not needed before V2: whether a relay run by the x100 project is acceptable for devices that
cannot reach each other directly. It contradicts "no server" but is what makes a phone work away from home.

## Licence

**proposed** MIT and Apache-2.0 together, as is usual for Rust.

## Code from elsewhere

**decided (owner, 2026-11-11)** x100 shares no code with any other project. It may take concepts from prior
work, and everything it needs is written down in these documents, so they stand on their own.

## Defaults the owner may want to overrule

None of these blocks work. Each is built as written unless the owner says otherwise.

1. The gate does not ask about every shell command inside a sandbox.
2. The delegator, not a classifier, states the effort a task needs.
3. Preview in v1 is a link to the builder's browser.
4. The licence is MIT and Apache-2.0 together.
5. The terminal interface uses ratatui.
6. An agent has one driver at a time. Work that should run in parallel goes to workers.
7. A takeover always rebuilds the new driver's context from x100's own records, never from a provider's own
   resume, even on the same harness.
8. Drivers do not outlive the daemon, so a daemon restart is a takeover for every working agent.
9. After three driver failures in a row on one task, the task is reported failed to its delegator.
10. A compatible driver only has to meet the task's effort, so a takeover may run at a lower effort level than
    the driver it replaces when the task needs no more.

11. A trial runs each arm three times, and a change needs to be no worse on every check and at least a tenth
    better on one measure to be committed.
12. The lab's cap is a tenth of the project budget.
13. Team changes are not put on trial in v1.

One question is left for V2: whether a relay run by the x100 project is acceptable for phones.
