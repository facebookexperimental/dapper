# Dapper — DAP Proxy for Multi-Client Debugging

Dapper is a Debug Adapter Protocol (DAP) proxy that lets multiple clients (VS Code, Claude Code, CLI) share a single debug session. It runs as a proxy between the IDE and the debug adapter, and ships an MCP server so AI agents can drive a live debugger.

For agent-driven debugging, start with `{{program}} help agent`. It covers the operating model, the end-to-end debugging loop, and the deeper-dive topics worth consulting on demand.

## Pick your task

| You want to... | Run | Then read |
|---|---|---|
| Drive Dapper as an autonomous agent | — | `{{program}} help agent` |
| Discover what sessions exist | `{{program}} debug sessions` | `{{program}} help sessions` |
| Inspect threads / stack / variables | `{{program}} debug threads`, `… stack-trace`, `… variables` | `{{program}} help debug` |
| Set or change breakpoints | `{{program}} debug set-breakpoints …` | `{{program}} help breakpoints` |
| Step / continue / pause | `{{program}} debug {step,continue,pause} <thread>` | `{{program}} help debug` |
| Connect an MCP-aware agent | `{{program}} mcp` | `{{program}} help mcp` |
| Start the proxy yourself (rare) | `{{program}} proxy …` | `{{program}} help proxy` |

## Quick smoke test

```bash
# Is anything running? If yes, the table tells you what you can talk to.
{{program}} debug sessions
```

If the answer is "No active sessions found.", an IDE or `fdb` invocation needs to start one first; this CLI only attaches to existing proxies, it does not launch debuggers on its own (use `{{program}} proxy from-config <config.json>` for headless workflows — see `{{program}} help proxy`).

## Help output format

`{{program}} help` renders Markdown with terminal styling when stdout is interactive and leaves raw Markdown intact when redirected. Set `DAPPER_HELP_FORMAT` to `auto`, `plain`, or `terminal` to force the behavior.
