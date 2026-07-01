---
title: Agentic Debugging
sidebar_label: Agentic Debugging
---

# Agentic Debugging

Dapper gives agents structured access to a live debugger without requiring them to own the original debug session.

## Recommended Setup

Use the MCP server for agent integrations that support MCP:

```bash
dapper mcp
```

The default `standard` toolset is enough for most debugging work. It includes thread and stack inspection, variables, navigation, and breakpoints. Session discovery is available separately in the MCP server regardless of the selected toolset. Escalate to `full` only when the agent needs evaluation, variable mutation, or memory access.

```bash
dapper mcp --toolset=full
```

## Good Debugging Loop

1. Identify the session with `dapper debug sessions` or MCP session tools.
2. Inspect threads and stack frames before stepping.
3. Fetch scopes and variables at the current frame.
4. Set one or two targeted breakpoints.
5. Step or continue, then re-fetch scopes and variables after each stop.
6. Apply the source fix and restart the session to verify behavior.

## Guardrails

- Prefer read-only inspection until there is a clear reason to mutate state.
- Pin the session with `--control-port` when multiple sessions are active.
- Re-fetch variable references after every stop; old references are not stable.
- Use raw DAP access only for adapter-specific commands that typed tools do not expose.

For command-level details, see [Agent Operating Guide](./reference/agent.md), [dapper debug](./reference/debug.md), and [MCP toolsets](./reference/mcp.md).
