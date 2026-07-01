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

## Headless Sessions

For automation and scripted agent workflows, run a session headlessly from a config file instead of attaching an external client:

```bash
dapper proxy --scope-id=my-session from-config /path/to/session.json
```

The config describes how to spawn or connect to the debug adapter (`spawnConfig`) and the request that starts debugging (`debugRequest`, a `launch` or `attach`). With a `debugRequest` present, Dapper drives the initialization handshake itself, so no editor or external DAP client is required. List and inspect the session exactly as above with `dapper debug sessions`, `dapper debug threads`, and friends.

### Child Sessions

Some adapters (for example `debugpy` with `subProcess: true`) ask the client to start additional debug sessions for subprocesses, via the DAP `startDebugging` reverse request. On a headless session there is no editor to handle that, so Dapper can spawn each child as its own peer `dapper proxy from-config` process. Children appear in the same scope and are driven like any other session — target one explicitly with its `--control-port`.

Child spawning is **off by default** and is configured declaratively under `childSessions`:

```json
{
  "spawnConfig": { "type": "stdio", "cmd": "/path/to/debug-adapter" },
  "debugRequest": { "request": "launch", "program": "/path/to/program" },
  "childSessions": {
    "autoSpawn": true,
    "maxDepth": 1,
    "maxChildren": 16,
    "profile": {
      "rules": [
        {
          "when": { "request": "attach", "exists": ["configuration.connect.host", "configuration.connect.port"] },
          "childBackend": { "type": "tcp", "host": "${configuration.connect.host}", "port": "${configuration.connect.port}" },
          "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
        }
      ]
    }
  }
}
```

Each rule's `when` clause is matched against the reverse request (first match wins); `${...}` templates substitute values from the request into the child's backend and debug request. `maxDepth` bounds how many generations of descendants may spawn (`1` allows one generation; `0` disables spawning), and `maxChildren` caps concurrent direct children. A reverse request with no matching rule fails closed — Dapper never spawns a session it cannot resolve.

Instead of writing the rules out, `profile` may name a bundled preset that expands to the same rules:

```json
{
  "spawnConfig": { "type": "stdio", "cmd": "/path/to/debugpy-adapter" },
  "debugRequest": { "request": "launch", "program": "/path/to/program" },
  "childSessions": { "autoSpawn": true, "profile": "debugpy" }
}
```

The `"debugpy"` preset is the connect-back rule shown above — it attaches each child to the `configuration.connect` host/port the adapter hands back, so it works regardless of how the parent adapter is reached. The `"lldb-dap"` preset reuses the parent's own server endpoint for the target handoff, so it applies **only when the parent backend is `tcp` or `uds`**; with a stdio parent it has no applicable rule, so the capability is not advertised and any `startDebugging` fails closed with an explanatory message.

> **Trust model.** Enabling `autoSpawn` lets the debug adapter trigger local child `dapper` processes on this machine according to the configured profile. It is opt-in (default off) and should be enabled only for adapters and configs you trust — especially for `tcp` and `uds` parent backends, where the adapter endpoint may be a shared or remote server whose reverse requests would then cause Dapper to spawn local processes. Child-session spawning is currently supported on Unix only; on other platforms the capability is not advertised and reverse requests fail closed.
