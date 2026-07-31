# Dapper end-to-end tests

The Rust sources under `e2e/` exercise Dapper as an external process.

## Running tests

Run the helper from the repository root:

```bash
python3 tests/run.py --test help_topics
```

The helper builds Dapper, obtains its executable path from Cargo's structured
artifact output, and invokes the dedicated test package with the required runtime
environment.

The default run executes every test whose adapter is available and reports
unavailable optional profiles as skipped. Selecting a test explicitly makes all
of its selected profiles required; use `--adapter` to select one profile for a
test that supports multiple adapters.

Debugpy tests use the active Python interpreter when it provides `debugpy`. A
dedicated environment can instead be created and used through `uv`:

```bash
uv run --project tests/profiles/debugpy --frozen \
  python tests/run.py --test launch --adapter debugpy
```

Use `--debugpy-python` to select another existing Python environment. The
runner validates it but does not install or update debugpy.

LLDB tests require `lldb-dap` and a C++ compiler. The runner discovers
`lldb-dap` and common compiler names on `PATH`, compiles the fixture with debug
information in a temporary directory, and removes it after the test:

```bash
python3 tests/run.py --test launch --adapter lldb
```

Use `--lldb-dap` and `--cxx` to select existing executables. The runner
validates these tools but does not install or update them.

For low-level debugging, callers may supply the runtime contract directly:

```bash
DAPPER_TEST_EXECUTABLE=/path/to/dapper \
  DAPPER_TEST_ADAPTER_EXECUTABLE=/path/to/python \
  DAPPER_TEST_ADAPTER_ARGUMENTS='["-m", "debugpy.adapter"]' \
  DAPPER_TEST_DEBUGGEE=/path/to/debuggee.py \
  cargo test --manifest-path tests/Cargo.toml \
    --features e2e-tests --test launch
```

The feature keeps these externally orchestrated tests out of ordinary
workspace test runs. It does not configure the test environment; use the
runner unless debugging the runtime contract directly.
