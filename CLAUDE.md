# x100

x100 is an agentic management environment: a Rust desktop app and terminal interface that run teams of coding
agents as an organisation. A builder talks to one CEO agent per project; the CEO forms a team of leads; leads
run workers. It is free and open source.

Nothing is built yet. This repository holds the design and will hold the code.

## Read these first, in this order

1. `docs/PRODUCT.md`: what x100 does, feature by feature, and what is in v1.
2. `docs/ARCHITECTURE.md`: how it is built. Each choice is marked **decided** (the owner's) or **proposed**
   (a default that may be changed).
3. `docs/V1_PLAN.md`: the milestones, who builds each, and the check each must pass.

`V1_PROMPT.md` is the brief for the session that orchestrates the build.

## What is where

| Path | What |
|---|---|
| `docs/` | The three design documents. They are the source of truth and change with the code. |
| `crates/` | The Cargo workspace, once M0 creates it. One crate per area in `ARCHITECTURE.md`. |
| `site/` | The marketing page. It is a separate, private repository nested here and ignored by this one. |
| `REEL.md` | A brief for the marketing reel. Belongs with the site. Ignored by this repository. |

Do not edit anything under `site/` or `REEL.md` as part of building x100. Do not use the marketing page as a
specification: its scenario, numbers, file names and comparisons are illustrations. `PRODUCT.md` says which of
its features are real requirements.

## How work is done here

- This is a new codebase with no users. Nothing is kept for compatibility. When a design changes, change the
  code and the document together and delete the old form.
- Large work is orchestrated: one session plans, briefs and verifies, and subagents write the code. The plan
  names Opus or Sonnet for each milestone.
- One subagent at a time, except where the plan marks a parallel pair with disjoint crates.
- A subagent's "done" is a claim. The orchestrator re-runs the milestone's check itself, plus `cargo build`,
  `cargo test`, `cargo clippy -- -D warnings` and `cargo fmt --check` for the whole workspace, before moving on.
- Report what was observed. If a check was not run, or could not be run on this machine, say so.
- Prefer existing open-source crates and tools to writing infrastructure. Before adopting one, confirm it does
  what the design assumes on the target platforms.
- Agents in x100 never hold credential values. The same goes for you: do not read `.env` files, tokens or keys.
- x100 shares no code with any other project.
- Prose in documents and comments is plain and direct. No em dashes. Comments say why, not what.

## This machine

- Windows with WSL2 (Ubuntu). Shell is zsh: unquoted variables are not word-split, and `path` is special.
- Installed: Rust 1.88, git, bubblewrap, Docker, Node 22 through `nvm`, the Claude Code and Codex command lines,
  and the GitHub CLI.
- There is no Mac here. Anything that must hold on macOS can only be checked by hosted CI.
- A GPU-rendered window under WSL depends on WSLg. Whether GPUI runs acceptably there is what spike S1 finds out.
- Other projects run dev servers on this machine. Never kill a process by port number or by name pattern.
  Stop only what you started.

## Needs the owner's say-so

- Pushing to a remote, creating or deleting repositories, and anything else that leaves this machine.
- Tests that call a real model provider, since they cost money. Everything else runs on the scripted fake
  harness and the fake clock.
- Changing anything marked **decided** in the documents.
- Stopping or redirecting the build because a spike failed (S1 above all).

## Repositories

- Code and documents: `calm-logic/x100` on GitHub, public. Remote `origin` of this directory.
- Marketing page: `calm-logic/x100-site` on GitHub, private. Remote `origin` of `site/`.
- Use `ssh://git@github.com/<owner>/<repo>.git` remotes. The scp-style form is rewritten to HTTPS on this
  machine and fails.
