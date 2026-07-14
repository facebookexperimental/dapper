// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::RwLock;
use std::sync::RwLockReadGuard;
use std::sync::RwLockWriteGuard;
use std::time::Instant;

use base64::Engine as _;
use dapper_config::DapperConfig;
use dapper_control_api::ControlPlaneResult;
use dapper_control_api::DapperControlPlane;
use dapper_control_api::DapperControlPlaneClient;
use dapper_control_api::RenderedResponse;
use dapper_control_api::render_plaintext;
use dapper_control_api::resolve_unique_session;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::responses::ResponseBody;
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::SessionInfo;
use dapper_session::SessionStore;
use rmcp::ErrorData;
use rmcp::ServerHandler;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::ContentBlock as Content;
use rmcp::model::ListToolsResult;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ServerCapabilities;
use rmcp::model::ServerInfo;
use rmcp::model::Tool;
use rmcp::serde_json;
use rmcp::service::NotificationContext;
use rmcp::service::RequestContext;
use rmcp::service::RoleServer;
use rmcp::tool;
use rmcp::tool_router;

use crate::toolsets::DebugTool;
use crate::toolsets::Toolset;

mod format;
mod params;

use format::format_capabilities;
use format::format_memory_read;
use params::EmptyParams;
use params::EvaluateRequest;
use params::FrameIdRequest;
use params::MAX_READ_BYTES;
use params::NavigateRequest;
use params::RawDapRequestParams;
use params::ReadMemoryRequest;
use params::SessionTargeted;
use params::SetBreakpointsRequest;
use params::SetExceptionBreakpointsRequest;
use params::SetVariableRequest;
use params::StackTraceRequest;
use params::ThreadSnapshotRequest;
use params::VariablesReferenceRequest;
use params::WriteMemoryRequest;
use params::clamp_snapshot_limits;
use params::default_timeout;
use params::hex_string_to_bytes;

struct CachedClient {
    client: Arc<DapperControlPlaneClient>,
    /// Full session info for the cached client, so the fast path in
    /// `get_client` can cheaply re-validate liveness (`is_active`) without a
    /// full sessions-directory scan.
    session: SessionInfo,
}

/// Everything an `McpHandler` needs from its environment.
pub struct McpServerEnv {
    /// Fixed control plane port; when set, session discovery is bypassed.
    pub control_port: Option<Port>,
    /// Scope filter for session discovery.
    pub scope_id: Option<ScopeId>,
    /// Where to discover active sessions.
    pub sessions: SessionStore,
    /// Rendering and context configuration.
    pub config: DapperConfig,
}

#[derive(Clone)]
pub struct McpHandler {
    control_port: Option<Port>,
    scope_id: Option<ScopeId>,
    /// Where to discover active sessions.
    sessions: SessionStore,
    tool_router: ToolRouter<McpHandler>,
    config: DapperConfig,
    /// Cached client for connection reuse.
    cached_client: Arc<RwLock<Option<CachedClient>>>,
    /// Tracks the last session this MCP server instance interacted with, so
    /// that when the caller omits `session_id` we fall back to *their* session
    /// rather than an arbitrary active session.
    last_session_id: Arc<Mutex<Option<SessionId>>>,
}

/// Shorthand for a successful text tool result.
fn ok_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![Content::text(text.into())])
}

/// Shorthand for a failed text tool result.
fn err_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![Content::text(text.into())])
}

#[tool_router]
impl McpHandler {
    /// Tools available in every toolset.
    fn always_available_tools() -> [&'static str; 4] {
        [
            DebugTool::Status.into(),
            DebugTool::Capabilities.into(),
            DebugTool::Sessions.into(),
            DebugTool::Config.into(),
        ]
    }

    pub fn new(env: McpServerEnv, toolset: &Toolset) -> Self {
        let mut tool_router = Self::tool_router();

        // Strip tools not in the active toolset (always-available tools are kept)
        let always_available = Self::always_available_tools();
        let all_tools: Vec<_> = tool_router.map.keys().cloned().collect();
        for tool in all_tools {
            if !toolset.contains_tool(tool.as_ref()) && !always_available.contains(&tool.as_ref()) {
                tool_router.remove_route(&tool);
            }
        }

        Self {
            control_port: env.control_port,
            scope_id: env.scope_id,
            sessions: env.sessions,
            tool_router,
            config: env.config,
            cached_client: Arc::new(RwLock::new(None)),
            last_session_id: Arc::new(Mutex::new(None)),
        }
    }

    fn last_session(&self) -> MutexGuard<'_, Option<SessionId>> {
        self.last_session_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn cached(&self) -> RwLockReadGuard<'_, Option<CachedClient>> {
        self.cached_client
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn cached_mut(&self) -> RwLockWriteGuard<'_, Option<CachedClient>> {
        self.cached_client
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn set_last_session_id(&self, session_id: &SessionId) {
        *self.last_session() = Some(session_id.clone());
    }

    /// " in scope '<id>'" when a scope filter is active, empty otherwise.
    fn scope_suffix(&self) -> String {
        self.scope_id
            .as_ref()
            .map_or(String::new(), |s| format!(" in scope '{}'", s))
    }

    /// Resolve the target session from an optional explicit session ID.
    fn resolve_session(&self, session_id: Option<&SessionId>) -> anyhow::Result<SessionInfo> {
        if let Some(explicit_port) = self.control_port {
            self.sessions
                .iter_active_sessions(self.scope_id.clone())
                .find(|s| s.control_plane_port.map(|p| p.get()) == Some(explicit_port.get()))
                .ok_or_else(|| anyhow::anyhow!("no session found on port {}", explicit_port.get()))
        } else if let Some(id) = session_id {
            let session = self
                .sessions
                .find_active_session_with_id(self.scope_id.clone(), id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Session '{}' not found or not active{}.",
                        id,
                        self.scope_suffix()
                    )
                })?;
            self.set_last_session_id(id);
            Ok(session)
        } else {
            let from_last = self.last_session().as_ref().and_then(|id| {
                self.sessions
                    .find_active_session_with_id(self.scope_id.clone(), id)
            });

            if let Some(session) = from_last {
                Ok(session)
            } else {
                let candidates: Vec<SessionInfo> = self
                    .sessions
                    .iter_active_sessions(self.scope_id.clone())
                    .collect();
                let session = resolve_unique_session(
                    candidates,
                    &self.scope_id,
                    Some("MCP tool calls also accept a session_id argument."),
                )?;
                self.set_last_session_id(&session.session_id);
                Ok(session)
            }
        }
    }

    /// Get a DapperControlPlaneClient for the specified session.
    ///
    /// Fast path: when the target session can be determined without touching the
    /// filesystem (no fixed `control_port`) and it matches the cached client, we
    /// return the cached client directly. This skips the per-call sessions
    /// directory scan in `resolve_session()` — a `read_dir` plus a JSON parse and
    /// liveness probe (`is_process_alive` + `TcpListener::bind`) for *every*
    /// active session — which dominates latency when an agent (e.g. heal) issues
    /// many tool calls in tight succession against a single session.
    ///
    /// The fast path still re-validates the cached session with an O(1)
    /// `is_active()` check (a single process-liveness check + one port probe) —
    /// not the O(N) directory scan — so a dead remembered session correctly
    /// falls through to the slow path, which re-resolves and falls back to
    /// another active session.
    fn get_client(
        &self,
        session_id: Option<&SessionId>,
    ) -> anyhow::Result<Arc<DapperControlPlaneClient>> {
        let start = Instant::now();

        // Fast path: resolve the target session id cheaply (no FS scan) and
        // return the cached client if it matches. Skipped when a fixed
        // `control_port` is configured, since that path resolves by port and the
        // cached client is not keyed by port here.
        if self.control_port.is_none() {
            let want_id: Option<SessionId> = match session_id {
                Some(id) => Some(id.clone()),
                None => self.last_session().clone(),
            };
            if let Some(want) = want_id {
                // Pull the matching client + a snapshot of its session out from
                // under the lock, then probe liveness *after* releasing it — the
                // probe does syscalls (and on some platforms `is_process_alive`
                // spawns a subprocess), which should not be held under the lock.
                let candidate = {
                    let cache = self.cached();
                    cache
                        .as_ref()
                        .filter(|cached| cached.session.session_id == want)
                        .map(|cached| (Arc::clone(&cached.client), cached.session.clone()))
                };
                if let Some((client, session)) = candidate
                    && session.is_active()
                {
                    tracing::info!(
                        cache_hit = true,
                        lookup_duration_us = start.elapsed().as_micros() as u64,
                        control_plane_session_id = %want,
                        "dapper_mcp_session_lookup"
                    );
                    return Ok(client);
                }
            }
        }

        // Slow path: resolve the session (FS scan + liveness check), then reuse
        // the cached gRPC client if it targets the resolved session, otherwise
        // construct and cache a new one.
        let session = self.resolve_session(session_id)?;
        let resolved_id = session.session_id.clone();

        let reused = self
            .cached()
            .as_ref()
            .filter(|cached| cached.session.session_id == resolved_id)
            .map(|cached| Arc::clone(&cached.client));

        let client = match reused {
            Some(client) => client,
            None => {
                let client = Arc::new(match session.control_plane_port {
                    Some(port) => DapperControlPlaneClient::for_port(port),
                    None => DapperControlPlaneClient::discover(
                        self.sessions.clone(),
                        self.scope_id.clone(),
                    ),
                });
                *self.cached_mut() = Some(CachedClient {
                    client: Arc::clone(&client),
                    session,
                });
                client
            }
        };

        tracing::info!(
            cache_hit = false,
            lookup_duration_us = start.elapsed().as_micros() as u64,
            control_plane_session_id = %resolved_id,
            "dapper_mcp_session_lookup"
        );
        Ok(client)
    }

    /// Helper to get client or return a CallToolResult error
    fn get_client_or_error(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Arc<DapperControlPlaneClient>, CallToolResult> {
        self.get_client(session_id)
            .map_err(|e| err_text(format!("Error connecting to session: {:#}", e)))
    }

    /// Bound an already-rendered DAP payload, spilling oversized text to a
    /// temp file off the async runtime.
    async fn bounded_dap_text(text: String) -> String {
        let rendered = RenderedResponse::from_text(text);
        tokio::task::spawn_blocking(move || rendered.spill_to_temp_and_render())
            .await
            .expect("response spill task should not panic")
    }

    /// The shared skeleton of every plaintext-rendering tool: resolve the
    /// target client, run `f` against it, render success with the handler's
    /// config, and prefix errors with `err_ctx`.
    async fn run_rendered<T: std::fmt::Display>(
        &self,
        session_id: Option<&SessionId>,
        err_ctx: &str,
        f: impl AsyncFnOnce(Arc<DapperControlPlaneClient>) -> anyhow::Result<ControlPlaneResult<T>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(session_id) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        Ok(match f(client).await {
            Ok(result) => ok_text(render_plaintext(&result, &self.config)),
            Err(e) => err_text(format!("{err_ctx}: {e:#}")),
        })
    }

    #[tool(
        description = "Get the current debug session status, including execution state (stop reason, signal, crashed thread ID), active breakpoints, and session metadata. This is a lightweight command that does not interact with the debugger — use it as a first step to understand the session state before issuing other debug commands."
    )]
    async fn debug_status_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted { session_id, .. }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        Ok(match client.status().await {
            Ok(result) => {
                let config = DapperConfig {
                    context: dapper_config::ContextConfig::all_enabled(),
                    ..self.config.clone()
                };
                ok_text(render_plaintext(&result, &config))
            }
            Err(e) => err_text(format!("Error getting status: {:#}", e)),
        })
    }

    #[tool(
        description = "Execute the `threads` command in the debugger. The `threads` command lists all available threads in the debugged process. May also include the first thread's stack trace, along with the topmost frame's scopes and top-level local variable values."
    )]
    async fn debug_threads_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted { session_id, .. }) = request;
        self.run_rendered(session_id.as_ref(), "Error getting threads", async |c| {
            c.threads().await
        })
        .await
    }

    #[tool(
        description = "Navigate debugger execution by stepping through code or continuing the program. Supports seven types of navigation: 'step_in' (step into functions), 'step_over' (step over functions - most commonly used), 'step_out' (step out of current frame), 'continue' (resume execution until breakpoint or exit), 'pause' (pause a running program), 'step_back' (step back one source line), and 'reverse_continue' (resume reverse execution until a breakpoint or the start of recording). The reverse navigation types ('step_back' and 'reverse_continue') require the connected adapter to advertise the DAP capability 'supportsStepBack'; otherwise the request is rejected without contacting the adapter."
    )]
    async fn debug_navigate_command(
        &self,
        request: Parameters<SessionTargeted<NavigateRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                NavigateRequest {
                    thread_id,
                    navigation_type,
                    single_thread,
                },
        }) = request;
        self.run_rendered(
            session_id.as_ref(),
            &format!("Error executing navigate {} command", navigation_type),
            async |c| c.navigate(navigation_type, thread_id, single_thread).await,
        )
        .await
    }

    #[tool(
        description = "Execute the `stack-trace` command in the debugger. The `stack-trace` command prints the current stack trace showing the call hierarchy for the specified thread. May also include the topmost frame's scopes with top-level local variable values."
    )]
    async fn debug_stack_trace_command(
        &self,
        request: Parameters<SessionTargeted<StackTraceRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                StackTraceRequest {
                    thread_id,
                    start_frame,
                    levels,
                },
        }) = request;
        self.run_rendered(
            session_id.as_ref(),
            "Error getting stack trace",
            async |c| c.stack_trace(thread_id, start_frame, levels).await,
        )
        .await
    }

    #[tool(
        description = "Execute the `scopes` command in the debugger. The `scopes` command lists all available scopes (like local variables, arguments, etc.) for the specified stack frame. May also include expanded top-level variable values for the 'Locals' scope."
    )]
    async fn debug_scopes_command(
        &self,
        request: Parameters<SessionTargeted<FrameIdRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner: FrameIdRequest { frame_id },
        }) = request;
        self.run_rendered(session_id.as_ref(), "Error getting scopes", async |c| {
            c.scopes(frame_id).await
        })
        .await
    }

    #[tool(
        description = "Execute the `variables` command in the debugger. The `variables` command lists all variables for the specified variables reference (obtained from scopes command or from other variables)."
    )]
    async fn debug_variables_command(
        &self,
        request: Parameters<SessionTargeted<VariablesReferenceRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner: VariablesReferenceRequest {
                variables_reference,
            },
        }) = request;
        self.run_rendered(session_id.as_ref(), "Error getting variables", async |c| {
            c.variables(variables_reference).await
        })
        .await
    }

    #[tool(
        description = "Execute the `set-variable` command in the debugger. The `set-variable` command sets the value of the specified variable to the given value. If you are setting strings make sure to add additional single quotes to quote them."
    )]
    async fn debug_set_variable_command(
        &self,
        request: Parameters<SessionTargeted<SetVariableRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                SetVariableRequest {
                    variables_reference,
                    name,
                    value,
                },
        }) = request;
        self.run_rendered(session_id.as_ref(), "Error setting variable", async |c| {
            c.set_variable(variables_reference, &name, &value).await
        })
        .await
    }

    #[tool(
        description = r#"Execute the `setBreakpoints` command in the debugger. Adds breakpoints at the specified lines in a source file. When clear_existing is false (default), new breakpoints are appended to existing ones. When clear_existing is true, all existing breakpoints are removed first.

Each breakpoint specifies a line and optionally:
- `condition`: expression that must evaluate to true for the breakpoint to stop execution
- `logMessage`: message to log instead of stopping; expressions within {} are interpolated

Example:
  {"source_path": "/path/to/file.py", "breakpoints": [
    {"line": 10, "condition": "x > 5"},
    {"line": 20, "logMessage": "value of x is {x}"},
    {"line": 30}
  ]}"#
    )]
    async fn debug_set_breakpoints_command(
        &self,
        request: Parameters<SessionTargeted<SetBreakpointsRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                SetBreakpointsRequest {
                    source_path,
                    breakpoints,
                    clear_existing,
                },
        }) = request;
        let breakpoint_specs: Vec<SourceBreakpoint> = breakpoints
            .into_iter()
            .map(SourceBreakpoint::from)
            .collect();
        self.run_rendered(session_id.as_ref(), "Error setting breakpoint", async |c| {
            c.set_breakpoints(&source_path, clear_existing, &breakpoint_specs)
                .await
        })
        .await
    }

    #[tool(
        description = r#"Execute the `setExceptionBreakpoints` command in the debugger. Configures which adapter-advertised exception filters cause the debuggee to stop.

Discover supported filter ids via `debug_capabilities_command` — they vary by adapter (e.g. debugpy: `raised`/`uncaught`/`userUnhandled`; lldb-dap: `cpp_throw`/`cpp_catch`; vscode-js-debug: `all`/`uncaught`).

Behavior:
- `clear_existing: false` (default): the explicit list is merged with the currently-installed set; existing conditions on re-specified filters are preserved (this surface has no way to express conditions, so re-specifying must not silently drop them).
- `clear_existing: true`: replaces the active set verbatim.
- Empty `filters` with `clear_existing: true`: clears all installed filters.
- Empty `filters` with `clear_existing: false`: rejected as a usage error (almost certainly a mistake).

Per-filter conditions (the DAP `filterOptions` mechanism) are not yet exposed by this tool — only bare filter ids are accepted, even if `debug_capabilities_command` advertises `supports_condition: true` for some filters.

Example:
  {"filters": ["raised", "uncaught"], "clear_existing": true}"#
    )]
    async fn debug_set_exception_breakpoints_command(
        &self,
        request: Parameters<SessionTargeted<SetExceptionBreakpointsRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                SetExceptionBreakpointsRequest {
                    filters,
                    clear_existing,
                },
        }) = request;

        // Strict empty-input validation (per design): empty + !clear is a
        // no-op, which is almost certainly a user/agent mistake. The
        // library layer (`ProxyClient::set_exception_breakpoints`) is
        // forgiving and silently no-ops; we reject here at the
        // user-facing surface so the LLM gets clear feedback.
        //
        // Intentionally validates *before* resolving the client so the
        // error message is the actionable one (about the missing filter)
        // rather than a confusing "no active session" — the caller can
        // fix the request shape regardless of session state.
        if filters.is_empty() && !clear_existing {
            return Ok(err_text(
                "specify at least one filter or set clear_existing: true to disable all exception breakpoints",
            ));
        }

        self.run_rendered(
            session_id.as_ref(),
            "Error setting exception breakpoints",
            async |c| c.set_exception_breakpoints(&filters, clear_existing).await,
        )
        .await
    }

    #[tool(
        description = "Execute the `evaluate` command in the debugger. The `evaluate` command evaluates an expression in the REPL context and returns the result. Depending on the debugger, the input can be a debugger command or a valid expression in the debugged language. Optionally, a frame_id can be provided to evaluate the expression in the context of a specific stack frame."
    )]
    async fn debug_evaluate_command(
        &self,
        request: Parameters<SessionTargeted<EvaluateRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                EvaluateRequest {
                    expression,
                    frame_id,
                },
        }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        Ok(match client.eval_repl(&expression, frame_id).await {
            Ok(result) => ok_text(result),
            Err(e) => err_text(format!("Error evaluating expression: {:#}", e)),
        })
    }

    #[tool(
        description = "Stop the dapper proxy server. This command aborts the proxy server task, shutting down the dapper infrastructure for this debug session."
    )]
    async fn debug_stop_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted { session_id, .. }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        // When stopping, the server shuts down immediately. Connection errors
        // are expected.
        let _ = client.stop().await;
        Ok(ok_text("Dapper proxy server stopped."))
    }

    #[tool(
        description = "List all active debug sessions. Returns session IDs, types (e.g. fb-lldb, fdb-debug/javadap), control ports, scope IDs, and other metadata. Call this to discover available sessions. When multiple sessions exist (e.g., dual-attach C++/Java debugging), pass the desired session_id to other debug commands to target a specific session."
    )]
    async fn debug_sessions_command(
        &self,
        _request: Parameters<EmptyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let sessions: Vec<SessionInfo> = self
            .sessions
            .iter_active_sessions(self.scope_id.clone())
            .collect();
        if sessions.is_empty() {
            Ok(ok_text(format!(
                "No active debug sessions found{}.",
                self.scope_suffix()
            )))
        } else {
            let mut output = format!(
                "Found {} active session(s){}:\n\n",
                sessions.len(),
                self.scope_suffix()
            );
            for session in &sessions {
                let _ = writeln!(output, "{}", session);
            }
            Ok(ok_text(output))
        }
    }

    #[tool(
        description = "Get the debug session's launch/attach configuration and dapper settings. Returns the DAP launch/attach request arguments (debugger_args) and the active dapper configuration (output format, context settings, command defaults). This is a lightweight command that reads session metadata — it does not interact with the debugger."
    )]
    async fn debug_config_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted { session_id, .. }) = request;
        Ok(match self.resolve_session(session_id.as_ref()) {
            Ok(session) => {
                let output = serde_json::json!({
                    "debugger_args": session.debugger_args,
                    "dapper_config": self.config,
                });
                ok_text(
                    serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
                )
            }
            Err(e) => err_text(format!("Error resolving session: {:#}", e)),
        })
    }

    #[tool(
        description = "Query the debug adapter's capabilities. Returns which optional DAP features are supported (e.g., stepBack, dataBreakpoints, functionBreakpoints). Call this before using advanced commands to check if the adapter supports them."
    )]
    async fn debug_capabilities_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted { session_id, .. }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        Ok(match client.capabilities().await {
            Ok(Some(json)) => {
                let output = match serde_json::from_str::<serde_json::Value>(&json) {
                    Ok(value) => format_capabilities(&value),
                    Err(_) => json,
                };
                ok_text(output)
            }
            Ok(None) => ok_text(
                "Adapter capabilities not yet available (initialize response not received).",
            ),
            Err(e) => err_text(format!("Error querying capabilities: {:#}", e)),
        })
    }

    #[tool(
        description = r#"Send any DAP (Debug Adapter Protocol) request to the debugger.

TYPICAL WORKFLOW:
  threads → stackTrace(threadId) → scopes(frameId) → variables(ref)
  Most inspection commands require the debuggee to be stopped (at a breakpoint or paused).

COMMANDS & ARGUMENTS (* = required, ? = optional):

Inspection (debuggee must be stopped):
- threads: {} → list all threads (start here to get threadIds)
- stackTrace: {*threadId, startFrame?, levels?} → stack frames (gives frameIds)
- scopes: {*frameId} → variable scopes (gives variablesReferences)
- variables: {*variablesReference, filter?: "indexed"|"named", count?} → child variables
- evaluate: {*expression, frameId?, context?: "repl"|"watch"|"hover"} → eval expression
- exceptionInfo: {*threadId} → details of current exception
- source: {sourceReference?, source?} → retrieve source code
- completions: {*text, *column, frameId?} → REPL tab-completions

Execution Control (use wait_for_event: true):
- continue: {*threadId} → resume execution
- pause: {*threadId} → pause a running thread
- next: {*threadId, granularity?: "statement"|"line"|"instruction"} → step over
- stepIn: {*threadId, targetId?, granularity?} → step into
- stepOut: {*threadId, granularity?} → step out of current frame
- stepBack: {*threadId, granularity?} → reverse step (rr/replay-style adapters; also exposed as debug_navigate_command navigation_type=step_back)
- reverseContinue: {*threadId} → reverse continue (rr/replay-style adapters; also exposed as debug_navigate_command navigation_type=reverse_continue)
- goto: {*threadId, *targetId} → jump to location (get targetId from gotoTargets)
- restartFrame: {*frameId} → restart a stack frame

Breakpoints:
- setBreakpoints: {*source: {path}, *breakpoints: [{*line, condition?, logMessage?, hitCondition?}]}
- setFunctionBreakpoints: {*breakpoints: [{*name, condition?, hitCondition?}]}
- setExceptionBreakpoints: {*filters: ["uncaught", "raised", ...]}
- setDataBreakpoints: {*breakpoints: [{*dataId, condition?}]} (get dataId from dataBreakpointInfo)
- setInstructionBreakpoints: {*breakpoints: [{*instructionReference, offset?, condition?}]}
- breakpointLocations: {*source: {path}, *line, endLine?} → valid breakpoint positions
- dataBreakpointInfo: {*name, variablesReference?} → get dataId for data breakpoints

Modification (debuggee must be stopped):
- setVariable: {*variablesReference, *name, *value} → change a variable's value
- setExpression: {*expression, *value, frameId?} → set value via evaluateName

Targets (for multi-target commands):
- stepInTargets: {*frameId} → possible step-in targets when line has multiple calls
- gotoTargets: {*source: {path}, *line} → valid goto targets

Info:
- modules: {startModule?, moduleCount?} → loaded modules/libraries
- loadedSources: {} → all source files (may be slow on some adapters)

Memory (low-level):
- readMemory: {*memoryReference, *count, offset?} → read bytes (base64 encoded)
- writeMemory: {*memoryReference, *data, offset?, allowPartial?} → write bytes (base64)
- disassemble: {*memoryReference, *instructionCount, offset?, resolveSymbols?} → disassembly

Use wait_for_event: true for execution-affecting commands (continue, pause, step*, goto).
Response is JSON from the debug adapter."#
    )]
    async fn debug_dap_request(
        &self,
        request: Parameters<SessionTargeted<RawDapRequestParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                RawDapRequestParams {
                    command,
                    arguments,
                    wait_for_event,
                    timeout_seconds,
                },
        }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        // Tolerate stringified-JSON arguments (LLM clients sometimes
        // double-encode); anything unparsable passes through verbatim.
        let arguments = match arguments {
            Some(serde_json::Value::String(s)) => {
                match serde_json::from_str::<serde_json::Value>(&s) {
                    Ok(parsed) => Some(parsed),
                    Err(_) => Some(serde_json::Value::String(s)),
                }
            }
            other => other,
        };

        Ok(
            match client
                .send_dap_request(&command, arguments, wait_for_event, timeout_seconds)
                .await
            {
                Ok(result) => ok_text(Self::bounded_dap_text(result.to_string()).await),
                Err(e) => err_text(format!("DAP request '{}' failed: {:#}", command, e)),
            },
        )
    }

    #[tool(
        description = "Read memory from the debugged process at a given address. Returns a formatted hex dump with addresses and ASCII sidebar. Use `debug_evaluate_command` to obtain memory references from expressions (e.g., `&myVariable`)."
    )]
    async fn debug_read_memory_command(
        &self,
        request: Parameters<SessionTargeted<ReadMemoryRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                ReadMemoryRequest {
                    memory_reference,
                    count,
                    offset,
                },
        }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        if count <= 0 {
            return Ok(err_text(format!("count must be > 0, got {}", count)));
        }
        if count > MAX_READ_BYTES {
            return Ok(err_text(format!(
                "count {} exceeds maximum of {} bytes",
                count, MAX_READ_BYTES
            )));
        }

        let mut args = serde_json::json!({
            "memoryReference": memory_reference,
            "count": count,
        });
        if let Some(offset) = offset {
            args["offset"] = serde_json::json!(offset);
        }

        Ok(
            match client
                .send_dap_request("readMemory", Some(args), false, default_timeout())
                .await
            {
                Ok(result) => match &result.body {
                    ResponseBody::ReadMemory(Some(body)) => match format_memory_read(body) {
                        Ok(text) => ok_text(text),
                        Err(e) => err_text(format!("Error decoding memory response: {}", e)),
                    },
                    ResponseBody::ReadMemory(None) => {
                        ok_text("No memory data returned by the debug adapter.")
                    }
                    _ => ok_text(Self::bounded_dap_text(result.to_string()).await),
                },
                Err(e) => err_text(format!("Error reading memory: {:#}", e)),
            },
        )
    }

    #[tool(
        description = "Write memory to the debugged process at a given address. WARNING: This directly modifies process memory and can crash the debuggee if misused. The data parameter is a hex string (e.g., \"48656C6C6F\" writes the bytes for \"Hello\")."
    )]
    async fn debug_write_memory_command(
        &self,
        request: Parameters<SessionTargeted<WriteMemoryRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(SessionTargeted {
            session_id,
            inner:
                WriteMemoryRequest {
                    memory_reference,
                    data,
                    offset,
                },
        }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        // Convert user-provided hex string to base64 for DAP protocol
        let raw_bytes = match hex_string_to_bytes(&data) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Ok(err_text(format!("{e:#}")));
            }
        };
        let b64_data = base64::engine::general_purpose::STANDARD.encode(&raw_bytes);

        let mut args = serde_json::json!({
            "memoryReference": memory_reference,
            "data": b64_data,
        });
        if let Some(offset) = offset {
            args["offset"] = serde_json::json!(offset);
        }

        Ok(
            match client
                .send_dap_request("writeMemory", Some(args), false, default_timeout())
                .await
            {
                Ok(result) => match &result.body {
                    ResponseBody::WriteMemory(Some(body)) => {
                        let written = body.bytes_written.unwrap_or(raw_bytes.len() as i64);
                        ok_text(format!(
                            "Successfully wrote {} byte(s) to {}.",
                            written, memory_reference
                        ))
                    }
                    ResponseBody::WriteMemory(None) => {
                        ok_text(format!("Write completed to {}.", memory_reference))
                    }
                    _ => ok_text(Self::bounded_dap_text(result.to_string()).await),
                },
                Err(e) => err_text(format!("Error writing memory: {:#}", e)),
            },
        )
    }

    #[tool(
        description = "Composite snapshot of all threads in the debugged process. Equivalent to calling debug_threads_command followed by debug_stack_trace_command for each thread, but parallelized and returned as a single structured JSON payload. Returns raw DAP data (thread id, name, stack frames) with no classification, filtering, or analysis — downstream callers (e.g. heal) layer those on top. Use this to avoid N+1 round-trips when inspecting all threads at once."
    )]
    async fn debug_thread_snapshot(
        &self,
        request: Parameters<SessionTargeted<ThreadSnapshotRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        use futures::stream::StreamExt;
        use futures::stream::{self};

        let Parameters(SessionTargeted {
            session_id,
            inner: req,
        }) = request;
        let client = match self.get_client_or_error(session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        let threads_result = match client.threads().await {
            Ok(r) => r.result,
            Err(e) => {
                return Ok(err_text(format!("Error getting threads: {}", e)));
            }
        };

        let (stack_depth, max_threads) = clamp_snapshot_limits(&req);

        let total_thread_count = threads_result.threads.len();
        let truncated = total_thread_count > max_threads;
        let visible_threads: Vec<_> = threads_result
            .threads
            .into_iter()
            .take(max_threads)
            .collect();

        let entries: Vec<serde_json::Value> = if req.include_stacks {
            let client = client.clone();
            stream::iter(visible_threads)
                .map(|thread| {
                    let client = client.clone();
                    async move {
                        let mut entry = serde_json::json!({
                            "id": thread.id.0,
                            "name": thread.name,
                        });
                        match client.stack_trace(thread.id, None, Some(stack_depth)).await {
                            Ok(st) => {
                                entry["stack_frames"] =
                                    serde_json::to_value(&st.result.stack_frames)
                                        .expect("StackFrame is plain serializable");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    thread_id = thread.id.0,
                                    error = %e,
                                    "stack_trace failed for thread; surfacing in stack_error",
                                );
                                entry["stack_error"] = serde_json::Value::String(e.to_string());
                            }
                        }
                        entry
                    }
                })
                .buffer_unordered(16)
                .collect()
                .await
        } else {
            visible_threads
                .into_iter()
                .map(|thread| {
                    serde_json::json!({
                        "id": thread.id.0,
                        "name": thread.name,
                    })
                })
                .collect()
        };

        let response = serde_json::json!({
            "thread_count": entries.len(),
            "total_thread_count": total_thread_count,
            "truncated": truncated,
            "threads": entries,
        });

        Ok(ok_text(serde_json::to_string_pretty(&response).expect(
            "serde_json::Value always serializes successfully",
        )))
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "DAP proxy server providing MCP clients access to active debugger sessions. Call debug_sessions_command to list available sessions. When multiple sessions exist (e.g., dual-attach C++/Java debugging), specify session_id in subsequent commands to target the correct session.",
        )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let tool_name = request.name.clone();
        let mcp_client_name = context
            .peer
            .peer_info()
            .map(|info| info.client_info.name.to_string())
            .unwrap_or_default();

        let tcc = ToolCallContext::new(self, request, context);
        let result = self.tool_router.call(tcc).await;

        // Read the proxy session ID from the cached client (set by get_client
        // during tool execution). This lets us correlate MCP tool calls back to
        // the proxy session they targeted.
        let cached = self.cached();
        let proxy_session_id = cached
            .as_ref()
            .map(|c| c.session.session_id.as_str())
            .unwrap_or("");

        match &result {
            Ok(r) if r.is_error.unwrap_or(false) => {
                tracing::warn!(
                    mcp_tool_name = %tool_name,
                    mcp_client_name = %mcp_client_name,
                    proxy_session_id,
                    "tool_call_error"
                );
            }
            Err(e) => {
                tracing::error!(
                    mcp_tool_name = %tool_name,
                    mcp_client_name = %mcp_client_name,
                    proxy_session_id,
                    error = %e,
                    "tool_call_failed"
                );
            }
            Ok(_) => {
                tracing::info!(
                    mcp_tool_name = %tool_name,
                    mcp_client_name = %mcp_client_name,
                    proxy_session_id,
                    "tool_call"
                );
            }
        }

        result
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        if let Some(peer_info) = context.peer.peer_info() {
            match serde_json::to_string(&*peer_info) {
                Ok(peer_info_json) => {
                    tracing::info!(
                        mcp_client_name = %peer_info.client_info.name,
                        client_version = %peer_info.client_info.version,
                        protocol_version = %peer_info.protocol_version,
                        peer_info = %peer_info_json,
                        "MCP client '{}' connected", peer_info.client_info.name
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        mcp_client_name = %peer_info.client_info.name,
                        error = %e,
                        "MCP client connected but failed to serialize peer info"
                    );
                }
            }
        } else {
            tracing::warn!("MCP client connected but peer info is not available");
        }
    }
}

#[cfg(test)]
mod tests;
