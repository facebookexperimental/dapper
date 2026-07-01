# Driving Dapper from an autonomous agent

The single page an agent should read before invoking `{{program}}`. Covers the operational rules, the end-to-end debugging loop, common pitfalls, and the deeper-dive topics to consult when the situation calls for them.

## Rules of engagement

### Discovery first, action second

Run `{{program}} debug sessions` before any other debug command. Dapper's session targeting is the most common source of agent confusion — knowing what's there avoids guessing.

### Pin a session when there are several

When more than one session is active, pass `--control-port=PORT` (deterministic) or `--scope-id=SCOPE` (narrows the auto-discovery candidate set) before the subcommand. `--scope-id` alone is not sufficient when multiple sessions share the same scope (e.g. dual-attach C++/Java) — the CLI errors out and tells you to use `--control-port`. Don't retry blindly. The full identifier model lives in `{{program}} help sessions`.

### Don't infer flags from training data

`{{program}}`'s surface evolves. Run `{{program}} help <topic>` against the installed binary instead of assuming a flag exists. The help output reflects the version that's running, not what some older revision documented.

### Wait for events in headless mode

When a debug session is started in headless mode (`{{program}} proxy from-config`), the proxy emits `SESSION_READY` and `PROGRAM_STOPPED` events as JSON lines on stdout (or on the numeric file descriptor passed via `--events-fd FD` — see `{{program}} help proxy`). Wait for them rather than polling — polling races against the adapter's startup and produces flaky behavior.

### Variable references invalidate on every stop

The `variablesReference` returned by `scopes` and `variables` is bound to the current stop. After any `continue`/`step`/`pause`, re-fetch via `scopes` then `variables` — re-using a stale reference will return wrong data or fail.

### Don't `{{program}} proxy` directly unless you mean to

The proxy is normally started by an IDE or by `fdb`. Running `{{program}} proxy` by hand is a manual-setup tool, not the default workflow.

### Read once per session

Topic content is stable for a given binary version. There's no value in re-running `{{program}} help <topic>` mid-session — the bytes don't change.

## When to reach for a debugger

Reach for the debugger when:

- You need to inspect runtime state (variable values, call stack, thread interactions) that logging didn't capture or would be expensive to add.
- You're investigating a behavior — wrong output, deadlock, race — rather than a known crash whose stack trace already names the culprit.
- A test reproduces consistently and you want to step through the failing path.

Skip the debugger when a trace, log, or single targeted print would answer the question faster.

## The loop

1. **Examine.** Discover what's running with `{{program}} debug sessions`. Look at `{{program}} debug threads` once stopped.
2. **Stop where it matters.** Set a breakpoint at the suspected site. Prefer one or two well-placed breakpoints to many speculative ones.
3. **Inspect at the breakpoint.** Walk the stack: `stack-trace <thread>` → `scopes <frame>` → `variables <ref>`. Re-fetch references after every stop.
4. **Form a hypothesis.** "If X were Y, the variable would be Z." Use `eval` to test it without rerunning.
5. **Iterate.** `step over`, `step in`, `step out`, `continue`. After each, re-examine state. Variable references invalidate on every stop.
6. **Fix and verify.** Change source, restart the session, confirm the breakpoint no longer fires the bad path.

## Multi-client driving

The whole point of Dapper is that the IDE, the CLI, and an MCP-aware agent can share one debug session. Typical pattern:

1. IDE launches the program, hits a breakpoint.
2. The agent connects via `{{program}} debug …` from the shell, or via the MCP server.
3. Agent and human take turns inspecting and stepping. The proxy keeps both sides consistent.

## Common pitfalls

- **Stepping blind.** Examining the call stack before stepping costs nothing and often reveals the answer immediately.
- **Too many breakpoints.** Each one slows execution and makes stops noisy. Add one, learn, then move it.
- **Stale variable references.** A `variablesReference` from before the last stop will silently return wrong data; refresh after every navigation.
- **Wrong session.** When multiple sessions are active and you didn't pin one, the next command may target the wrong debuggee.

## Follow-up topics

Read these when the situation calls for them — not preemptively:

- `{{program}} help sessions` — deep dive on the `scope-id` / `control-port` / `session-id` model and the auto-discovery rules. Read when `{{program}} debug sessions` returns more than one entry or when a dual-attach error message points you here.
- `{{program}} help debug` — full CLI reference for `{{program}} debug` subcommands with example outputs. Read once before constructing your first command, then return only when you need a flag you haven't used.
- `{{program}} help breakpoints` — JSON spec for line / conditional / log-message breakpoints; function and exception breakpoint workarounds. Read before constructing anything beyond a bare line number.

For the full list of topics run `{{program}} help`.
