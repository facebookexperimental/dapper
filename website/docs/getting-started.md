---
title: Getting Started
sidebar_label: Getting Started
---

# Getting Started

This guide assumes you already have a working `dapper` binary. If not, start with the installation guide for your environment.

## 1. Start A Debug Session

The easiest path is to launch debugging from an integration or wrapper that already knows how to start Dapper. If you do not have one, configure your debugger launch command so Dapper starts the debug adapter through `dapper proxy`.

If you are wiring a setup manually, start the proxy with the adapter you want to use:

```bash
dapper proxy --scope-id=my-session process /path/to/debug-adapter
```

Most day-to-day users should eventually launch this through an editor or project-specific wrapper. Running `dapper proxy` directly is useful while developing an integration, troubleshooting, or setting up headless workflows.

## 2. Confirm Dapper Can See The Session

In another terminal, list active sessions:

```bash
dapper debug sessions
```

If exactly one session is active, Dapper can target it automatically. If several sessions are active, copy the `Control Port` from the session you want and pass it with `--control-port`.

## 3. Inspect Runtime State

Once the debuggee is stopped, inspect it from the CLI:

```bash
dapper debug threads
dapper debug stack-trace 1
dapper debug scopes 1001
dapper debug variables 2001
```

The exact frame and variable reference IDs come from the previous command output. Variable references are valid only for the current stopped state; re-fetch scopes and variables after stepping or continuing.

## 4. Add An Agent

For MCP-capable clients, run:

```bash
dapper mcp
```

The MCP server connects to the active session and exposes debugger tools to the client. See [MCP Server](./mcp.md) for toolsets and setup guidance.
