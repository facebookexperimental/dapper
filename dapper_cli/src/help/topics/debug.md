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

The expression is evaluated by the active adapter (LLDB / debugpy / ...) in its native syntax. `--frame-id` selects which frame's locals are in scope; without it, no frame is sent and the adapter picks the context (the DAP default is the global scope), so pass it whenever the expression reads locals.

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

`--wait-for-event` blocks until the next `stopped`/`exited` event. `--timeout` bounds the request and, with `--wait-for-event`, the event wait (default 60 s; 0 also means 60). Output is pretty-printed JSON; pipe it into `jq`, if available, for further processing.

## Machine-readable output

Add `--json` after `debug` (or set `DAPPER_OUTPUT_JSON=true`) to get one JSON object per command: `result` holds the command's own data and `context` the session state. Text output is laid out for reading; JSON keeps every field, so use it when a script needs particular values.

If `jq` is available, these pick out common fields. Without it, read the JSON as is.

```bash
# Frame ids, names and locations of thread 1
{{program}} debug --json stack-trace 1 | jq -r '.result.stackFrames[] | "\(.id)\t\(.name)\t\(.source.path):\(.line)"'

# How a continue ended: stopped, exited, terminated or timedOut
{{program}} debug --json continue 1 | jq -r '.result.result.type'

# One local of the innermost frame, in full
{{program}} debug --json stack-trace 1 | jq -r '.result.scopes.locals[] | select(.name == "order") | .value'

# An evaluation result, in a frame id from this stop's stack-trace
{{program}} debug --json eval "len(orders)" --frame-id 1001 | jq -r '.result'

# Execution state: status, stopped thread, stop reason
{{program}} debug --json status | jq '.context.executionState'
```

A `continue` that stopped returns:

```json
{
  "result": {
    "navigationType": "continue",
    "result": {"type": "stopped", "data": {"reason": "breakpoint", "threadId": 1, "allThreadsStopped": true}}
  },
  "context": {
    "session": {"sessionId": "7e84ab68-1e64-4203-87c3-9c5952107ced", "type": "launch", "program": "/path/to/invoice.par"},
    "executionState": {"version": 3, "status": "stopped", "threadId": 1, "stopReason": "breakpoint"}
  }
}
```

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
