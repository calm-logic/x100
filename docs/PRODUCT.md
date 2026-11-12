# x100 product spec

Status: draft for the first build. Written 2026-11-11 from the owner's brief and everything settled while building
the marketing page in `site/`.

Read with `docs/ARCHITECTURE.md` (how it is built) and `docs/V1_PLAN.md` (what is built first).

## What x100 is

x100 is an agentic management environment: a desktop app that runs teams of coding agents as an organisation.
You talk to one agent per project, the CEO. The CEO plans the work, forms a team of leads, and reports back.
Leads run their own workers. The name is the goal: one builder in effective control of 100 or more agents.

x100 is free and open source.

## Clients

| Client | When |
|---|---|
| Desktop app | MVP |
| Terminal interface (TUI) | MVP |
| Mobile app | V2 |

Every client shows the same projects, chat and tabs, because each is a view onto the same daemon.

## Words

| Word | Meaning |
|---|---|
| Builder | The person using x100. The page says "builder", not "developer". |
| Project | One body of work with one CEO. Projects are separate from each other. |
| Agent | A member of a project's team: the CEO, a lead or a worker. It keeps its role, memories and work while drivers come and go. |
| CEO | The single agent a builder talks to in a project. It owns the outcome. |
| Lead | An agent the CEO puts on the project team, for example Tech, Product or Marketing. Not "chief". |
| Worker | An agent started by a lead or by another worker. The tree can be any depth. |
| Team | The CEO, its leads and all their workers. The CEO "forms" or "chooses" a team. It does not "hire". |
| Effort | How much capability a task needs, and a driver gives: Low, Medium, High or Max. |
| Driver | What does an agent's work right now: a harness running one model at one effort level on one machine. When a driver fails, another takes over. |
| Harness | An agent program x100 runs as a driver: Claude Code, Codex, or a custom harness around an LLM. |
| Model | A provider's LLM that a harness runs, for example a Claude model. |
| Machine | A computer or VM that runs drivers. In v1, the builder's own. |
| Handoff | An agent's work moving from one driver to another, mid-task included. The agent and its work carry on. |
| Delegate | Give a task to an agent below you. It reports back to you. |
| Job | Something x100 does at a set time or on a repeat: wake an agent, or delegate a task to it. |
| Skill | A reusable, versioned instruction set an agent can load. |
| Trial | A test the CEO runs before changing how the team works: the same tasks done with and without the change, compared. |
| Lab | The CEO's private space for running trials. Nothing in it touches the project. |
| Memory | A note an agent keeps between tasks. |

Agents are "it". No agent is "he" or "she", in the product, the documents or the marketing page.

## Rules of the organisation

These are the product's behaviour, not suggestions.

1. Every project has exactly one CEO. Creating a project creates its CEO.
2. The builder talks to the CEO. The CEO answers first, then delegates.
3. The CEO works out the team early, while it talks to the builder about the goal. The builder can direct who is
   on it. There is no fixed roster.
4. The CEO may work alone. A small project may never need a lead.
5. A lead is not started until there is work for it.
6. Each lead is autonomous inside its budget and effort range, and has its own drivers.
7. Leads and workers delegate to workers of their own, starting them as needed.
8. Work reports up: worker to its parent, lead to CEO, CEO to builder.
9. Leads message each other directly. The CEO is not a relay for work between leads.
10. No agent is tied to a driver or a provider. The CEO gives each agent a minimum and a maximum effort. Each
    task runs on the cheapest driver the builder allows that meets the effort the task needs. The same lead may
    be driven by Claude Code for one task and Codex for the next. If its driver fails mid-task, another one,
    from any provider, carries on.
11. The CEO can reshape the team after a piece of work: add a lead, fold one into another, or promote a worker
    to a lead.
12. An agent acts when work is delegated to it, a message reaches it, or a job wakes it. It can wait for any of
    these, and spends nothing while it waits.

## Features

Each feature is tagged **v1** (in the first release), **later** (designed for, not built first) or
**page only** (exists on the marketing page for illustration and is not a requirement).

### The app window

- **v1** A native desktop window with three areas: a projects sidebar, a chat with the CEO, and a main panel.
- **v1** Before the first message in a project, the main panel is hidden. The window shows the CEO and a box
  to write the brief in. After the first message the main panel opens.
- **v1** The brief accepts attached files. Attachments show as previews in the chat.
- **v1** The main panel has tabs, in this order: Team, Specs, Tasks, Files, Budget, Log, Preview, Infra, Settings.
  A tab shows a dot when something in it changed while it was not open.
- **v1** Multiple projects, listed in the sidebar with a live indicator and an agent count. The page's demo shows
  one project only because it is scripted.
- **later** Theming: accent colour, corner radius, sidebar side, density.

### Chat

- **v1** Free-form messages to the CEO. The page's canned prompts are a demo device.
- **v1** The CEO's replies are short and say what it did and what it needs from the builder.
- **v1** The CEO can ask the builder a question and wait for the answer without stopping other work.

### Team

- **v1** Shows the CEO with its status, and each lead as a row: name, current task, progress, the driver its
  task is running on (harness, model and effort), and the lead's effort range.
- **v1** Clicking an agent opens it. The view then shows that agent, a trail back to the CEO, and either the
  agents it runs or, if it has none, what it is working in right now.
- **v1** "What it is working in" is the live output of the agent's driver: its terminal and tool output. When
  the agent changes driver, the output carries on in the same view, with the change marked.
- **v1** An agent's view also shows the drivers it has had and why each ended, what it is waiting for, and the
  jobs it has scheduled.
- **v1** The Team tab can list drivers instead of agents: every live and warm driver in the project, with its
  agent, harness, model, effort, machine, state, how full its context is, and what it has cost.
- **v1** There is one chat per project, with the CEO. The builder does not message leads or workers. A request
  for one of them goes to the CEO, which passes it on. Opening an agent in the Team tab is for watching only.
- **later** Richer per-agent views: a live browser view for an agent driving a browser, an editor view with the
  file and line being changed.
- **v1** Rows update in place. Nothing blinks when unrelated state changes.

### Waiting and scheduled jobs

- **v1** An agent can wait: for a worker's report, a reply, the builder's answer, a decision on a held action,
  or a set time. A waiting agent spends nothing, and anything new for it wakes it.
- **v1** Agents schedule jobs: wake themselves or an agent below them at a time or on a repeat, or delegate a
  task on a repeat ("run the smoke tests every morning"). The builder asks the CEO for one.
- **v1** The builder can cancel any job from the agent's view. Jobs scheduled and fired show in the Log.

### Specs

- **v1** Three documents per project: BRD (business requirements, written by the CEO), PRD (product
  requirements, written by the Product lead or the CEO) and ADD (architecture design, written by the Tech lead
  or the CEO).
- **v1** Each has an author, a revision number and a status of Draft or Approved. Agents revise them as
  decisions change. They are stored as Markdown in the project.
- **later** The builder edits a spec directly and agents pick up the change.

### Tasks

- **v1** A flat list: an id, a title with a live dot while an agent is on it, a status,
  the part of the product it belongs to, and who raised it.
- **v1** Statuses, in order: New idea, In progress, In review, Promoting, Done.
- **v1** Agents and the builder can both raise tasks. A QA worker can raise a bug with a screenshot.
- **later** Board view, filters, and linking tasks to a map of the product's parts.

### Files

- **v1** Every file the project holds outside the code: what the builder attached, reference images agents
  made or found, bug screenshots, generated assets. Each shows a preview, its kind, who added it and its size.
- **v1** The builder can remove a file to save space. Removal is logged.

### Budget

- **v1** A budget is a total for one project, not a monthly fee. It is the most the CEO may spend on models for
  the whole project.
- **v1** Shows spend against the project budget, spend per agent, and tokens used.
- **v1** Nothing new starts once the project budget is reached, and the CEO tells the builder.
- **later** An estimate of what per-task model choice saved against running everything at maximum effort.
  The page's figure for this is invented.
- **v1** Every lead has a budget of its own, taken out of the project's total. The CEO sets it, and the builder
  can set or change it directly. A lead's workers spend from that lead's budget. The tab shows each lead's
  spend against its own budget.
- **v1** A lead that reaches its budget stops starting work and tells the CEO.

### Log

- **v1** Every action by every agent, newest first: who, when, what. Includes driver choices and handoffs, jobs
  scheduled and fired, workers started, files added or removed, deploys, and settings the builder changed.

### Preview

- **later** A view of the running app the project is building, with two selectors: which app (a project may
  hold several, for example a website and a dashboard) and which version (In development, Staging, Production).
  Versions that are not deployed yet are disabled.
- **v1** A link that opens the current development build in the builder's own browser.

### Infra

- **later** A live map of the infrastructure the project uses: providers as regions, services as tiles,
  connections between them, and a live metric per service. The page shows Cloudflare, Neon, AWS and Stripe as
  an example. Provisioning itself is done by agents through the builder's own cloud accounts.

### Settings (per project)

- **v1** Ask me about: Nothing, High risk only, Medium and up, Every decision.
- **v1** Project budget, in total, and a budget for each lead.
- **v1** Harnesses and models x100 may use: a list the builder turns on and off. Turning one off hands the work
  of any driver using it to another driver.
- **v1** An effort range per lead, set by the CEO and adjustable by the builder.
- **v1** Credentials: add one, limit the hosts it may reach and the commands that may receive it, and choose
  which projects may use it. A value can be entered and replaced, never read back.
- **later** Reflection schedule: every 6 hours, daily, or off.

### Bring your own tools, agents, stack, O/S

- **v1** x100 runs the agent harnesses the builder already has, as drivers. First targets are Claude Code and
  Codex. Others, including a custom harness around any LLM, plug in through one harness interface.
- **v1** Agents use the builder's own tools and accounts: git hosting, cloud, databases.
- **v1** Runs on the builder's operating system. See `ARCHITECTURE.md` for which ones come first.
- **page only** The fifty-odd logos are illustrative. They are not a list of integrations to build.

### Drivers and handoff

- **v1** x100 picks the driver for all work itself: the cheapest one the builder allows that meets the effort
  the work needs. Neither the builder nor the CEO picks one by hand.
- **v1** When a driver fails mid-task (it crashes, hangs, or its provider fails), x100 starts a compatible one,
  from another provider if need be, and it carries on from where the work stood. The agent, its task, its files
  and its history are unchanged, and the Log says what happened.
- **v1** x100 also hands work to a new driver on purpose: when a driver's context fills up mid-task, when a
  task's effort changes, or when the builder turns off the driver's harness or model.
- **v1** The builder can replace an agent's driver from the agent's view. The work hands off the same way.

### Control everything from anywhere

- **later** The same project, in step, on more than one device. A message typed on a phone appears on the laptop
  at once.
- **later** Run drivers on any machine the builder has access to: a build server, a cloud GPU, another desktop,
  a VM. A Machines list shows which drivers run where, and a handoff can move an agent's work from one machine
  to another.
- **v1** One machine. The design must not block the above: the app is a client of a local daemon from day one.

### Stay in the loop, or out of it

- **v1** Classifiers label every action an agent is about to take with a kind and a risk level.
- **v1** The "Ask me about" setting decides which actions wait for the builder. The rest go through.
- **v1** A waiting action shows what it is, why it was flagged, and Approve and Deny.

### Strict security

- **v1** Sandbox isolation. Every agent runs in its own sandbox with its own files, processes and network
  rules. A destructive command affects only the sandbox.
- **v1** Auth proxy. Agents hold references to credentials, never the values. The real credential is added to
  the request outside the sandbox. A secret never enters an agent's context, transcript or log.
- **v1** High-risk action detection. As above: flagged, paused, and resumed or dropped on the builder's answer.

### Automatic reflection and evolution

- **v1** Each agent's memories and skills are stored in a repository of their own, in a documented format.
- **v1** A skill written once can be loaded by any agent in the project.
- **v1** Reflect now: on request, the CEO reads the log and telemetry of the last piece of work across all leads
  and workers, then compacts memories, improves existing skills, writes new ones, and proposes changes to the team.
- **v1** Reflection is driven and gated by evidence. The CEO does not commit a change because it seems better.
  It runs a trial first: for example, the same tasks given to a lead with a skill and without it, compared on
  checks, cost and time. It commits the change only if the trial supports it, and drops it otherwise.
- **v1** The CEO has a lab for this: a private environment where it can try skills, memories and ways of working
  against real past tasks from the project. Nothing done in the lab reaches the project's code, files or
  accounts, and lab spend has its own cap inside the project budget.
- **v1** Trials are visible. The builder can see each one: what was proposed, what it was tested on, how each
  side scored, and whether it was committed or dropped. The CEO does this in the background without asking,
  but never out of sight.
- **later** Reflection on a schedule, in the background.
- **later** Sharing a skill across projects. Who approves a share is open.
- **later** The CEO applies team changes itself. In v1 it proposes them, with the evidence from its telemetry,
  and the builder accepts.
- **later** Trials of team shape: the same work run by two different teams and compared.

### Maximum performance

- **v1** The interface and the backend are both Rust. There is no browser engine in the app's own interface.
- **v1** Measured, published figures for memory and CPU with a stated workload.
- **page only** The comparison with Orca and T3 Code. Those numbers are invented and must be replaced with
  measurements, or the comparison removed, before the page is published.

### Free, forever. Make it yours

- **v1** The source is public on GitHub under an open-source licence.
- **later** Settings for appearance. Anything further is done by forking.

## What the page shows that is not a requirement

- The Fernleaf scenario, its tasks, file names, costs, timings and CEO wording.
- File paths such as `agents/tech/memories/`, `shared/skills/`, `ui/theme.rs`, and skill names such as `deploy`.
- The worker names Backend, Frontend, QA, Research, Writer, Designer, Copywriter.
- The security examples (`ref_7f3a`, the Stripe and Postgres commands).
- All figures: gauge values, savings, token counts, "212 tests".

Treat these as illustrations of behaviour. The behaviour is the requirement.

## Open questions

Product decisions only the owner can make. Technical decisions are in `ARCHITECTURE.md`.

1. **Which roles exist.** Can the CEO invent any lead role, or does it choose from a list the builder can edit?
   Recommended: any role, described in a short charter the builder can read.
2. **What reflection may change without asking.** Recommended for v1: memories and skills freely, team shape
   only with the builder's agreement.
3. **Sharing skills across projects.** What is shared, and who approves it.
4. **Name of the category.** The page title and FAQ still say "Agency Management Environment". The first section
   says "agentic management environment". Pick one.
