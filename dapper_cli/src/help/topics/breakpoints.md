# Breakpoints

Set, modify, or clear breakpoints in a source file with `{{program}} debug set-breakpoints`.

## Forms

A breakpoint can be a bare line number or a JSON object with optional fields. The JSON form is the underlying spec — the bare number is sugar for `{"line": N}`. The supported keys are `line`, `condition`, and `logMessage`; other DAP fields (`hitCondition`, `column`, ...) are not currently surfaced and will be silently ignored.

```bash
# Bare line numbers — the simplest form
{{program}} debug set-breakpoints /abs/path/file.py -b 10 -b 20
```

Example output:

```
Set 2 breakpoints in /abs/path/file.py:
  Verified: Line 10
  Verified: Line 20
```

`Not Verified` means the adapter accepted the breakpoint but couldn't bind it (usually a path mismatch or the line isn't executable). Re-check the path and the line.

```bash
# Conditional: stop only when the expression evaluates truthy
{{program}} debug set-breakpoints /abs/path/file.py \
  -b '{"line":10,"condition":"x > 5"}'

# Log point: log a message instead of stopping. `{name}` interpolates a variable.
{{program}} debug set-breakpoints /abs/path/file.py \
  -b '{"line":10,"logMessage":"value of x is {x}"}'
```

## `--clear-existing`

By default, `set-breakpoints` *appends* to whatever breakpoints already exist in the file. Pass `--clear-existing` to replace them instead:

```bash
{{program}} debug set-breakpoints /abs/path/file.py --clear-existing -b 15
```

This is the right flag when you want a known clean state in a file (e.g. after iterating during a debugging session).

## Exception breakpoints

Use `{{program}} debug set-exception-breakpoints` to enable adapter-advertised exception filters (e.g. "raised", "uncaught", "cpp_throw") that cause the debuggee to stop when an exception is thrown or unhandled.

```bash
# Discover supported filter ids first.
{{program}} debug capabilities | jq '.exceptionBreakpointFilters'

# Enable one or more filters. The flag is repeatable.
{{program}} debug set-exception-breakpoints --filter raised --filter uncaught

# Replace the active set instead of merging with what's already installed.
{{program}} debug set-exception-breakpoints --filter raised --clear-existing

# Disable all exception breakpoints (clear-all path).
{{program}} debug set-exception-breakpoints --clear-existing
```

By default, `set-exception-breakpoints` *merges* with the currently-installed set. Filters that are already installed keep their existing condition (set via `from-config` or by an IDE) — re-specifying a filter via this CLI doesn't drop its condition since the CLI has no way to express conditions. Pass `--clear-existing` to replace the active set verbatim.

Caveats:

- **IDE clobber.** If an IDE later sends its own `setExceptionBreakpoints`, the IDE's request replaces the entire active set on the adapter. CLI-installed filters can be silently overwritten.
- **Unmodeled DAP state.** The DAP protocol allows per-filter `mode` and a hierarchical `exceptionOptions` tree (used mostly by .NET and Java adapters). Neither is exposed by this CLI; if an IDE sets them, a subsequent `set-exception-breakpoints` call will erase them. Use `{{program}} debug dap setExceptionBreakpoints --arguments '{...}'` for full control over the raw request.

## Function breakpoints

Function-name breakpoints aren't yet a first-class command. Send the raw DAP request via `{{program}} debug dap`:

```bash
{{program}} debug dap setFunctionBreakpoints \
  --arguments '{"breakpoints":[{"name":"my_function"}]}'
```

Adapter capability matters here — not every adapter supports function breakpoints. If the request returns an error, the adapter doesn't support that breakpoint kind; fall back to line breakpoints.

## Tips

- **Use absolute paths.** Relative paths depend on the adapter's CWD and are a common source of "my breakpoint never hits" confusion.
- **Re-resolve after rebuilding.** Source-line numbers can shift across recompilations. Re-set breakpoints after a rebuild rather than wondering why the old ones miss.
- **Set sparingly.** Each active breakpoint slows execution and makes stops noisier. Add one, learn what it tells you, then move it.
