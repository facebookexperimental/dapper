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

## Per-call session targeting

Unlike the CLI, an MCP server is a long-lived connection. New sessions can come and go during a single MCP session, so MCP tool calls additionally accept a `session_id` argument that overrides the server's startup-time `--control-port`/`--scope-id`. Use it when a single agent is driving multiple debuggees over the lifetime of one MCP connection.

When `session_id` is omitted, the MCP server falls back to the *last* session it interacted with on this connection (if still active), and only then to the oldest active session — so a one-debuggee agent never has to think about it.

## Toolset escalation, not over-grant

For most agentic debugging, the default `standard` toolset is right. Escalate to `full` only when you need `evaluate` or `set-variable`. Use `raw` only when an adapter exposes a DAP command not surfaced by the typed tools — `raw` puts the entire DAP API at your disposal but loses the schema-checking safety net.

## Setting `DAPPER_SCOPE_ID` from the agent's session

If your agent has a stable session identifier of its own (e.g. Claude Code's `CLAUDE_CODE_CURRENT_SESSION_ID`), pass it through as `DAPPER_SCOPE_ID` so the MCP server and the proxy auto-pair without explicit `--scope-id` on every invocation.
