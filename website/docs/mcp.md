---
title: MCP Server
sidebar_label: MCP Server
---

# MCP Server

Dapper includes an MCP server that exposes debugger operations as tools for AI agents.

The server connects to a Dapper proxy session and translates tool calls into debug operations such as listing threads, reading stack frames, inspecting variables, setting breakpoints, and navigating execution.

## Start The Server

```bash
dapper mcp
```

When exactly one session is active, Dapper can target it automatically. For multiple sessions, pass the control port:

```bash
dapper mcp --control-port=47823
```

## Choose A Toolset

Use the default `standard` toolset for normal agentic debugging:

```bash
dapper mcp --toolset=standard
```

Use `full` when the agent needs evaluation, variable mutation, or memory access:

```bash
dapper mcp --toolset=full
```

Use `raw` only when the agent needs to send adapter-specific DAP requests that are not represented by typed tools.

## Session Targeting

For long-lived agent sessions, pass a stable scope through `DAPPER_SCOPE_ID` when your agent environment provides one:

```bash
DAPPER_SCOPE_ID=my-agent-session dapper mcp
```

The scope helps the proxy and agent pair automatically, but `--control-port` is still the most deterministic option when several sessions are active.

For the exact toolset contents and custom tool selection flags, see [dapper mcp](./reference/mcp.md).
