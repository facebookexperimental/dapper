# {{program}} mcp — MCP Server for AI Agents

Start an MCP (Model Context Protocol) server on stdin/stdout that exposes debugging tools to AI agents like Claude Code. The server connects to an active Dapper proxy session and translates MCP tool calls into DAP commands.

```bash
{{program}} mcp
```

For session targeting (`--scope-id`, `--control-port`, ambiguity rules) see `{{program}} help sessions`.

## Toolsets

`--toolset` selects a builtin grouping of tools that the MCP server exposes. The table below is generated from `BuiltinToolset::tools()` at render time, so it always reflects what the running binary actually offers; the `*(default)*` marker is derived from the enum's `#[default]` attribute.

{{toolset_table}}

```bash
{{program}} mcp --toolset=full
```

The `raw` toolset is the escape hatch for DAP commands that the typed tools don't expose. Prefer `standard`/`full` so the agent sees self-describing tool schemas; reach for `raw` only when you genuinely need an adapter-specific request.

`sessions` is exposed by the MCP handler **regardless of `--toolset`** — it isn't in any `BuiltinToolset::tools()` definition (so it doesn't appear as a row above), but the handler keeps it available everywhere. `capabilities` shows up in `minimal`/`standard`/`full` per the table above and is *also* kept available in `raw`. Agents should still prefer the `--scope-id` / `--control-port` plumbing over enumerating sessions in an MCP loop.

## Custom tool selection

Instead of a builtin toolset, enable specific tools individually. When `--enable-tool` is used it overrides `--toolset` entirely:

```bash
{{program}} mcp \
  --enable-tool debug_threads_command \
  --enable-tool debug_stack_trace_command \
  --enable-tool debug_variables_command
```

`--enable-tool` accepts the strum-serialized tool names (e.g. `debug_threads_command`, `debug_dap_request`) — the same identifiers the MCP server exposes to clients — **not** the abbreviated forms shown in the toolset table above. Run `{{program}} mcp --help` for the full accepted list.

## JSON output

`--json` (or `DAPPER_OUTPUT_JSON=true`, or `output_format = "json"` in Dapper's `config.toml`, which lives in `$DAPPER_CONFIG_DIR` when set and otherwise under the platform config dir, `~/.config/dapper` on Linux) makes tool results machine-readable instead of prose. It is a global flag, so it may appear before or after the subcommand:

```bash
{{program}} mcp --json
{{program}} --json mcp
```

`DAPPER_OUTPUT_JSON` is parsed as a strict boolean, so it accepts only `true` or `false`. Any other value, including `1`, aborts the command.

The result is still a single text content block; `--json` only changes the string inside it, so a client parses that string rather than reading MCP fields. Results carry the same envelope the CLI emits, with the response under `result` and any session context under `context`:

```json
{"context":{"session":{"sessionId":"abc123"}},"result":{"threads":[{"id":1,"name":"main"}]}}
```

Failures are objects too, so one parse handles both outcomes. They also still set `isError` on the tool result, so the `error` key is a convenience rather than the only signal:

```json
{"error":"specify at least one filter or set clear_existing: true to disable all exception breakpoints"}
```

Five tools sit outside that envelope. `debug_dap_request`, `debug_thread_snapshot` and `debug_config_command` return JSON in both modes, though the first two stop being parseable past 100 KB, where the response is truncated and spilled to a temp file. `debug_read_memory_command` and `debug_write_memory_command` render their successful payload as text in both modes; their failures are still the `error` object above.

## Per-call session targeting

Unlike the CLI, an MCP server is a long-lived connection. New sessions can come and go during a single MCP session, so MCP tool calls additionally accept a `session_id` argument that overrides the server's startup-time `--control-port`/`--scope-id`. Use it when a single agent is driving multiple debuggees over the lifetime of one MCP connection.

When `session_id` is omitted, the MCP server falls back to the *last* session it interacted with on this connection (if still active), and only then to the oldest active session — so a one-debuggee agent never has to think about it.

## Toolset escalation, not over-grant

For most agentic debugging, the default `standard` toolset is right. Escalate to `full` only when you need `evaluate` or `set-variable`. Use `raw` only when an adapter exposes a DAP command not surfaced by the typed tools — `raw` puts the entire DAP API at your disposal but loses the schema-checking safety net.

## Setting `DAPPER_SCOPE_ID` from the agent's session

If your agent has a stable session identifier of its own (e.g. Claude Code's `CLAUDE_CODE_CURRENT_SESSION_ID`), pass it through as `DAPPER_SCOPE_ID` so the MCP server and the proxy auto-pair without explicit `--scope-id` on every invocation.
