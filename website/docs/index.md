---
title: Overview
sidebar_label: Overview
---

# Dapper

Dapper is a Debug Adapter Protocol (DAP) proxy for shared, AI-assisted debugging.

It lets an IDE, command-line tools, and MCP-aware agents inspect and control the same live debug session. You keep the debugger integration you already use, while Dapper adds a control plane that other clients can safely attach to.

## What Dapper Is For

- Share one debug session between a human in an IDE and an agent in a terminal.
- Let agents inspect threads, stack frames, scopes, variables, breakpoints, and debugger capabilities.
- Connect MCP clients to an existing debug session without giving them direct ownership of the debug adapter.
- Run headless debugging workflows when an IDE is not involved.

## How It Fits Together

An IDE or launcher starts a debug adapter through Dapper. Dapper forwards DAP traffic between the original client and the adapter, then exposes a control API for extra clients. The CLI and MCP server use that control API to inspect or drive the same session.

```text
IDE / launcher <-> Dapper proxy <-> debug adapter <-> debuggee
                      ^
                      |
                 CLI / MCP agent
```

## Where To Start

Use [Installation](./installation.md) to build or locate the `dapper` binary, then [Getting Started](./getting-started.md) for the fastest path to a first session. Use [Agentic Debugging](./ai-agent.md) when connecting an agent through MCP or the CLI. Detailed command behavior lives under Reference.
