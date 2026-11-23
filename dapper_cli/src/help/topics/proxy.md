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

Pass-through is unconditional, so it swallows Dapper's own global flags too. Put them before the subcommand, or use the environment variable instead (`DAPPER_REASON`, `DAPPER_OUTPUT_JSON`), which is placement-proof:

```bash
{{program}} proxy process /path/to/lldb-dap --reason "..."   # adapter gets --reason
{{program}} --reason "..." proxy process /path/to/lldb-dap   # dapper gets --reason
```

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

## Access policies

By default, any caller of the control plane, `{{program}} debug` or an MCP server, may do anything with the session. `--allow` limits callers to the presets and operations it names, comma-separated or repeated. A session config's `access` takes the same list, as `"access": {"allow": ["standard", "read_memory"]}`; set one or the other, since setting both refuses to start.

```bash
{{program}} proxy --allow minimal,navigate process /path/to/lldb-dap
```

| Operation | Lets a caller |
|-----------|---------------|
| `inspect` | list threads, stack frames, scopes, modules and sources |
| `read_values` | read variables, and see locals, program output and why the program stopped |
| `navigate` | continue, step, step back, pause and jump |
| `breakpoints` | set breakpoints of any kind |
| `evaluate` | evaluate expressions and debugger commands |
| `set_variable` | change the value of a variable or expression |
| `read_memory` | read and disassemble memory |
| `write_memory` | write memory |
| `stop` | stop the session or its threads |
| `unclassified` | send any other request, such as `launch`, `restart` or an adapter's custom request |

The presets build on each other: `structure` is `inspect`, `minimal` adds `read_values`, `standard` adds `navigate`, `breakpoints` and `stop`, and `full` is every operation. Naming an operation also allows what it needs: `evaluate`, `set_variable` and `read_memory` need `read_values`, and `unclassified` needs everything. An operation allows its own requests and no more, even where those reach further: an expression or debugger command can write memory or end the session (lldb's `memory write`, any code in debugpy), and the adapter evaluates a variable's new value and a breakpoint's condition or log message the same way. A policy limits which requests a caller makes, not what the code in them does. A list that names something Dapper does not know refuses to start.

A refused call fails with what it needs and what the session allows, e.g. `` `eval_repl` needs evaluate, which this session's access policy does not allow; it allows inspect ``. Without `read_values`, the session also keeps the program's values out of its answers: it expands no locals, and drops program output and stop descriptions from the response context. `{{program}} debug status` reports the policy in force, and child sessions spawned for `startDebugging` get the parent's policy and override token.

A headless session reports its policy in its `DAPPER_READY` event and, when it has one, an override token that lets its launcher past it (`{{program}} help debug`). A proxy started with `DAPPER_OVERRIDE_TOKEN` set takes that token instead of making one. A launcher that asked for a policy should refuse to go on unless `DAPPER_READY` reports that policy. A proxy that predates access policies reports none and enforces nothing, and a misspelled `access` key is ignored like any other unknown config key, so that session runs unrestricted and reports every operation.

## Options

| Option | Description |
|--------|-------------|
| `--control-port PORT` | Port for the gRPC control plane (default `0` — OS picks an ephemeral port) |
| `--client-port PORT` | TCP port for external DAP client connections (default: stdio) |
| `--scope-id ID` | Scope identifier for this session (e.g., `vscode-54196`); defaults to `DAPPER_SCOPE_ID` |
| `--allow GRANTS` | Presets and operations control-plane callers may use (default: everything); see above |

When `from-config` is used, an extra `--events-fd FD` flag (Unix only) redirects the progress event stream to that file descriptor as bare JSON lines, instead of `[DAPPER_SESSION] `-prefixed lines on stdout, giving the driver a dedicated event channel.
