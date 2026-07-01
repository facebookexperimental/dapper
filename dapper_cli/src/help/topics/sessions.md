# Sessions: scope-id, control-port, session-id

A *session* is one running debug-adapter instance proxied by Dapper. Multiple sessions can coexist (e.g. dual-attach C++/Java debugging two debuggees concurrently). Routing a CLI command to the right session is what `--scope-id` and `--control-port` are for. (MCP additionally supports per-call `session_id` — see `{{program}} help mcp`.)

## The three identifiers

| Identifier | Set by | Stable across | Used for |
|---|---|---|---|
| **scope-id** | client / IDE / agent | the lifetime of the calling scope | grouping sessions together |
| **control-port** | proxy at startup | the proxy process | deterministic, single-session targeting |
| **session-id** | proxy at startup | the session | per-call MCP targeting; identification in the listing |

## Discovery

```bash
{{program}} debug sessions
{{program}} debug --scope-id=vscode-54196 sessions
```

Each entry reports the session-id, pid, control-port, scope-id, the start time, and the proxy's command line. Example for a single active session:

```
Found 1 active session(s):

Session a1b2c3d4-e5f6-7890-abcd-ef0123456789:
  PID:          54321
  Control Port: 47823
  Scope ID:     vscode-54196
  Session Type: lldb
  Started At:   2026-04-23 10:32:01
  Directory:    /home/user/code
  Command:      /usr/local/bin/dapper proxy --scope-id=vscode-54196 process /usr/local/bin/lldb-dap
```

When nothing is running:

```
No active sessions found.
```

## Auto-discovery rules

`--scope-id` and `--control-port` are both optional. With neither flag set:

- **Exactly one active session** → auto-discovered, used.
- **Multiple active sessions** → command exits with a candidate list. Pin one of them.

To pin:

- `--control-port=PORT` is **always deterministic** — it names exactly one session.
- `--scope-id=SCOPE` (or `DAPPER_SCOPE_ID` env var) **only narrows the candidate set**. If multiple sessions share the same scope (the dual-attach case), `--scope-id` alone is insufficient and the CLI errors out with a message pointing you at `--control-port`.

## Setting `DAPPER_SCOPE_ID` from the agent's session

If your agent has a stable session identifier of its own, export it as `DAPPER_SCOPE_ID` before invoking `{{program}}`. The proxy and all subsequent `{{program}} debug` calls auto-pair without you having to pass `--scope-id` on every command.

## Quick reference

```bash
# List everything that's running
{{program}} debug sessions

# Pin by control-port (always works)
{{program}} debug --control-port=12345 threads

# Pin by scope (only if scope is unique across active sessions)
DAPPER_SCOPE_ID=vscode-54196 {{program}} debug threads
```
