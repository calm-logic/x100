# How Claude Code and Codex are run

Status: research for M3, 2026-11-11. From the installed versions (Claude Code 2.1.322, Codex 0.167.1), their
help, the Claude Agent SDK's types (`@anthropic-ai/claude-agent-sdk` 0.3.322, `sdk.d.ts`), the schema Codex
generates (`codex app-server generate-json-schema`), and official documentation. No model was called. Each
point is **installed** (seen in the installed version), **docs** (documentation only) or **unknown**. The
unknowns are what M3's real runs settle. Delete this file's unknowns as they are settled.

## Claude Code

- **Run.** `claude -p --input-format stream-json --output-format stream-json --verbose`. JSON lines on stdin
  and stdout, alive until stdin closes. This is what the Agent SDK spawns. (installed, docs)
- **Stream.** One JSON object per line with `type` and `subtype`. Text and tool calls are blocks in `assistant`
  messages. Tool results are `user` messages with `tool_result` blocks. `result` ends a turn, exactly one per
  turn, with `usage`, `total_cost_usd`, `modelUsage`, `is_error` and `terminal_reason`. `system/init` lists
  model, tools, MCP status and `capabilities`. Also `system/api_retry`, `system/compact_boundary`,
  `rate_limit_event`. The schema is the `SDKMessage` union in `sdk.d.ts`. There is no published wire spec.
- **Control.** `control_request` and `control_response` lines in both directions: `initialize`, `interrupt`,
  `get_context_usage`, `list_models`, `can_use_tool`, hook callbacks.
- **Further turns.** Yes: write another `user` line. (docs)
- **Cost and context.** `total_cost_usd` is a running total at list price, so a turn's cost is the difference.
  Context fill from `get_context_usage` or `modelUsage[model].contextWindow`.
- **Compaction.** Send `/compact` as a user message. It auto-compacts unless `DISABLE_AUTO_COMPACT=1`. (docs)
- **Permission hook.** `--permission-prompt-tool stdio` sends each call needing approval as a `can_use_tool`
  control request, answered allow or deny. The turn pauses while it is pending. Calls a rule already allows
  never reach it. It works under `--bare` (settled against the fake provider).
- **MCP for one run.** `--mcp-config <file-or-json> --strict-mcp-config`. Server types: stdio, http, sse, ws.
  No Unix socket. Tools appear as `mcp__x100__<tool>`.
- **Model and effort.** `--model`, `--effort low|medium|high|xhigh|max`. `CLAUDE_CODE_EFFORT_LEVEL` overrides
  the flag, so drivers get a clean environment.
- **Stop.** `interrupt` ends the turn. Closing stdin ends the process. SIGTERM exits 143 and kills the process
  trees of running Bash commands. Whether stdio MCP children are always reaped is unknown: kill the group.
- **Provider failures.** Error categories on `assistant.error` and `api_retry.error`. Provider or account:
  `authentication_failed`, `billing_error`, `rate_limit`, `overloaded`, `server_error`, `account_on_hold`.
  Run or config: `invalid_request`, `max_output_tokens`, `model_not_found`. It retries ten times itself
  (`CLAUDE_CODE_MAX_RETRIES`) before a turn fails with `terminal_reason: "api_error"`.
- **Login.** `ANTHROPIC_API_KEY` (sent as `X-Api-Key`), `ANTHROPIC_AUTH_TOKEN` (Bearer),
  `CLAUDE_CODE_OAUTH_TOKEN` (a subscription token from `claude setup-token`), or the stored login. `--bare`
  reads only the API key. Anthropic's terms do not let third-party products offer claude.ai login without
  approval.
- **Proxy.** Honours `HTTPS_PROXY`, takes extra authorities from `NODE_EXTRA_CA_CERTS`, and `ANTHROPIC_BASE_URL`
  moves the base URL. Main host `api.anthropic.com`. `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` cuts the
  optional ones.
- **Isolation.** `--bare`, `--setting-sources ""`, `--strict-mcp-config`, a per-driver `CLAUDE_CONFIG_DIR`,
  `DISABLE_AUTOUPDATER=1`. The CLI updates itself often, so record the version and feature-detect from
  `system/init.capabilities`.
- **Prices per million tokens** (docs): Fable 5.2 8 in, 40 out. Opus 5.5 4 and 20. Sonnet 5.5 2 and 10.
  Haiku 5.5 0.10 and 0.50, five times that above 100K tokens of prompt. Cache writes 1.25 times input for five
  minutes, twice for an hour. Take the live list from `list_models` and the pricing page when building the
  table.

## Codex

- **Run.** `codex app-server --listen stdio://`. JSON-RPC 2.0 as JSON lines. `initialize`, `initialized`,
  `thread/start`, `turn/start`. Marked experimental, and used by Codex's own IDE clients. `codex exec --json`
  is one turn per process with string-only errors, so it is not the base for a driver. (installed)
- **Stream.** `item/agentMessage/delta`, `item/started` and `item/completed` for commands, file changes and MCP
  calls, `thread/tokenUsage/updated`, `turn/completed` with a status and `codexErrorInfo`. The schema is
  generated for the exact installed version.
- **Further turns.** Yes: `turn/start` again on the thread. `turn/interrupt` cancels.
- **Cost and context.** Tokens only, no cost, so x100 prices them. `modelContextWindow` gives the window.
- **Compaction.** `thread/compact/start`. Auto-compaction is set by `model_auto_compact_token_limit`.
- **Permission hook.** `item/commandExecution/requestApproval` and `item/fileChange/requestApproval`. With
  Codex's own sandbox off they fire only when `approvalPolicy: "untrusted"` is sent on `thread/start`. The
  config file refuses that value. A denial is `decline` and carries no reason.
- **MCP for one run.** `-c mcp_servers.x100.url=...` with `bearer_token_env_var`, or a per-driver `CODEX_HOME`.
  stdio and HTTP. No Unix socket. Tool timeout defaults to 60 seconds.
- **Model and effort.** On `thread/start` and `turn/start`. Efforts low to max, plus `ultra`, which starts
  sub-agents of its own and is not offered by x100. Live list from `model/list`.
- **Stop.** `turn/interrupt`, close stdin, then SIGTERM. What it leaves running is unknown: kill the group.
- **Provider failures.** `codexErrorInfo`. Provider: `serverOverloaded`, `internalServerError`,
  `rateLimitExceeded`, `usageLimitExceeded`, `unauthorized`, `httpConnectionFailed`,
  `responseStreamDisconnected`. Run: `badRequest`, `contextWindowExceeded`, `sandboxError`, `modelNotFound`.
  `error{willRetry:true}` means it is still retrying.
- **Login.** Cleanest for a placeholder: a custom provider in `config.toml` with `base_url` and `env_key`.
  ChatGPT login is OAuth with refresh, not a static token.
- **Proxy.** `CODEX_CA_CERTIFICATE` for the authority. It honours `HTTPS_PROXY`. With its optional features
  on it reaches for chatgpt.com and github.com. With x100's list turned off it reaches for nothing else.
- **Isolation.** A per-driver `CODEX_HOME` with a config x100 writes, `project_doc_max_bytes=0`,
  `sandbox_mode="danger-full-access"`, `approval_policy="never"`, `check_for_update_on_startup=false`.
- **Prices per million tokens** (docs): astra 10 in, 50 out. 6.1-sol 2 and 10. 6.1-terra 1 and 5. luna 0.10 and
  0.50. About double above 272K tokens of input.

## Settled by M3 against a local fake provider

The real binaries, run with a dummy key and their base URL on localhost. No provider was called.

- Further turns keep context, and compaction on request works, for both.
- Stopping either leaves nothing in its process group, a command running in Codex's PTY included.
- The key reaches the provider as `x-api-key` (Claude Code) and `Authorization: Bearer` (Codex).
- Failures, with each harness's own retries set to 1. Claude Code: 401 `authentication_failed`, 429
  `rate_limit`, 500 and 529 `server_error`. Codex: 401 `httpConnectionFailed{401}`, 429
  `responseTooManyFailedAttempts{429}`, 500 `internalServerError`, 529 `httpConnectionFailed{529}`, so Codex is
  classified by the HTTP status inside the variant.
- Claude Code under `--bare` has only Bash, Edit and Read. Its reported cost matches the price table for
  Haiku 5.5.
- Codex on gpt-6-luna gives the model one free-form `exec` tool that runs JavaScript, through which it reaches
  commands, patches and MCP tools. It reports a window of 258,400 tokens.

Still unknown: the certificate-authority settings (they need M11's proxy), reaping of stdio MCP children (x100
uses HTTP MCP), and everything on the paid list below.

## MCP, as each harness speaks it (seen by M6b)

Claude Code 2.1.322 speaks MCP `2026-07-28`, the version without `initialize`: it sends `server/discover`, then
`tools/list` and `tools/call`. Codex 0.167.1 speaks `2025-06-18` with `initialize`. Both send the bearer token
and accept plain JSON answers. The daemon serves both with the `rmcp` crate.

## Neither speaks ACP

Adapters exist for both. Not needed for v1.

## What only a real run settles

Free, against a local fake provider that returns 401, 429, 500 and 529: how each harness reports each failure,
that the placeholder key reaches the proxy, and that the proxy and authority settings are honoured.

Paid, each under about a cent, on Haiku 5.5 and gpt-6-luna at low effort, about six runs per harness: two turns
keep context; compaction on request; the permission hook (under `--bare` for Claude, with the sandbox off for
Codex); an MCP tool call with a per-driver token; a clean stop with nothing left running; and M3's own check,
the one-file edit.

# As terminal programs, on local logins

Research for M3T, 2026-11-12. Both real programs were run in a pseudo-terminal against the fake provider,
inside a sandbox with no network, with dummy logins. No provider and no subscription was used. Scratch runs
with recorded terminals are under the session scratchpad in `m3t-research/`.

Against the fake provider both programs, in a terminal: made a one-file edit, called a daemon tool over MCP
with its bearer token, took a second turn that kept context, compacted on `/compact`, and stopped with
nothing left running.

## Claude Code in a terminal

- **Start.** `claude --model M --settings FILE --setting-sources "" --strict-mcp-config --mcp-config FILE
  --append-system-prompt-file PACK --permission-mode bypassPermissions "first prompt"`, with a clean
  environment and `DISABLE_AUTOUPDATER`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `DISABLE_AUTO_COMPACT`,
  `CLAUDE_CODE_MAX_RETRIES=1`, `CLAUDE_CODE_DISABLE_CLAUDE_MDS`, `CLAUDE_CODE_DISABLE_AUTO_MEMORY`. `--bare`
  cannot be used: it reads no subscription login. The start pack goes in the appended system prompt (41 KB
  arrived whole), not in a paste, which Claude treats as possibly untrusted.
- **First-run screens** are avoided by seeding `.claude.json` in the config directory with
  `hasCompletedOnboarding`, a theme, and `hasTrustDialogAccepted` for the worktree. `settings` needs
  `skipDangerousModePermissionPrompt`.
- **A further turn.** One write of a bracketed paste, then Enter as a separate write about 250 ms later. A
  prompt typed mid-turn is queued. Esc interrupts.
- **Turn end.** The `Stop` hook, with the last message. `StopFailure` on an error, with a category
  (`rate_limit`, `authentication_failed`, `server_error`). An interrupt fires neither. Hooks can be of type
  `http`, so the daemon is the hook.
- **State on the terminal.** Claude asks `OSC 7501 ?`, and once the terminal answers it reports
  `state=idle|working|done|blocked|error` with a kind. This covers the interrupt and serves as a watchdog.
- **Usage.** On `Stop`, the rows added to the session file named in the hook carry each request's tokens. The
  status line command receives cumulative cost and context fill, but runs only now and then.
- **Tools.** HTTP MCP with a bearer header. In bypass mode the daemon's tools never prompt.
- **Gate.** A `PreToolUse` hook that denies reaches the model with its reason. A `PermissionRequest` hook can
  allow with no dialog drawn.
- **Stop.** SIGTERM or closing the terminal. Background commands run in sessions of their own, so stopping
  must use the sandbox's process namespace, not the process group.
- **Login.** On Linux a file in the config directory, rewritten by rename when the token refreshes. On macOS
  the Keychain, keyed to the config directory. A fresh config directory holding only a login file started as
  a subscriber.
- **Terminal.** Alternate screen, mouse and focus reporting, bracketed paste. It queries the terminal (device
  attributes, version, keyboard) and expects answers.

## Codex in a terminal

- **Preferred: the app-server with the real terminal attached.** The daemon runs `codex app-server --listen
  unix://PATH` on the builder's login and drives it with M3's JSON-RPC code. It starts `codex --remote
  unix://PATH` in the pseudo-terminal for the builder to watch. A second client that resumed the terminal's
  thread and started a turn got the structured events, and the terminal drew the turn. Not yet tried: the
  daemon creating the thread first and the terminal attaching to it.
- **Fallback: the terminal alone.** `codex --no-daemon --dangerously-bypass-approvals-and-sandbox "first
  prompt"` with a per-driver `CODEX_HOME`. The pack goes in `developer_instructions`. Hooks in `hooks.json`,
  trusted by a hash in the config: `Stop`, `Interrupt`, and `PostModelRequest` with each request's tokens and
  its failure. There is no hook for a failed turn's end. It reports `OSC 7501` states unasked.
- **A further turn.** Paste, then Enter more than 120 ms later. Enter mid-turn steers the running turn.
- **Extra request.** The terminal asks the model for a thread title once per thread.
- **Login.** `auth.json` in `CODEX_HOME`, on macOS too, rewritten by rename on refresh. A read-only file fails.
  With a ChatGPT login the terminal needs chatgpt.com reachable at start.
- **Compaction.** `/compact` works. Automatic compaction can only be pushed away with a high limit.

## What the other tools do

- **herdr.** A terminal multiplexer. It types the agent's command into a shell, pastes prompts, and reads state
  from the screen and title.
- **Orca.** Terminals, with hooks that post to a local port and `OSC 7501` as the fallback. Trust entries are
  written ahead.
- **T3 Code.** No terminals for agents. Claude through the Agent SDK, Codex through `codex app-server`, both on
  the local login.

## Terms, as read on 2026-11-12

- **Anthropic.** Its legal page says third-party developers may not offer Claude.ai login in their own
  applications or collect, store or intermediate its credentials, and also that this does not prevent an end
  user from signing in to the unmodified Claude Code binary with their own subscription. So: running the
  unmodified program on the builder's own login looks allowed. x100 reading or holding the token is not.
  Several automated sessions at once on one subscription is unclear, and the risk is throttling.
- **OpenAI.** Its app-server page says a local or open-source application using app-server authentication can
  continue to, and that it was never permitted for commercial or hosted services. Heavy automation is unclear.

## Corrected by M3T, which built the terminal drivers

- `SessionStart` never reaches an `http` hook, so readiness comes from `OSC 7501 idle`. The first turn is typed
  like the rest, not given as an argument.
- Claude's session file lags the `Stop` hook. The driver re-reads it until the last message appears.
- The answer to Claude's `OSC 7501` probe must come before the device-attributes answer from the same read.
- `codex resume` refuses a thread not yet stored, so the driver writes one note into the thread and names it
  before attaching the terminal. Naming it also stops the extra title request.
- Codex's terminal fetches an announcement from raw.githubusercontent.com at every start, with no setting.
- One login is shared by links, as STATUS.md "From M3T" records. Settled on the owner's real logins.

## Open, and only a real login settles it

- **Sharing one login among many drivers.** Both programs rewrite the login file when the token refreshes. A
  copy per driver would go stale when another driver refreshes, and a read-only or single-file mount breaks
  the rewrite. On macOS Claude's login is not a file at all. The likely answer is that terminal drivers use
  the builder's own config directory, with everything of the builder's setup switched off by flags. M3T
  settles it with a real run.
- Whether the proxy must pass each provider's login hosts untouched, since ending TLS there would show x100
  the token.
- The screens for a usage limit and an expired login.
