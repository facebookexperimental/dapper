# {{program}} debug — CLI Debug Client

Send debug commands to an active Dapper proxy session. This is the primary CLI interface for inspecting and controlling a running debugger.

For session targeting (`--scope-id`, `--control-port`, ambiguity rules) see `{{program}} help sessions`. The same auto-discovery rules apply to every subcommand.

```bash
# When exactly one session is active, auto-discovered:
{{program}} debug threads
```

## Inspection

### threads

```bash
{{program}} debug threads
```

```
Threads:
  Thread 1: main
  Thread 2: tokio-runtime-worker
```

### stack-trace

```bash
{{program}} debug stack-trace 1
```

```
Stack trace (frames 0 - 2) for thread 1:
  #0: process_request (frame id: 1001) at /code/server.rs:42
  #1: handle_connection (frame id: 1002) at /code/server.rs:18
  #2: main (frame id: 1003) at /code/main.rs:8
```

`--start-frame N` and `--levels K` paginate. `--levels 0` returns all frames.

### scopes

```bash
{{program}} debug scopes 1001
```

```
Scopes for frame 1001:
  Scope: Locals (ref: 2001, expensive: false)
  Scope: Globals (ref: 2002, expensive: true)
```

### variables

```bash
{{program}} debug variables 2001
```

```
Variables for reference 2001:
  request: HttpRequest{...} (HttpRequest) [ref: 3001]
  user_id: 42 (i64)
  buffer: [1, 2, 3] (Vec<u8>) [ref: 3002]
```

`[ref: N]` marks a structured value — feed `N` back into `variables` to drill in. References invalidate on every stop.

### eval

```bash
{{program}} debug eval "buffer.len()"
{{program}} debug eval "buffer.len()" --frame-id=1001
```

The expression is evaluated by the active adapter (LLDB / debugpy / ...) in its native syntax. `--frame-id` selects which frame's locals are in scope; without it, the topmost frame on the current thread is used.

## Navigation

```bash
{{program}} debug continue 1
{{program}} debug pause 1
{{program}} debug step in 1
{{program}} debug step over 1
{{program}} debug step out 1
```

`continue` waits up to 60 s and `pause` up to 5 s for a `stopped`/`exited` event and then prints the reason; `step` commands return as soon as the request is acknowledged. The waits are tunable via the `[navigate]` block in the user config.

## Breakpoints

For the JSON spec (line, conditional, log-message) plus function/exception breakpoints see `{{program}} help breakpoints`.

```bash
{{program}} debug set-breakpoints /abs/path/file.py -b 10 -b 20
```

```
Set 2 breakpoints in /abs/path/file.py:
  Verified: Line 10
  Verified: Line 20
```

## Raw DAP

Send any DAP request directly with the `dap` subcommand:

```bash
{{program}} debug dap threads
{{program}} debug dap pause --arguments '{"threadId": 1}'
{{program}} debug dap continue --arguments '{"threadId": 1}' --wait-for-event
```

`--wait-for-event` blocks until the next `stopped`/`exited` event (default 60 s, override with `--timeout`). Output is pretty-printed JSON; pipe into `jq` for further processing.

## Sessions

```bash
{{program}} debug sessions
```

Lists every active proxy. Output and pinning rules: `{{program}} help sessions`.

```bash
{{program}} debug stop
```

Stops the proxy and disconnects the adapter.

## Tips

- Examine the current state (`threads` → `stack-trace` → `variables`) before stepping. Stepping blind on an unknown program is the slow path.
- Variable references invalidate on every stop; re-fetch via `scopes` then `variables` after each navigation command.
- Pass absolute file paths to `set-breakpoints`; relative paths depend on the adapter's CWD.
- Set breakpoints sparingly — each one slows execution and clutters the stop reason.
