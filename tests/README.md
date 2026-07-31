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

For low-level debugging, callers may supply the runtime contract directly:

```bash
DAPPER_TEST_EXECUTABLE=/path/to/dapper \
  cargo test --manifest-path tests/Cargo.toml \
    --features e2e-tests --test help_topics
```

The feature keeps these externally orchestrated tests out of ordinary
workspace test runs. It does not configure the test environment; use the
runner unless debugging the runtime contract directly.
