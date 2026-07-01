# Dapper

Dapper is a tool that provides multiple clients with simultaneous access to
debugging sessions that is uniform across different programming languages and
environments.

The goal of the project is to enable agentic AI debugging through a universal
MCP server for debugging. It enables access to debugging sessions in various
settings:

- **Autonomous.** Agents can drive debugging sessions without human
  involvement. Imagine inspecting and summarizing findings from analyzing core
  dumps.
- **Collaborative.** An AI agent jumps into your existing debugging session.
  Perhaps it has new insights into why that thread hangs.
- **Introspective.** Follow along as an AI agent drives a debugging session in
  the IDE, all through the familiar debugging interface of VS Code.

## How It Works

Dapper is a debug proxy server that sits between DAP clients (like VS Code)
and language-specific debug adapters (like LLDB, debugpy). It works with any
[DAP](https://microsoft.github.io/debug-adapter-protocol/specification) server
(lldb-dap, debugpy, dlv, etc.) without requiring modifications to existing
components.

**Why a proxy?** DAP does not support multi-client interactions with a
debugging session out of the box. The proxy enables us to offer AI agents
insights and control of existing debugging sessions.

## License

MIT

---

*dapper /'dæpər/ n. 1. In computing, a system or entity that utilizes the Debug Adapter Protocol (DAP). 2. Dutch. brave.*
