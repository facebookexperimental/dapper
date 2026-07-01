# {{program}} proxy — DAP Proxy Server

Start a Dapper proxy that sits between a DAP client (VS Code, IDE) and a debug adapter backend. The proxy enables multiple clients to share a single debug session and exposes a gRPC control plane for programmatic access.

You typically don't run this directly — an IDE integration starts it automatically when launching a debug session. Understanding this command is useful for manual setups or troubleshooting.

For session targeting (`--scope-id`, `--control-port`) see `{{program}} help sessions`.

## Backend modes

The proxy connects to a debug adapter in one of four ways:

### Process (stdio)

Spawn a debug adapter process and communicate via stdin/stdout:

```bash
{{program}} proxy --scope-id=vscode-54196 process /path/to/lldb-dap --interpreter=vscode
```

Arguments after the command name are passed through to the debug adapter.

### TCP

Connect to a debug adapter already listening on a TCP socket:

```bash
{{program}} proxy --scope-id=vscode-54196 tcp 127.0.0.1:4711
```

### Unix Domain Socket

Connect to a debug adapter listening on a UDS (Unix only):

```bash
{{program}} proxy --scope-id=vscode-54196 uds /tmp/debug.sock
```

### From config

Read a JSON configuration file that specifies the backend, launch request, and breakpoints:

```bash
{{program}} proxy --scope-id=vscode-54196 from-config /path/to/config.json
```

When a `debugRequest` is present in the config, the proxy runs in **headless mode** (no external DAP client needed) and emits structured progress events on stdout (or the fd passed to `--events-fd`). This is the mode used by automated/agent-driven debugging workflows.

Config keys are **camelCase** (the schema is `dapper_session::DebugSessionConfig`). Watch out: snake_case keys like `debug_request` are silently ignored — the proxy then sees no debug request and hangs waiting for an external DAP client instead of running headless.

`spawnConfig.type` is the *transport* (`stdio` / `tcp` / `uds`), not the language; the adapter in `cmd` must already be installed (e.g. `pip install debugpy`). Use `"console": "internalConsole"` for headless runs — `integratedTerminal` relies on a `runInTerminal` client capability the headless driver does not advertise. Fields inside `debugRequest` (other than `request`) pass through verbatim as DAP arguments, mirroring an IDE `launch.json`.

## Options

| Option | Description |
|--------|-------------|
| `--control-port PORT` | Port for the gRPC control plane (default `0` — OS picks an ephemeral port) |
| `--client-port PORT` | TCP port for external DAP client connections (default: stdio) |
| `--scope-id ID` | Scope identifier for this session (e.g., `vscode-54196`) |

When `from-config` is used, an extra `--events-fd FD` flag (Unix only) redirects the progress event stream to that file descriptor instead of stdout — useful when stdout is reserved for the DAP client.
