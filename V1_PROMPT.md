# Prompt: build x100 v1

You are the orchestrator for the first build of x100. You plan, brief, verify and decide. Subagents write the
code. Read this whole prompt, then `CLAUDE.md`, then the three documents in `docs/` in the order `CLAUDE.md`
gives, before you do anything else.

## The goal

Take x100 from nothing to v1 as `docs/V1_PLAN.md` defines it: every milestone built and its check passed, and
the fifteen "v1 is done" conditions at the top of the plan true with evidence.

## Before the first milestone

1. Read `docs/ARCHITECTURE.md` critically, against `docs/PRODUCT.md` and `docs/V1_PLAN.md`. It was extended late
   and has not had an end-to-end review. Look for contradictions between sections, between the state machines
   and the prose, between the crate list and the milestones, and between the three documents. Fix what you find
   in the documents before any code depends on it. You may change anything marked **proposed**. Anything marked
   **decided** goes to the owner.
2. Check the toolchain against what the design assumes: the Rust version GPUI needs, that bubblewrap and Docker
   work for an unprivileged user here, and how Claude Code and Codex are run headless today. Record what you
   find where the documents assumed otherwise.
3. Create `docs/STATUS.md` (see "Keeping state" below).

## How to run a milestone

For each milestone in the plan, in dependency order:

1. **Brief.** Write the subagent a brief that stands on its own, since it starts with no context. Give it: the
   goal in a sentence; the files to read (`CLAUDE.md`, the relevant sections of the three documents, and the
   crates it builds on) instead of retyping them; the crates it owns and that it may edit nothing else; the
   check it must pass, word for word from the plan; and that it must show the output of that check.
2. **Model.** Use the Agent tool with `model: "opus"` or `model: "sonnet"` as the plan's table says. Use Opus
   for anything the plan marks Opus, and for any Sonnet milestone that has failed its check twice.
3. **Verify.** When it reports, do not take its word. Run its check yourself. Run `cargo build`, `cargo test`,
   `cargo clippy -- -D warnings` and `cargo fmt --check` on the whole workspace. Read the diff: look for work
   outside its crates, tests that assert nothing, checks that were weakened to pass, dead code, and
   compatibility shims.
4. **Fix or accept.** If something fails, send a fresh subagent with the original brief, what was built, and
   exactly what failed. Accept only when everything passes on your own run.
5. **Commit.** One commit per accepted milestone on `main`, with a message that says what now works.
6. **Record.** Update `docs/STATUS.md`.

Rules for the whole build:

- One subagent at a time, except the parallel pairs the plan marks. Parallel subagents own disjoint crates.
- `x100-core` and `x100-proto` are the contract. After M1 and M2 are accepted, only you change them, and you
  re-run every dependent crate's tests when you do.
- A milestone that is too large for one subagent to finish well may be split. Split along crate or module
  lines, give each part its own check, and keep the plan's check for the whole. Orchestration (M6), the real
  harnesses (M3) and the desktop app (M15) are the likely ones.
- If a milestone shows the design is wrong, change the design: update the documents and the code together,
  delete the old form, and note it in `docs/STATUS.md`. Do not build around a flaw.
- Do not let subagents add abstractions for needs the plan does not have, or keep old code paths alive.
- Do the first task of every milestone that adopts a library: confirm the library does what the design assumes
  on the target platforms, before building on it.
- S1, the GPUI spike, decides whether the desktop app can be built as designed. If its report says it cannot,
  stop and bring that to the owner before M15. The terminal interface does not depend on it.

## What you can and cannot verify here

- This machine is Windows with WSL2. You can verify Linux behaviour and behaviour under WSL.
- There is no Mac. macOS can only be verified by hosted CI once pushing is allowed. Until then, say "not
  verified on macOS" wherever it applies. Do not claim it.
- Everything is testable without a paid model, through the scripted fake harness and the fake clock. Use them
  for all routine checks.

## What needs the owner

Ask, then carry on with whatever does not depend on the answer:

- **Pushing.** Keep all work local until the owner says you may push to `origin`. Ask once, when M0 is
  accepted. Until then "CI is green" means the four commands pass locally and the CI file is written.
- **Real-provider tests.** Several checks run Claude Code and Codex for real, which costs money. Before the
  first such run, tell the owner which checks need it and roughly how many runs, and wait for a yes.
- **Anything marked decided** in the documents.
- **A failed spike** or a milestone you cannot get to pass after three rounds.

Do not ask about things the documents already answer or mark as proposed defaults. Decide and record it.

## Keeping state

This build will outlast your context. `docs/STATUS.md` is how you, or a session that replaces you, pick it up.
Keep it short and current:

- which milestones are accepted, each with the commit and the check output that proved it;
- what is in progress, and with which subagent;
- decisions you made that changed a document, with the reason;
- what could not be verified and why;
- questions waiting on the owner.

Update it every time a milestone is accepted or a decision is made. When you resume after a break, read it and
`git log` before anything else.

## Reporting to the owner

At the end of each milestone, a few lines: what now works, what you ran to prove it, anything that fell short,
and what is next. Lead with anything that needs a decision. Do not report a subagent's claim as a fact; report
what you observed.

## Done

v1 is done when M17's check passes: each of the fifteen conditions at the top of `docs/V1_PLAN.md`, with
evidence you produced yourself, and a plain list of anything that holds only on some platforms or was not
verified.
