---
title: Editor Integration
sidebar_label: Editor Integration
---

# Editor Integration

Dapper does not ship a one-click IDE extension. Instead, you keep the debugger
extension you already use and insert Dapper *between* the editor and the debug
adapter it launches. Once the adapter runs through `dapper proxy`, the CLI and
MCP agents can attach to the very same live session.

```text
editor DAP client  ->  dapper proxy  ->  debug adapter  ->  debuggee
                            ^
                            |
                       CLI / MCP agent
```

## The wrapper pattern

Most editors launch their debug adapter as a child process and speak DAP to it
over stdin/stdout. The trick is to replace that adapter command with a thin
wrapper that execs `dapper proxy process <real-adapter>`:

- Dapper speaks DAP over **stdio** to the editor (no `--client-port`).
- Dapper spawns the real adapter and speaks DAP to it.
- Dapper's own logs go to **stderr** and a log file (`DAPPER_LOG_PATH`, or a
  temp path), so **stdout stays clean** for the DAP stream.

How you point the editor at the wrapper depends on the extension: some expose a
setting for the adapter path, others let you override it per launch in
`launch.json`. Below are step-by-step setups for the two official VS Code
extensions.

## What You Need

- A working `dapper` binary on your `PATH` (see [Installation](./installation.md)).
- The path to the real debug adapter the extension normally launches
  (`lldb-dap`, or `debugpy`).
- Permission to edit your VS Code `settings.json` and/or `launch.json`.

:::note `dapper` not found?
VS Code launched from the GUI may not inherit your shell's `PATH`. If the editor
reports that the adapter failed to start, use the **absolute** path to `dapper`
in the scripts below (find it with `which dapper`).
:::

## VS Code: LLDB DAP extension

For the official [`llvm-vs-code-extensions.lldb-dap`](https://marketplace.visualstudio.com/items?itemName=llvm-vs-code-extensions.lldb-dap)
extension, the adapter is launched from the path in the `lldb-dap.executable-path`
setting (or auto-detected). Point that setting at a wrapper script.

### 1. Create the wrapper script

Save this as, for example, `~/.dapper/lldb-dap-proxy.sh`:

```bash
#!/usr/bin/env bash
# Run the real lldb-dap behind a Dapper proxy so the CLI and MCP agents
# can share this VS Code debug session.
set -euo pipefail

# The real lldb-dap binary. On macOS, xcrun resolves Xcode's copy;
# otherwise fall back to the first lldb-dap on PATH. Replace with an
# absolute path if neither applies.
REAL_LLDB_DAP="$(xcrun -f lldb-dap 2>/dev/null || command -v lldb-dap || true)"
if [[ -z "$REAL_LLDB_DAP" ]]; then
  echo "lldb-dap not found; set REAL_LLDB_DAP to an absolute path." >&2
  exit 1
fi

# Use an absolute path here if `dapper` is not on VS Code's PATH.
DAPPER_BIN="${DAPPER_BIN:-dapper}"

# Keep Dapper's logs off stdout (which carries the DAP stream).
export DAPPER_LOG_PATH="${TMPDIR:-/tmp}/dapper-lldb.log"

# `"$@"` forwards any extra args from `lldb-dap.arguments`.
# `exec` hands stdio and signals straight to the proxy.
exec "$DAPPER_BIN" proxy \
  --scope-id="vscode-lldb" \
  process "$REAL_LLDB_DAP" "$@"
```

Make it executable:

```bash
chmod +x ~/.dapper/lldb-dap-proxy.sh
```

### 2. Point the extension at the wrapper

In VS Code `settings.json` (User or Workspace), set the adapter path to your
script and keep server mode off:

```json
{
  "lldb-dap.executable-path": "/Users/you/.dapper/lldb-dap-proxy.sh",
  "lldb-dap.serverMode": false
}
```

You can also set this from the Settings UI — search for **"lldb-dap: Executable
Path"**.

:::caution Keep `serverMode` off
With `lldb-dap.serverMode` enabled (or a `debugAdapterPort` in your launch
config), the extension connects to lldb-dap over a TCP port and **bypasses the
wrapped executable**, so the proxy would not be in the path. Leave both unset.
:::

### 3. Debug as usual

Your existing `lldb-dap` launch configurations work unchanged — the `program`,
`args`, and other fields still flow through to lldb-dap over DAP:

```json
{
  "type": "lldb-dap",
  "request": "launch",
  "name": "Debug (via Dapper)",
  "program": "${workspaceFolder}/build/my_program"
}
```

**Per-project alternative.** Instead of the global setting, you can override the
adapter for a single launch with `debugAdapterExecutable` (do not also set
`debugAdapterPort`):

```json
{
  "type": "lldb-dap",
  "request": "launch",
  "name": "Debug (via Dapper)",
  "program": "${workspaceFolder}/build/my_program",
  "debugAdapterExecutable": "/Users/you/.dapper/lldb-dap-proxy.sh"
}
```

## VS Code: Python Debugger (debugpy) extension

The official [`ms-python.debugpy`](https://marketplace.visualstudio.com/items?itemName=ms-python.debugpy)
extension has no adapter-path **setting**. It always launches its adapter as
`python <adapter-path> [--log-dir ...]`, and any interpreter override is
validated as a real Python install. That means a bash wrapper can't stand in for
the adapter — but a small **Python** shim can. Point the launch config's
`debugAdapterPath` at that shim, and it re-execs the proxy.

### 1. Install debugpy in your interpreter

The shim launches the adapter as `python -m debugpy.adapter`, so `debugpy` must
be importable by the interpreter you debug with:

```bash
python -m pip install debugpy
```

### 2. Create the Python adapter shim

Save this as, for example, `~/.dapper/debugpy-adapter-proxy.py`:

```python
#!/usr/bin/env python3
"""Run the debugpy DAP adapter behind a Dapper proxy.

The Python Debugger extension launches its adapter as:

    <python> <this script> [--log-dir DIR]

so `sys.executable` is exactly the interpreter the extension selected.
We re-exec that as `dapper proxy process <python> -m debugpy.adapter`,
forwarding any extra args (e.g. --log-dir) through to the adapter.
"""
import os
import sys

# Fail early with a clear message if debugpy isn't available.
try:
    import debugpy  # noqa: F401
except ImportError:
    sys.exit(
        f"debugpy is not installed for {sys.executable}. "
        f"Run: {sys.executable} -m pip install debugpy"
    )

# Use an absolute path here if `dapper` is not on VS Code's PATH.
dapper_bin = os.environ.get("DAPPER_BIN", "dapper")

# Keep Dapper's logs off stdout (which carries the DAP stream).
os.environ.setdefault(
    "DAPPER_LOG_PATH",
    os.path.join(os.environ.get("TMPDIR", "/tmp"), "dapper-python.log"),
)

argv = [
    dapper_bin, "proxy",
    "--scope-id", "vscode-python",
    "process",
    sys.executable, "-m", "debugpy.adapter",
    *sys.argv[1:],
]

try:
    os.execvp(dapper_bin, argv)
except FileNotFoundError:
    sys.exit(
        f"Could not find '{dapper_bin}' on PATH. "
        f"Set DAPPER_BIN to its absolute path."
    )
```

### 3. Point launch.json at the shim

Add `debugAdapterPath` (a fully-qualified path) to your Python launch config:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "debugpy",
      "request": "launch",
      "name": "Python: Current File (via Dapper)",
      "program": "${file}",
      "console": "integratedTerminal",
      "debugAdapterPath": "/Users/you/.dapper/debugpy-adapter-proxy.py"
    }
  ]
}
```

The debuggee still launches with your normal interpreter selection — only the
adapter is routed through Dapper. Press **F5** and the proxy starts
automatically; there is no separate server to launch.

:::note Why a Python script and not a bash one?
Unlike lldb-dap, the Python Debugger always runs its adapter *through* Python
(`python <debugAdapterPath>`) and checks that the adapter interpreter is a valid
Python install. A bash script would be handed to Python and rejected, so the
override has to be a Python file.
:::

## Verify the session is shared

Once the editor has started a debug session through Dapper, confirm Dapper can
see it from another terminal:

```bash
dapper debug sessions
```

When exactly one session is active, the CLI and MCP server discover it
automatically. With several active sessions, pin one deterministically with its
control port:

```bash
dapper debug --control-port=47823 threads
dapper mcp --control-port=47823
```

The `--scope-id` baked into each wrapper (`vscode-lldb`, `vscode-python`) groups
sessions and lets agents auto-pair, but `--control-port` is the most precise
when multiple sessions are live. See [Sessions](./reference/sessions.md) for the
full targeting model and [dapper proxy](./reference/proxy.md) for other proxy
modes (TCP, Unix-domain socket, headless `from-config`).
