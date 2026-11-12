# How Claude Code and Codex run headless

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
  never reach it. Whether it works under `--bare` is unknown.
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
- **Permission hook.** `item/commandExecution/requestApproval` and `item/fileChange/requestApproval`. Whether
  they fire at all once Codex's own sandbox is off (`danger-full-access`) is unknown, so treat it as partial.
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
- **Proxy.** `CODEX_CA_CERTIFICATE` for the authority. Whether it honours `HTTPS_PROXY` is unknown.
- **Isolation.** A per-driver `CODEX_HOME` with a config x100 writes, `project_doc_max_bytes=0`,
  `sandbox_mode="danger-full-access"`, `approval_policy="never"`, `check_for_update_on_startup=false`.
- **Prices per million tokens** (docs): astra 10 in, 50 out. 6.1-sol 2 and 10. 6.1-terra 1 and 5. luna 0.10 and
  0.50. About double above 272K tokens of input.

## Neither speaks ACP

Adapters exist for both. Not needed for v1.

## What only a real run settles

Free, against a local fake provider that returns 401, 429, 500 and 529: how each harness reports each failure,
that the placeholder key reaches the proxy, and that the proxy and authority settings are honoured.

Paid, each under about a cent, on Haiku 5.5 and gpt-6-luna at low effort, about six runs per harness: two turns
keep context; compaction on request; the permission hook (under `--bare` for Claude, with the sandbox off for
Codex); an MCP tool call with a per-driver token; a clean stop with nothing left running; and M3's own check,
the one-file edit.
