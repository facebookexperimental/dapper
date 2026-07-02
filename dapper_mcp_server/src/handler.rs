// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::Instant;

use base64::Engine as _;
use dapper_config::DapperConfig;
use dapper_control_api::DapperControlPlane;
use dapper_control_api::DapperControlPlaneClient;
use dapper_control_api::NavigationType;
use dapper_control_api::render_plaintext;
use dapper_control_api::resolve_unique_session;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_dap_protocol::responses::ReadMemoryResponseBody;
use dapper_dap_protocol::responses::ResponseBody;
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::SessionInfo;
use rmcp::ErrorData;
use rmcp::ServerHandler;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::Content;
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
use schemars::JsonSchema;
use schemars::Schema;
use schemars::generate::SchemaGenerator;

use crate::toolsets::DebugTool;
use crate::toolsets::Toolset;

struct CachedClient {
    client: Arc<DapperControlPlaneClient>,
    /// Full session info for the cached client, so the fast path in
    /// `get_client` can cheaply re-validate liveness (`is_active`) without a
    /// full sessions-directory scan.
    session: SessionInfo,
}

#[derive(Clone)]
pub struct McpHandler {
    control_port: Option<Port>,
    scope_id: Option<ScopeId>,
    tool_router: ToolRouter<McpHandler>,
    config: DapperConfig,
    /// Cached client for connection reuse.
    cached_client: Arc<RwLock<Option<CachedClient>>>,
    /// Tracks the last session this MCP server instance interacted with, so
    /// that when the caller omits `session_id` we fall back to *their* session
    /// rather than an arbitrary active session.
    last_session_id: Arc<Mutex<Option<SessionId>>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EmptyParams {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SessionTargeted<T> {
    /// Id of the debug session to send the request to. If left unspecified, requests are sent to the last session this MCP server used (if still active), or the oldest active session otherwise.
    #[serde(default)]
    #[schemars(schema_with = "optional_string_schema")]
    session_id: Option<SessionId>,
    #[serde(flatten)]
    pub inner: T,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StackTraceRequest {
    /// The thread id to execute the command on. To obtain thread ids, call `debug_threads_command` first.
    #[schemars(schema_with = "integer_schema")]
    thread_id: ThreadId,
    /// The index of the first frame to return. If omitted, frames start at index 0.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_integer_schema")]
    start_frame: Option<i64>,
    /// Maximum number of stack frames to return. If not specified, uses the configured default. Set to 0 to return all frames.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_integer_schema")]
    levels: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FrameIdRequest {
    /// The frame id to execute the command on. To obtain frame ids, call `debug_stack_trace_command` first.
    #[schemars(schema_with = "integer_schema")]
    frame_id: FrameId,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VariablesReferenceRequest {
    /// The variable reference to execute the command on. Variable references are obtained from both `debug_scopes_command` (or other `debug_variables_command` calls when we want to look at nested variables). Note that variable references need to be re-obtained every time the debugger stops.
    #[schemars(schema_with = "integer_schema")]
    variables_reference: VariablesReference,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetVariableRequest {
    /// The variable reference to execute the command on. Variable references are obtained from both `debug_scopes_command` (or other `debug_variables_command` calls when we want to look at nested variables). Note that variable references need to be re-obtained every time the debugger stops.
    #[schemars(schema_with = "integer_schema")]
    variables_reference: VariablesReference,
    /// Name of the variable to set
    name: String,
    /// New value for the variable. Note that string values need to be quoted with single quotes.
    value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NavigateRequest {
    /// The thread id to execute the command on. To obtain thread ids, call `debug_threads_command` first.
    #[schemars(schema_with = "integer_schema")]
    thread_id: ThreadId,
    /// The type of navigation to perform: "step_in" (step into functions), "step_over" (step over functions - most commonly used), "step_out" (step out of current frame), "continue" (resume execution until breakpoint or exit), "pause" (pause a running program), "step_back" (step back one source line, requires adapter `supportsStepBack`), or "reverse_continue" (resume reverse execution until a breakpoint or the start of recording, requires adapter `supportsStepBack`)
    navigation_type: NavigationType,
    /// When true, only the specified thread is resumed; other suspended threads remain paused. Requires the adapter to advertise `supportsSingleThreadExecutionRequests`. If omitted or false, all threads are resumed.
    #[serde(default)]
    #[schemars(schema_with = "optional_boolean_schema")]
    single_thread: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub struct BreakpointSpec {
    line: i64,
    condition: Option<String>,
    log_message: Option<String>,
}

impl JsonSchema for BreakpointSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "BreakpointSpec".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        #[derive(JsonSchema)]
        #[allow(dead_code)]
        struct BreakpointSpecSchema {
            /// The line number where the breakpoint should be set.
            #[schemars(schema_with = "integer_schema")]
            line: i64,
            /// An optional expression that controls when a breakpoint is hit. The breakpoint only stops execution when this expression evaluates to true.
            #[serde(default)]
            #[schemars(schema_with = "optional_string_schema")]
            condition: Option<String>,
            /// If specified, the debugger will log this message instead of stopping at the breakpoint. Expressions within {} are interpolated.
            #[serde(default)]
            #[schemars(rename = "logMessage", schema_with = "optional_string_schema")]
            log_message: Option<String>,
        }
        BreakpointSpecSchema::json_schema(generator)
    }
}

impl TryFrom<serde_json::Value> for BreakpointSpec {
    type Error = String;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        #[derive(serde::Deserialize)]
        struct Inner {
            #[serde(deserialize_with = "deserialize_string_or_int")]
            line: i64,
            #[serde(default)]
            condition: Option<String>,
            #[serde(default, rename = "logMessage")]
            log_message: Option<String>,
        }

        match &value {
            serde_json::Value::Object(_) => {}
            serde_json::Value::String(s) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                    return BreakpointSpec::try_from(parsed);
                }
                let line: i64 = s.parse().map_err(|_| {
                    format!(
                        "invalid breakpoint spec: expected a JSON object like \
                         {{\"line\": 10}} or a line number, got {s:?}"
                    )
                })?;
                return Ok(BreakpointSpec {
                    line,
                    condition: None,
                    log_message: None,
                });
            }
            serde_json::Value::Number(n) => {
                let line = n.as_i64().ok_or("breakpoint line must be an integer")?;
                return Ok(BreakpointSpec {
                    line,
                    condition: None,
                    log_message: None,
                });
            }
            _ => {
                return Err(
                    "invalid breakpoint spec: expected a JSON object like {\"line\": 10}, \
                     a line number, or a JSON string"
                        .to_string(),
                );
            }
        }

        let inner: Inner = serde_json::from_value(value).map_err(|e| e.to_string())?;
        Ok(BreakpointSpec {
            line: inner.line,
            condition: inner.condition,
            log_message: inner.log_message,
        })
    }
}

impl From<BreakpointSpec> for SourceBreakpoint {
    fn from(spec: BreakpointSpec) -> Self {
        Self {
            line: spec.line,
            condition: spec.condition,
            log_message: spec.log_message,
            ..Default::default()
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetBreakpointsRequest {
    /// The absolute path to the source file where breakpoints should be set.
    source_path: String,
    /// Per-breakpoint specifications. Each entry specifies a line and optional condition/logMessage.
    breakpoints: Vec<BreakpointSpec>,
    /// When true, clears all existing breakpoints in the file before adding new ones.
    /// When false (default), new breakpoints are appended to existing ones.
    #[serde(default)]
    clear_existing: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetExceptionBreakpointsRequest {
    /// Adapter-advertised exception filter ids (e.g. "raised", "uncaught").
    /// Discover supported ids via `debug_capabilities_command`'s
    /// `exceptionBreakpointFilters` section.
    #[serde(default)]
    filters: Vec<String>,
    /// When true, clears all existing exception filters before enabling
    /// these. When false (default), the explicit list is merged with the
    /// currently-installed set; existing conditions on re-specified
    /// filters are preserved.
    #[serde(default)]
    clear_existing: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EvaluateRequest {
    /// The input to evaluate in the REPL context. Depending on the debugger, this can be a debugger command or a valid expression in the debugged language to be evaluated.
    expression: String,
    /// The stack frame in which to evaluate the expression. If omitted, the expression is evaluated in the global scope. To obtain frame ids, call `debug_stack_trace_command` first.
    #[serde(default)]
    #[schemars(schema_with = "optional_integer_schema")]
    frame_id: Option<FrameId>,
}

fn default_timeout() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn default_stack_depth() -> i64 {
    10
}

fn default_max_threads() -> i64 {
    50
}

/// Hard upper bound on threads enumerated per snapshot, regardless of caller request.
/// Protects against pathological binaries (tens of thousands of threads) where the
/// raw stack output would be unmanageable for both transport and the LLM client.
const MAX_THREADS_HARD_CAP: usize = 500;

/// Hard upper bound on stack frames per thread.
const MAX_STACK_DEPTH: i64 = 512;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RawDapRequestParams {
    /// The DAP command name (e.g., "threads", "pause", "setExceptionBreakpoints")
    command: String,
    /// Arguments as JSON object. Pass {} or omit if no arguments needed.
    #[serde(default)]
    arguments: Option<serde_json::Value>,
    /// Wait for stopped/exited events after request (for pause, continue, step commands)
    #[serde(default)]
    wait_for_event: bool,
    /// Timeout in seconds for event wait. Default: 60
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}

fn default_read_count() -> i64 {
    256
}

/// Upper bound on `count` for a single readMemory request (1 MiB). DAP itself
/// has no cap, so without this an MCP client could ask the adapter to materialize
/// arbitrarily large reads.
const MAX_READ_BYTES: i64 = 1 << 20;

/// Upper bound on the byte payload of a single writeMemory request (1 MiB).
/// Same reasoning as MAX_READ_BYTES — bound the allocation an MCP client can
/// force before the adapter sees the request.
const MAX_WRITE_BYTES: usize = 1 << 20;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadMemoryRequest {
    /// Memory reference address (e.g., "0x7fff5fbff8a0") or expression that evaluates to a memory address. Obtain memory references from `debug_evaluate_command` or variable `memoryReference` fields.
    memory_reference: String,
    /// Number of bytes to read. Must be > 0. Default: 256.
    #[serde(
        default = "default_read_count",
        deserialize_with = "deserialize_string_or_int"
    )]
    #[schemars(schema_with = "integer_schema")]
    count: i64,
    /// Byte offset from the memory reference.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_integer_schema")]
    offset: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WriteMemoryRequest {
    /// Memory reference address (e.g., "0x7fff5fbff8a0") or expression that evaluates to a memory address.
    memory_reference: String,
    /// Hex string of bytes to write (e.g., "48656C6C6F" to write "Hello"). Each pair of hex digits represents one byte.
    data: String,
    /// Byte offset from the memory reference.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_integer_schema")]
    offset: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ThreadSnapshotRequest {
    /// Include stack traces for each thread (default true).
    #[serde(default = "default_true")]
    include_stacks: bool,
    /// Maximum stack frames per thread (default 10, hard cap 512).
    #[serde(
        default = "default_stack_depth",
        deserialize_with = "deserialize_string_or_int"
    )]
    #[schemars(schema_with = "integer_schema")]
    stack_depth: i64,
    /// Maximum threads to enumerate (default 50, hard cap 500). Protects against
    /// pathological processes with tens of thousands of threads.
    #[serde(
        default = "default_max_threads",
        deserialize_with = "deserialize_string_or_int"
    )]
    #[schemars(schema_with = "integer_schema")]
    max_threads: i64,
}

#[tool_router]
impl McpHandler {
    pub fn new(control_port: Option<Port>, scope_id: Option<ScopeId>, toolset: &Toolset) -> Self {
        let mut tool_router = Self::tool_router();

        // Strip tools not in the active toolset (always-available tools are kept)
        let always_available: [&str; 4] = [
            DebugTool::Status.into(),
            DebugTool::Capabilities.into(),
            DebugTool::Sessions.into(),
            DebugTool::Config.into(),
        ];
        let all_tools: Vec<_> = tool_router.map.keys().cloned().collect();
        for tool in all_tools {
            if !toolset.contains_tool(tool.as_ref()) && !always_available.contains(&tool.as_ref()) {
                tool_router.remove_route(&tool);
            }
        }

        Self {
            control_port,
            scope_id,
            tool_router,
            config: DapperConfig::load_or_default(),
            cached_client: Arc::new(RwLock::new(None)),
            last_session_id: Arc::new(Mutex::new(None)),
        }
    }

    fn set_last_session_id(&self, session_id: &SessionId) -> anyhow::Result<()> {
        let mut last = self
            .last_session_id
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire last_session_id lock: {}", e))?;
        *last = Some(session_id.clone());
        Ok(())
    }

    /// Resolve the target session from an optional explicit session ID.
    fn resolve_session(&self, session_id: Option<&SessionId>) -> anyhow::Result<SessionInfo> {
        if let Some(explicit_port) = self.control_port {
            SessionInfo::iter_active_sessions(self.scope_id.clone())?
                .find(|s| s.control_plane_port.map(|p| p.get()) == Some(explicit_port.get()))
                .ok_or_else(|| anyhow::anyhow!("no session found on port {}", explicit_port.get()))
        } else if let Some(id) = session_id {
            let session = SessionInfo::find_active_session_with_id(self.scope_id.clone(), id)?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Session '{}' not found or not active{}.",
                        id,
                        self.scope_id
                            .as_ref()
                            .map_or(String::new(), |s| format!(" in scope '{}'", s))
                    )
                })?;
            self.set_last_session_id(id)?;
            Ok(session)
        } else {
            let from_last = {
                let last = self.last_session_id.lock().map_err(|e| {
                    anyhow::anyhow!("Failed to acquire last_session_id lock: {}", e)
                })?;

                last.as_ref().and_then(|id| {
                    SessionInfo::find_active_session_with_id(self.scope_id.clone(), id)
                        .ok()
                        .flatten()
                })
            };

            if let Some(session) = from_last {
                Ok(session)
            } else {
                let candidates: Vec<SessionInfo> =
                    SessionInfo::iter_active_sessions(self.scope_id.clone())?.collect();
                let session = resolve_unique_session(
                    candidates,
                    &self.scope_id,
                    Some("MCP tool calls also accept a session_id argument."),
                )?;
                self.set_last_session_id(&session.session_id)?;
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
                None => self
                    .last_session_id
                    .lock()
                    .map_err(|e| anyhow::anyhow!("Failed to acquire last_session_id lock: {}", e))?
                    .clone(),
            };
            if let Some(want) = want_id {
                // Pull the matching client + a snapshot of its session out from
                // under the lock, then probe liveness *after* releasing it — the
                // probe does syscalls (and on some platforms `is_process_alive`
                // spawns a subprocess), which should not be held under the lock.
                let candidate = {
                    let cache = self
                        .cached_client
                        .read()
                        .map_err(|e| anyhow::anyhow!("Failed to acquire cache lock: {}", e))?;
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

        let reused = {
            let cache = self
                .cached_client
                .read()
                .map_err(|e| anyhow::anyhow!("Failed to acquire cache lock: {}", e))?;
            cache
                .as_ref()
                .filter(|cached| cached.session.session_id == resolved_id)
                .map(|cached| Arc::clone(&cached.client))
        };

        let client = match reused {
            Some(client) => client,
            None => {
                let client = Arc::new(DapperControlPlaneClient::new(
                    session.control_plane_port,
                    self.scope_id.clone(),
                ));
                let mut cache = self
                    .cached_client
                    .write()
                    .map_err(|e| anyhow::anyhow!("Failed to acquire cache lock: {}", e))?;
                *cache = Some(CachedClient {
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
        self.get_client(session_id).map_err(|e| {
            CallToolResult::error(vec![Content::text(format!(
                "Error connecting to session: {:#}",
                e
            ))])
        })
    }

    #[tool(
        description = "Get the current debug session status, including execution state (stop reason, signal, crashed thread ID), active breakpoints, and session metadata. This is a lightweight command that does not interact with the debugger — use it as a first step to understand the session state before issuing other debug commands."
    )]
    async fn debug_status_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client.status().await {
            Ok(result) => {
                let config = DapperConfig {
                    context: dapper_config::ContextConfig::all_enabled(),
                    ..Default::default()
                };
                Ok(CallToolResult::success(vec![Content::text(
                    render_plaintext(&result, &config),
                )]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error getting status: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Execute the `threads` command in the debugger. The `threads` command lists all available threads in the debugged process. May also include the first thread's stack trace, along with the topmost frame's scopes and top-level local variable values."
    )]
    async fn debug_threads_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client.threads().await {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error getting threads: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Navigate debugger execution by stepping through code or continuing the program. Supports seven types of navigation: 'step_in' (step into functions), 'step_over' (step over functions - most commonly used), 'step_out' (step out of current frame), 'continue' (resume execution until breakpoint or exit), 'pause' (pause a running program), 'step_back' (step back one source line), and 'reverse_continue' (resume reverse execution until a breakpoint or the start of recording). The reverse navigation types ('step_back' and 'reverse_continue') require the connected adapter to advertise the DAP capability 'supportsStepBack'; otherwise the request is rejected without contacting the adapter."
    )]
    async fn debug_navigate_command(
        &self,
        request: Parameters<SessionTargeted<NavigateRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        let result = client
            .navigate(
                request.0.inner.navigation_type,
                request.0.inner.thread_id,
                request.0.inner.single_thread,
            )
            .await;

        match result {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error executing navigate {} command: {:#}",
                request.0.inner.navigation_type, e
            ))])),
        }
    }

    #[tool(
        description = "Execute the `stack-trace` command in the debugger. The `stack-trace` command prints the current stack trace showing the call hierarchy for the specified thread. May also include the topmost frame's scopes with top-level local variable values."
    )]
    async fn debug_stack_trace_command(
        &self,
        request: Parameters<SessionTargeted<StackTraceRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client
            .stack_trace(
                request.0.inner.thread_id,
                request.0.inner.start_frame,
                request.0.inner.levels,
            )
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error getting stack trace: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Execute the `scopes` command in the debugger. The `scopes` command lists all available scopes (like local variables, arguments, etc.) for the specified stack frame. May also include expanded top-level variable values for the 'Locals' scope."
    )]
    async fn debug_scopes_command(
        &self,
        request: Parameters<SessionTargeted<FrameIdRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client.scopes(request.0.inner.frame_id).await {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error getting scopes: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Execute the `variables` command in the debugger. The `variables` command lists all variables for the specified variables reference (obtained from scopes command or from other variables)."
    )]
    async fn debug_variables_command(
        &self,
        request: Parameters<SessionTargeted<VariablesReferenceRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client.variables(request.0.inner.variables_reference).await {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error getting variables: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Execute the `set-variable` command in the debugger. The `set-variable` command sets the value of the specified variable to the given value. If you are setting strings make sure to add additional single quotes to quote them."
    )]
    async fn debug_set_variable_command(
        &self,
        request: Parameters<SessionTargeted<SetVariableRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client
            .set_variable(
                request.0.inner.variables_reference,
                request.0.inner.name.as_str(),
                request.0.inner.value.as_str(),
            )
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error setting variable: {:#}",
                e
            ))])),
        }
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
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        let breakpoint_specs: Vec<SourceBreakpoint> = request
            .0
            .inner
            .breakpoints
            .into_iter()
            .map(SourceBreakpoint::from)
            .collect();
        match client
            .set_breakpoints(
                &request.0.inner.source_path,
                request.0.inner.clear_existing,
                &breakpoint_specs,
            )
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error setting breakpoint: {:#}",
                e
            ))])),
        }
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
        let SetExceptionBreakpointsRequest {
            filters,
            clear_existing,
        } = request.0.inner;

        // Strict empty-input validation (per design): empty + !clear is a
        // no-op, which is almost certainly a user/agent mistake. The
        // library layer (`ProxyClient::set_exception_breakpoints`) is
        // forgiving and silently no-ops; we reject here at the
        // user-facing surface so the LLM gets clear feedback.
        //
        // Intentionally validates *before* `get_client_or_error` so the
        // error message is the actionable one (about the missing filter)
        // rather than a confusing "no active session" — the caller can
        // fix the request shape regardless of session state.
        if filters.is_empty() && !clear_existing {
            return Ok(CallToolResult::error(vec![Content::text(
                "specify at least one filter or set clear_existing: true to disable all exception breakpoints"
                    .to_string(),
            )]));
        }

        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client
            .set_exception_breakpoints(&filters, clear_existing)
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                render_plaintext(&result, &self.config),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error setting exception breakpoints: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Execute the `evaluate` command in the debugger. The `evaluate` command evaluates an expression in the REPL context and returns the result. Depending on the debugger, the input can be a debugger command or a valid expression in the debugged language. Optionally, a frame_id can be provided to evaluate the expression in the context of a specific stack frame."
    )]
    async fn debug_evaluate_command(
        &self,
        request: Parameters<SessionTargeted<EvaluateRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client
            .eval_repl(&request.0.inner.expression, request.0.inner.frame_id)
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(result)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error evaluating expression: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Stop the dapper proxy server. This command aborts the proxy server task, shutting down the dapper infrastructure for this debug session."
    )]
    async fn debug_stop_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        // When stopping, the server shuts down immediately. Connection errors
        // are expected.
        let _ = client.stop().await;
        Ok(CallToolResult::success(vec![Content::text(
            "Dapper proxy server stopped.",
        )]))
    }

    #[tool(
        description = "List all active debug sessions. Returns session IDs, types (e.g. fb-lldb, fdb-debug/javadap), control ports, scope IDs, and other metadata. Call this to discover available sessions. When multiple sessions exist (e.g., dual-attach C++/Java debugging), pass the desired session_id to other debug commands to target a specific session."
    )]
    async fn debug_sessions_command(
        &self,
        _request: Parameters<EmptyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match SessionInfo::iter_active_sessions(self.scope_id.clone()) {
            Ok(sessions) => {
                let sessions: Vec<SessionInfo> = sessions.collect();
                if sessions.is_empty() {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "No active debug sessions found{}.",
                        self.scope_id
                            .as_ref()
                            .map_or(String::new(), |s| format!(" in scope '{}'", s))
                    ))]))
                } else {
                    let mut output = format!(
                        "Found {} active session(s){}:\n\n",
                        sessions.len(),
                        self.scope_id
                            .as_ref()
                            .map_or(String::new(), |s| format!(" in scope '{}'", s))
                    );
                    for session in &sessions {
                        output.push_str(&format!("{}\n", session));
                    }
                    Ok(CallToolResult::success(vec![Content::text(output)]))
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error listing sessions: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Get the debug session's launch/attach configuration and dapper settings. Returns the DAP launch/attach request arguments (debugger_args) and the active dapper configuration (output format, context settings, command defaults). This is a lightweight command that reads session metadata — it does not interact with the debugger."
    )]
    async fn debug_config_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.resolve_session(request.0.session_id.as_ref()) {
            Ok(session) => {
                let output = serde_json::json!({
                    "debugger_args": session.debugger_args,
                    "dapper_config": self.config,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
                )]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error resolving session: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Query the debug adapter's capabilities. Returns which optional DAP features are supported (e.g., stepBack, dataBreakpoints, functionBreakpoints). Call this before using advanced commands to check if the adapter supports them."
    )]
    async fn debug_capabilities_command(
        &self,
        request: Parameters<SessionTargeted<EmptyParams>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };
        match client.capabilities().await {
            Ok(Some(json)) => {
                let output = match serde_json::from_str::<serde_json::Value>(&json) {
                    Ok(value) => format_capabilities(&value),
                    Err(_) => json,
                };
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Ok(None) => Ok(CallToolResult::success(vec![Content::text(
                "Adapter capabilities not yet available (initialize response not received).",
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error querying capabilities: {:#}",
                e
            ))])),
        }
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
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        let arguments = match &request.0.inner.arguments {
            Some(serde_json::Value::String(s)) => {
                match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(parsed) => Some(parsed),
                    Err(_) => request.0.inner.arguments.clone(),
                }
            }
            other => other.clone(),
        };

        match client
            .send_dap_request(
                &request.0.inner.command,
                arguments,
                request.0.inner.wait_for_event,
                request.0.inner.timeout_seconds,
            )
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                result.render(),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "DAP request '{}' failed: {:#}",
                request.0.inner.command, e
            ))])),
        }
    }

    #[tool(
        description = "Read memory from the debugged process at a given address. Returns a formatted hex dump with addresses and ASCII sidebar. Use `debug_evaluate_command` to obtain memory references from expressions (e.g., `&myVariable`)."
    )]
    async fn debug_read_memory_command(
        &self,
        request: Parameters<SessionTargeted<ReadMemoryRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        let count = request.0.inner.count;
        if count <= 0 {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "count must be > 0, got {}",
                count
            ))]));
        }
        if count > MAX_READ_BYTES {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "count {} exceeds maximum of {} bytes",
                count, MAX_READ_BYTES
            ))]));
        }

        let mut args = serde_json::json!({
            "memoryReference": request.0.inner.memory_reference,
            "count": count,
        });
        if let Some(offset) = request.0.inner.offset {
            args["offset"] = serde_json::json!(offset);
        }

        match client
            .send_dap_request("readMemory", Some(args), false, default_timeout())
            .await
        {
            Ok(result) => match &result.body {
                ResponseBody::ReadMemory(Some(body)) => match format_memory_read(body) {
                    Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                        "Error decoding memory response: {}",
                        e
                    ))])),
                },
                ResponseBody::ReadMemory(None) => Ok(CallToolResult::success(vec![Content::text(
                    "No memory data returned by the debug adapter.",
                )])),
                _ => Ok(CallToolResult::success(vec![Content::text(
                    result.render(),
                )])),
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error reading memory: {:#}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Write memory to the debugged process at a given address. WARNING: This directly modifies process memory and can crash the debuggee if misused. The data parameter is a hex string (e.g., \"48656C6C6F\" writes the bytes for \"Hello\")."
    )]
    async fn debug_write_memory_command(
        &self,
        request: Parameters<SessionTargeted<WriteMemoryRequest>>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        // Convert user-provided hex string to base64 for DAP protocol
        let raw_bytes = match hex_string_to_bytes(&request.0.inner.data) {
            Ok(bytes) => bytes,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(e)]));
            }
        };
        let b64_data = base64::engine::general_purpose::STANDARD.encode(&raw_bytes);

        let mut args = serde_json::json!({
            "memoryReference": request.0.inner.memory_reference,
            "data": b64_data,
        });
        if let Some(offset) = request.0.inner.offset {
            args["offset"] = serde_json::json!(offset);
        }

        match client
            .send_dap_request("writeMemory", Some(args), false, default_timeout())
            .await
        {
            Ok(result) => match &result.body {
                ResponseBody::WriteMemory(Some(body)) => {
                    let written = body.bytes_written.unwrap_or(raw_bytes.len() as i64);
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "Successfully wrote {} byte(s) to {}.",
                        written, request.0.inner.memory_reference
                    ))]))
                }
                ResponseBody::WriteMemory(None) => {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "Write completed to {}.",
                        request.0.inner.memory_reference
                    ))]))
                }
                _ => Ok(CallToolResult::success(vec![Content::text(
                    result.render(),
                )])),
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error writing memory: {:#}",
                e
            ))])),
        }
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

        let client = match self.get_client_or_error(request.0.session_id.as_ref()) {
            Ok(c) => c,
            Err(e) => return Ok(e),
        };

        let threads_result = match client.threads().await {
            Ok(r) => r.result,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Error getting threads: {}",
                    e
                ))]));
            }
        };

        let req = &request.0.inner;
        let stack_depth = req.stack_depth.clamp(1, MAX_STACK_DEPTH);
        let max_threads = (req.max_threads.max(1) as usize).min(MAX_THREADS_HARD_CAP);

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

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response)
                .expect("serde_json::Value always serializes successfully"),
        )]))
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
        let cached = self.cached_client.read().unwrap_or_else(|e| {
            tracing::debug!("Failed to read cached_client for telemetry: {}", e);
            e.into_inner()
        });
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

fn format_capabilities(value: &serde_json::Value) -> String {
    let mut supported = Vec::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if val == &serde_json::Value::Bool(true) {
                supported.push(key.as_str());
            }
        }
    }
    let exception_filters_section: Option<String> = value
        .get("exceptionBreakpointFilters")
        .and_then(|v| v.as_array())
        .filter(|arr| !arr.is_empty())
        .map(|arr| format_exception_breakpoint_filters(arr.as_slice()));
    if supported.is_empty() && exception_filters_section.is_none() {
        return "No optional capabilities reported by the adapter.".to_string();
    }
    let mut output = String::new();
    if !supported.is_empty() {
        supported.sort();
        output.push_str("Supported capabilities:\n");
        for cap in &supported {
            let _ = writeln!(output, "  - {cap}");
        }
        output.push('\n');
    }
    if let Some(section) = exception_filters_section {
        output.push_str(&section);
        output.push('\n');
    }
    output.push_str("Capabilities not listed are unsupported by this adapter.");
    output
}

/// Render the `exceptionBreakpointFilters` array (advertised in the
/// `Capabilities` response) as a sorted list of filter ids with optional
/// label/default/supports_condition annotations. The bool-only walker
/// above silently drops this array, so it gets its own dedicated section.
fn format_exception_breakpoint_filters(filters: &[serde_json::Value]) -> String {
    let mut entries: Vec<&serde_json::Value> = filters.iter().collect();
    entries.sort_by(|a, b| {
        a.get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("filter").and_then(|v| v.as_str()).unwrap_or(""))
    });

    let mut output = String::from("Exception breakpoint filters:\n");
    for entry in entries {
        let filter = entry
            .get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        // Up to 3 annotations: label, default, supports_condition.
        let mut annotations: Vec<String> = Vec::with_capacity(3);
        // Debug-format the label so values containing whitespace or
        // punctuation render with visible quotes — useful for an LLM
        // agent reading the capability output.
        if let Some(label) = entry.get("label").and_then(|v| v.as_str()) {
            annotations.push(format!("label: {label:?}"));
        }
        if entry
            .get("default")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            annotations.push("default: true".to_string());
        }
        if entry
            .get("supportsCondition")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            annotations.push("supports_condition: true".to_string());
        }
        if annotations.is_empty() {
            let _ = writeln!(output, "  - {filter}");
        } else {
            let _ = writeln!(output, "  - {filter} ({})", annotations.join(", "));
        }
    }
    output
}

/// Format a ReadMemoryResponseBody as a hex dump with addresses and ASCII sidebar.
///
/// Returns `Err` only when the response payload exists but cannot be base64-decoded —
/// that's a protocol-level failure the caller should surface as a tool error rather
/// than a successful response. All other "no data" cases (None payload, unreadable
/// bytes) render as informational text inside `Ok`.
fn format_memory_read(body: &ReadMemoryResponseBody) -> Result<String, String> {
    let data = match &body.data {
        Some(b64) => base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("address {}: {}", body.address, e))?,
        None => {
            return Ok(match body.unreadable_bytes {
                Some(n) => format!("Address: {}\n{} byte(s) unreadable.", body.address, n),
                None => format!("Address: {}\nNo data returned.", body.address),
            });
        }
    };

    let base_addr = parse_address(&body.address);
    let mut output = format!("Memory at {} ({} bytes):\n", body.address, data.len());
    if let Some(n) = body.unreadable_bytes {
        let _ = writeln!(output, "({} byte(s) unreadable)", n);
    }

    for (i, chunk) in data.chunks(16).enumerate() {
        match base_addr {
            Some(base) => {
                let addr = base.wrapping_add((i * 16) as u64);
                let _ = write!(output, "0x{:016X}: ", addr);
            }
            // base address didn't parse — show row offsets so the column isn't a lie
            None => {
                let _ = write!(output, "+0x{:08X}:        ", i * 16);
            }
        }

        for (j, byte) in chunk.iter().enumerate() {
            if j == 8 {
                output.push(' ');
            }
            let _ = write!(output, "{:02X} ", byte);
        }
        // Pad short final row
        for j in chunk.len()..16 {
            if j == 8 {
                output.push(' ');
            }
            output.push_str("   ");
        }

        output.push(' ');
        for byte in chunk {
            output.push(if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            });
        }
        output.push('\n');
    }

    Ok(output)
}

/// Parse a DAP `memoryReference`-style address into a u64.
///
/// Per the DAP spec, the address is hex when prefixed with `0x`/`0X` and
/// decimal otherwise. Returns `None` if the input doesn't parse — callers
/// should fall back to relative offsets rather than render a misleading
/// absolute column.
fn parse_address(s: &str) -> Option<u64> {
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(rest, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

/// Convert a hex string (e.g., "48656C6C6F") into raw bytes.
fn hex_string_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex
        .strip_prefix("0x")
        .or_else(|| hex.strip_prefix("0X"))
        .unwrap_or(hex);
    if !hex.is_ascii() {
        return Err("hex string must contain only ASCII characters".to_string());
    }
    if !hex.len().is_multiple_of(2) {
        return Err("hex string must have an even number of digits".to_string());
    }
    let byte_len = hex.len() / 2;
    if byte_len > MAX_WRITE_BYTES {
        return Err(format!(
            "hex payload of {} bytes exceeds maximum of {} bytes",
            byte_len, MAX_WRITE_BYTES
        ));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| format!("invalid hex at position {}: {:?}", i, &hex[i..i + 2]))
        })
        .collect()
}

fn integer_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = i64::json_schema(generator);
    schema.remove("format");
    schema
}

/// Convert a schemars-generated `"type": ["T", "null"]` schema into
/// the `"anyOf": [{"type": "T"}, {"type": "null"}]` form that the
/// Claude API accepts (matching Pydantic v2 output).
fn type_array_to_any_of(schema: &mut Schema) {
    if let Some(type_val) = schema.get("type")
        && let Some(type_array) = type_val.as_array()
        && type_array.len() > 1
    {
        let any_of: Vec<serde_json::Value> = type_array
            .iter()
            .map(|t| serde_json::json!({"type": t}))
            .collect();
        schema.remove("type");
        schema.insert("anyOf".to_string(), serde_json::Value::Array(any_of));
    }
}

fn optional_integer_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = Option::<i64>::json_schema(generator);
    schema.remove("format");
    type_array_to_any_of(&mut schema);
    schema
}

fn optional_string_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = Option::<String>::json_schema(generator);
    type_array_to_any_of(&mut schema);
    schema
}

fn optional_boolean_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = Option::<bool>::json_schema(generator);
    type_array_to_any_of(&mut schema);
    schema
}

/// Accepts a JSON integer or a JSON string containing an integer.
/// LLM clients sometimes send integer parameters as strings.
fn deserialize_string_or_int<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    use serde::de::Error;

    let value = serde_json::Value::deserialize(deserializer)?;
    dapper_dap_protocol::data_types::i64_from_value(&value).map_err(D::Error::custom)
}

/// Accepts a JSON integer, a JSON string containing an integer, or null.
/// LLM clients sometimes send integer parameters as strings.
fn deserialize_optional_string_or_int<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    use serde::de::Error;

    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => dapper_dap_protocol::data_types::i64_from_value(&v)
            .map(Some)
            .map_err(D::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::from_value;
    use serde_json::json;
    use serde_json::to_value;

    use super::*;

    #[test]
    fn breakpoint_spec_from_object() {
        let spec: BreakpointSpec = from_value(json!({"line": 71})).unwrap();
        assert_eq!(spec.line, 71);
        assert_eq!(spec.condition, None);
        assert_eq!(spec.log_message, None);
    }

    #[test]
    fn breakpoint_spec_from_object_with_options() {
        let spec: BreakpointSpec = from_value(json!({
            "line": 10,
            "condition": "x > 5",
            "logMessage": "val is {x}"
        }))
        .unwrap();
        assert_eq!(spec.line, 10);
        assert_eq!(spec.condition, Some("x > 5".to_string()));
        assert_eq!(spec.log_message, Some("val is {x}".to_string()));
    }

    #[test]
    fn breakpoint_spec_from_stringified_json() {
        let spec: BreakpointSpec = from_value(json!("{\"line\": 71}")).unwrap();
        assert_eq!(spec.line, 71);
        assert_eq!(spec.condition, None);
    }

    #[test]
    fn breakpoint_spec_from_stringified_json_with_condition() {
        let spec: BreakpointSpec =
            from_value(json!("{\"line\": 10, \"condition\": \"x > 5\"}")).unwrap();
        assert_eq!(spec.line, 10);
        assert_eq!(spec.condition, Some("x > 5".to_string()));
    }

    #[test]
    fn breakpoint_spec_from_integer() {
        let spec: BreakpointSpec = from_value(json!(42)).unwrap();
        assert_eq!(spec.line, 42);
        assert_eq!(spec.condition, None);
    }

    #[test]
    fn breakpoint_spec_from_string_integer() {
        let spec: BreakpointSpec = from_value(json!("71")).unwrap();
        assert_eq!(spec.line, 71);
        assert_eq!(spec.condition, None);
    }

    #[test]
    fn breakpoint_spec_vec_mixed_formats() {
        let specs: Vec<BreakpointSpec> = from_value(json!([
            {"line": 10},
            "{\"line\": 20}",
            30,
            "40"
        ]))
        .unwrap();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].line, 10);
        assert_eq!(specs[1].line, 20);
        assert_eq!(specs[2].line, 30);
        assert_eq!(specs[3].line, 40);
    }

    #[test]
    fn breakpoint_spec_rejects_invalid_string() {
        let result: Result<BreakpointSpec, _> = from_value(json!("not_a_number"));
        assert!(result.is_err());
    }

    #[test]
    fn breakpoint_spec_schema_shape() {
        let schema =
            schemars::generate::SchemaGenerator::default().into_root_schema_for::<BreakpointSpec>();
        let json = to_value(&schema).unwrap();
        let props = json["properties"].as_object().unwrap();
        assert!(props.contains_key("line"));
        assert!(props.contains_key("condition"));
        assert!(props.contains_key("logMessage"));
        let required = json["required"].as_array().unwrap();
        assert!(required.contains(&json!("line")));
        assert!(!required.contains(&json!("condition")));
        assert!(!required.contains(&json!("logMessage")));
    }

    fn full_toolset_handler() -> McpHandler {
        let toolset = crate::toolsets::Toolset::from(crate::toolsets::BuiltinToolset::Full);
        McpHandler::new(None, None, &toolset)
    }

    /// The cache-first fast path in `get_client` must return the cached client
    /// without a sessions-directory scan when the requested session matches the
    /// cached one *and the cached session is still alive* — both for an implicit
    /// session (via `last_session_id`) and an explicit matching `session_id`. A
    /// dead cached session must fall through to the slow path instead of serving
    /// a stale client, so that fallback to another active session still works.
    #[test]
    fn get_client_fast_path_respects_cached_session_liveness() {
        use std::net::TcpListener;

        let handler = full_toolset_handler(); // control_port = None
        let sid = SessionId::from("fast-path-session");
        let client = Arc::new(DapperControlPlaneClient::new(None, None));

        // A live cached session: hold a listener so the port probe reports the
        // port as occupied, and use the current pid (via generate) so the
        // process looks alive -> is_active() == true.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = Port::try_new(listener.local_addr().unwrap().port()).unwrap();
        let live = SessionInfo::generate(sid.clone(), Some(port), None, None, None);
        assert!(
            live.is_active(),
            "precondition: seeded session must look active"
        );
        *handler.cached_client.write().unwrap() = Some(CachedClient {
            client: Arc::clone(&client),
            session: live,
        });
        *handler.last_session_id.lock().unwrap() = Some(sid.clone());

        // Implicit (via last_session_id) and explicit matching id both hit cache
        // and return the exact cached client without touching the filesystem.
        let got = handler
            .get_client(None)
            .expect("implicit live session should hit the cache");
        assert!(
            Arc::ptr_eq(&got, &client),
            "implicit session should return the cached client"
        );
        let got = handler
            .get_client(Some(&sid))
            .expect("explicit live session should hit the cache");
        assert!(
            Arc::ptr_eq(&got, &client),
            "explicit session should return the cached client"
        );

        // The fast path must not mutate last_session_id (only resolve_session does).
        assert_eq!(
            *handler.last_session_id.lock().unwrap(),
            Some(sid.clone()),
            "fast-path hit must leave last_session_id unchanged"
        );

        // A dead cached session (no reachable port -> not active): the fast path
        // must fall through. With an explicit id that resolves to no active
        // session, the slow path errors rather than serving the stale client.
        let dead = SessionInfo::generate(sid.clone(), None, None, None, None);
        assert!(
            !dead.is_active(),
            "precondition: seeded session must look dead"
        );
        *handler.cached_client.write().unwrap() = Some(CachedClient {
            client: Arc::clone(&client),
            session: dead,
        });
        assert!(
            handler.get_client(Some(&sid)).is_err(),
            "dead cached session must not be served from cache"
        );

        drop(listener);
    }

    /// When a fixed `control_port` is configured, `get_client` must NOT use the
    /// cache-first fast path (that path keys on session_id, but a port-configured
    /// handler resolves by port). It must fall through to `resolve_session`, which
    /// resolves by port and errors here since no session listens on the port.
    #[test]
    fn get_client_skips_fast_path_when_control_port_set() {
        use std::net::TcpListener;

        let toolset = crate::toolsets::Toolset::from(crate::toolsets::BuiltinToolset::Full);
        // Port 1: no dapper session listens here, so resolve-by-port fails.
        let handler = McpHandler::new(Some(Port::try_new(1).unwrap()), None, &toolset);
        let sid = SessionId::from("control-port-session");
        let client = Arc::new(DapperControlPlaneClient::new(None, None));

        // Seed a *live* matching cached session; if the fast path were taken it
        // would return this client. The control_port gate must prevent that.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = Port::try_new(listener.local_addr().unwrap().port()).unwrap();
        let live = SessionInfo::generate(sid.clone(), Some(port), None, None, None);
        assert!(
            live.is_active(),
            "precondition: seeded session must look active"
        );
        *handler.cached_client.write().unwrap() = Some(CachedClient {
            client: Arc::clone(&client),
            session: live,
        });
        *handler.last_session_id.lock().unwrap() = Some(sid.clone());

        // control_port is Some -> fast path skipped -> resolve_session by port -> Err.
        assert!(
            handler.get_client(None).is_err(),
            "control_port handler must bypass the fast path and resolve by port"
        );
        assert!(
            handler.get_client(Some(&sid)).is_err(),
            "control_port handler must bypass the fast path even with explicit id"
        );

        drop(listener);
    }

    #[test]
    fn all_tool_schemas_have_properties() {
        // Tools that take no parameters are allowed to have no properties
        let no_params_tools: &[&str] = &["debug_sessions_command"];
        let handler = full_toolset_handler();
        for (name, route) in &handler.tool_router.map {
            if no_params_tools.contains(&name.as_ref()) {
                continue;
            }
            let schema = &route.attr.input_schema;
            assert!(
                schema
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .is_some(),
                "tool '{}' schema has no 'properties' object",
                name
            );
        }
    }

    #[test]
    fn no_degenerate_schemas() {
        let handler = full_toolset_handler();
        for (name, route) in &handler.tool_router.map {
            let schema = Value::Object(route.attr.input_schema.as_ref().clone());
            check_schema_tree(&schema, name, "root");
        }
    }

    fn check_schema_tree(schema: &Value, tool_name: &str, path: &str) {
        assert!(
            !is_degenerate_schema(schema),
            "tool '{}' has degenerate schema at {}: {}",
            tool_name,
            path,
            schema
        );

        let Some(obj) = schema.as_object() else {
            return;
        };

        for key in &[
            "$defs",
            "properties",
            "patternProperties",
            "dependentSchemas",
        ] {
            if let Some(map) = obj.get(*key).and_then(|v| v.as_object()) {
                for (entry_name, entry_schema) in map {
                    check_schema_tree(
                        entry_schema,
                        tool_name,
                        &format!("{}/{}/{}", path, key, entry_name),
                    );
                }
            }
        }

        for key in &[
            "items",
            "not",
            "if",
            "then",
            "else",
            "additionalProperties",
            "propertyNames",
            "contains",
        ] {
            if let Some(sub) = obj.get(*key) {
                if sub.is_boolean() {
                    continue;
                }
                check_schema_tree(sub, tool_name, &format!("{}/{}", path, key));
            }
        }

        for key in &["oneOf", "anyOf", "allOf", "prefixItems"] {
            if let Some(items) = obj.get(*key).and_then(|v| v.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    check_schema_tree(item, tool_name, &format!("{}/{}[{}]", path, key, i));
                }
            }
        }
    }

    #[test]
    fn no_type_arrays_in_schemas() {
        // The Claude API does not accept "type": ["string", "null"] (the JSON Schema
        // 2020-12 array form for nullable types). It requires the equivalent "anyOf"
        // form instead. This test ensures no tool schema contains type arrays, which
        // would cause HTTP 500 errors from the Claude API.
        let handler = full_toolset_handler();
        for (name, route) in &handler.tool_router.map {
            let schema = Value::Object(route.attr.input_schema.as_ref().clone());
            assert_no_type_arrays(&schema, name, "root");
        }
    }

    fn assert_no_type_arrays(schema: &Value, tool_name: &str, path: &str) {
        let Some(obj) = schema.as_object() else {
            return;
        };

        if let Some(type_val) = obj.get("type") {
            assert!(
                !type_val.is_array(),
                "tool '{}' has array-valued 'type' at {}: {} — \
                 use anyOf instead for Claude API compatibility",
                tool_name,
                path,
                type_val
            );
        }

        // Recurse using the same subschema positions as check_schema_tree
        for key in &[
            "$defs",
            "properties",
            "patternProperties",
            "dependentSchemas",
        ] {
            if let Some(map) = obj.get(*key).and_then(|v| v.as_object()) {
                for (entry_name, entry_schema) in map {
                    assert_no_type_arrays(
                        entry_schema,
                        tool_name,
                        &format!("{}/{}/{}", path, key, entry_name),
                    );
                }
            }
        }

        for key in &["items", "not", "if", "then", "else", "additionalProperties"] {
            if let Some(sub) = obj.get(*key)
                && sub.is_object()
            {
                assert_no_type_arrays(sub, tool_name, &format!("{}/{}", path, key));
            }
        }

        for key in &["oneOf", "anyOf", "allOf", "prefixItems"] {
            if let Some(items) = obj.get(*key).and_then(|v| v.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    assert_no_type_arrays(item, tool_name, &format!("{}/{}[{}]", path, key, i));
                }
            }
        }
    }

    #[test]
    fn sessions_tool_always_available() {
        for builtin in &[
            crate::toolsets::BuiltinToolset::Minimal,
            crate::toolsets::BuiltinToolset::Standard,
            crate::toolsets::BuiltinToolset::Full,
            crate::toolsets::BuiltinToolset::Raw,
        ] {
            let toolset = crate::toolsets::Toolset::from(*builtin);
            let handler = McpHandler::new(None, None, &toolset);
            assert!(
                handler
                    .tool_router
                    .map
                    .contains_key("debug_sessions_command"),
                "debug_sessions_command should be available in {:?} toolset",
                builtin
            );
        }
    }

    #[test]
    fn sessions_tool_not_in_any_toolset_definition() {
        // Sessions is always-available, not part of any toolset definition
        for builtin in &[
            crate::toolsets::BuiltinToolset::Minimal,
            crate::toolsets::BuiltinToolset::Standard,
            crate::toolsets::BuiltinToolset::Full,
            crate::toolsets::BuiltinToolset::Raw,
        ] {
            let tools = builtin.tools();
            assert!(
                !tools.contains(&crate::toolsets::DebugTool::Sessions),
                "Sessions should not be in {:?} toolset definition",
                builtin
            );
        }
    }

    fn is_degenerate_schema(schema: &Value) -> bool {
        match schema {
            Value::Bool(b) => *b,
            Value::Object(obj) => {
                let meaningful_keys = [
                    "type",
                    "properties",
                    "patternProperties",
                    "additionalProperties",
                    "propertyNames",
                    "dependentSchemas",
                    "$ref",
                    "oneOf",
                    "anyOf",
                    "allOf",
                    "not",
                    "if",
                    "then",
                    "else",
                    "enum",
                    "const",
                    "items",
                    "prefixItems",
                    "contains",
                    "$defs",
                ];
                !meaningful_keys.iter().any(|k| obj.contains_key(*k))
            }
            _ => false,
        }
    }

    /// Send a `tools/call` MCP request through a full in-process MCP
    /// client/server pair (using `tokio::io::duplex`), returning the
    /// result or error exactly as a real MCP client would see it.
    async fn call_tool_e2e(
        tool_name: impl Into<String>,
        arguments: Value,
    ) -> Result<CallToolResult, rmcp::service::ServiceError> {
        let tool_name: String = tool_name.into();
        use rmcp::ClientHandler;
        use rmcp::ServiceExt;
        use rmcp::model::ClientInfo;

        #[derive(Debug, Clone, Default)]
        struct DummyClientHandler;
        impl ClientHandler for DummyClientHandler {
            fn get_info(&self) -> ClientInfo {
                ClientInfo::default()
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(4096);

        let handler = full_toolset_handler();
        let server_handle = tokio::spawn(async move {
            handler.serve(server_transport).await?.waiting().await?;
            anyhow::Ok(())
        });

        let client = DummyClientHandler.serve(client_transport).await.unwrap();

        let result = client
            .call_tool(
                CallToolRequestParams::new(tool_name).with_arguments(
                    arguments
                        .as_object()
                        .expect("test input must be a JSON object")
                        .clone(),
                ),
            )
            .await;

        client.cancel().await.ok();
        server_handle.abort();

        result
    }

    /// Assert that a `call_tool_e2e` result did not fail with a
    /// deserialization error (MCP error code -32602). The tool itself
    /// may return an application-level error (e.g. "no active session")
    /// which is fine — we only care that parameter parsing succeeded.
    fn assert_params_accepted(
        result: &Result<CallToolResult, rmcp::service::ServiceError>,
        context: &str,
    ) {
        match result {
            Err(rmcp::service::ServiceError::McpError(e)) if e.code.0 == -32602 => {
                panic!("{context}: parameter deserialization failed: {}", e.message);
            }
            _ => {} // Ok or any non-deserialization error is fine
        }
    }

    // -- stack_trace: ThreadId, Option<i64> levels, Option<i64> start_frame --

    #[tokio::test]
    async fn stack_trace_integer_values() {
        let result = call_tool_e2e(
            "debug_stack_trace_command",
            json!({"thread_id": 4743, "levels": 40}),
        )
        .await;
        assert_params_accepted(&result, "integer thread_id + integer levels");
    }

    #[tokio::test]
    async fn stack_trace_string_thread_id() {
        let result = call_tool_e2e("debug_stack_trace_command", json!({"thread_id": "4743"})).await;
        assert_params_accepted(&result, "string thread_id");
    }

    #[tokio::test]
    async fn stack_trace_string_levels() {
        let result = call_tool_e2e(
            "debug_stack_trace_command",
            json!({"thread_id": 4743, "levels": "40"}),
        )
        .await;
        assert_params_accepted(&result, "string levels");
    }

    #[tokio::test]
    async fn stack_trace_string_start_frame() {
        let result = call_tool_e2e(
            "debug_stack_trace_command",
            json!({"thread_id": 4743, "start_frame": "5"}),
        )
        .await;
        assert_params_accepted(&result, "string start_frame");
    }

    #[tokio::test]
    async fn stack_trace_all_strings() {
        let result = call_tool_e2e(
            "debug_stack_trace_command",
            json!({"thread_id": "4743", "levels": "40", "start_frame": "5"}),
        )
        .await;
        assert_params_accepted(&result, "all string values");
    }

    // -- navigate: ThreadId --

    #[tokio::test]
    async fn navigate_string_thread_id() {
        let result = call_tool_e2e(
            "debug_navigate_command",
            json!({"thread_id": "1", "navigation_type": "continue"}),
        )
        .await;
        assert_params_accepted(&result, "string thread_id in navigate");
    }

    #[tokio::test]
    async fn navigate_step_back() {
        let result = call_tool_e2e(
            "debug_navigate_command",
            json!({"thread_id": 1, "navigation_type": "step_back"}),
        )
        .await;
        assert_params_accepted(&result, "step_back navigation_type");
    }

    #[tokio::test]
    async fn navigate_reverse_continue() {
        let result = call_tool_e2e(
            "debug_navigate_command",
            json!({"thread_id": 1, "navigation_type": "reverse_continue"}),
        )
        .await;
        assert_params_accepted(&result, "reverse_continue navigation_type");
    }

    // -- scopes: FrameId --

    #[tokio::test]
    async fn scopes_string_frame_id() {
        let result = call_tool_e2e("debug_scopes_command", json!({"frame_id": "42"})).await;
        assert_params_accepted(&result, "string frame_id in scopes");
    }

    // -- variables: VariablesReference --

    #[tokio::test]
    async fn variables_string_variables_reference() {
        let result = call_tool_e2e(
            "debug_variables_command",
            json!({"variables_reference": "100"}),
        )
        .await;
        assert_params_accepted(&result, "string variables_reference");
    }

    // -- set_variable: VariablesReference --

    #[tokio::test]
    async fn set_variable_string_variables_reference() {
        let result = call_tool_e2e(
            "debug_set_variable_command",
            json!({"variables_reference": "100", "name": "x", "value": "42"}),
        )
        .await;
        assert_params_accepted(&result, "string variables_reference in set_variable");
    }

    // -- evaluate: Option<FrameId> --

    #[tokio::test]
    async fn evaluate_string_frame_id() {
        let result = call_tool_e2e(
            "debug_evaluate_command",
            json!({"expression": "1+1", "frame_id": "5"}),
        )
        .await;
        assert_params_accepted(&result, "string frame_id in evaluate");
    }

    #[tokio::test]
    async fn evaluate_integer_frame_id() {
        let result = call_tool_e2e(
            "debug_evaluate_command",
            json!({"expression": "1+1", "frame_id": 5}),
        )
        .await;
        assert_params_accepted(&result, "integer frame_id in evaluate");
    }

    #[tokio::test]
    async fn evaluate_no_frame_id() {
        let result = call_tool_e2e("debug_evaluate_command", json!({"expression": "1+1"})).await;
        assert_params_accepted(&result, "omitted frame_id in evaluate");
    }

    // -- set_breakpoints: BreakpointSpec.line --

    #[tokio::test]
    async fn breakpoints_string_line() {
        let result = call_tool_e2e(
            "debug_set_breakpoints_command",
            json!({"source_path": "/tmp/test.py", "breakpoints": [{"line": "10"}]}),
        )
        .await;
        assert_params_accepted(&result, "string line in breakpoint spec");
    }

    #[tokio::test]
    async fn breakpoints_integer_line() {
        let result = call_tool_e2e(
            "debug_set_breakpoints_command",
            json!({"source_path": "/tmp/test.py", "breakpoints": [{"line": 10}]}),
        )
        .await;
        assert_params_accepted(&result, "integer line in breakpoint spec");
    }

    // -- set_exception_breakpoints --

    #[tokio::test]
    async fn set_exception_breakpoints_filters_only() {
        let result = call_tool_e2e(
            "debug_set_exception_breakpoints_command",
            json!({"filters": ["raised"]}),
        )
        .await;
        assert_params_accepted(&result, "filters list with default clear_existing");
    }

    #[tokio::test]
    async fn set_exception_breakpoints_with_clear_existing() {
        let result = call_tool_e2e(
            "debug_set_exception_breakpoints_command",
            json!({"filters": ["raised", "uncaught"], "clear_existing": true}),
        )
        .await;
        assert_params_accepted(&result, "filters + clear_existing");
    }

    #[tokio::test]
    async fn set_exception_breakpoints_rejects_empty_filters_without_clear() {
        // Strict empty-input validation: empty filters + !clear is a
        // user/agent mistake, the tool must reject before reaching the
        // library no-op.
        let result = call_tool_e2e(
            "debug_set_exception_breakpoints_command",
            json!({"filters": [], "clear_existing": false}),
        )
        .await
        .expect("call should succeed at the MCP layer");
        assert_eq!(
            result.is_error,
            Some(true),
            "expected tool error for empty filters + !clear_existing"
        );
        let text = match result.content.first() {
            Some(c) => match &c.raw {
                rmcp::model::RawContent::Text(t) => t.text.as_str(),
                other => panic!("expected text content, got {other:?}"),
            },
            None => panic!("expected at least one content block"),
        };
        assert!(
            text.contains("specify at least one filter") && text.contains("clear_existing: true"),
            "expected explanatory error message; got: {text}"
        );
    }

    #[tokio::test]
    async fn set_exception_breakpoints_accepts_empty_filters_with_clear() {
        // Clear-all path: empty filters + clear_existing: true is the
        // documented way to disable all exception breakpoints. Should
        // pass parameter validation and reach the library.
        let result = call_tool_e2e(
            "debug_set_exception_breakpoints_command",
            json!({"filters": [], "clear_existing": true}),
        )
        .await;
        assert_params_accepted(&result, "empty filters + clear_existing: true (clear-all)");
    }

    // -- format_capabilities: exceptionBreakpointFilters extension --

    #[test]
    fn format_capabilities_renders_exception_filters() {
        let value = json!({
            "supportsStepBack": true,
            "exceptionBreakpointFilters": [
                {"filter": "uncaught", "label": "Uncaught", "default": true, "supportsCondition": true},
                {"filter": "raised", "label": "Raised"}
            ]
        });
        let rendered = format_capabilities(&value);
        // Bool capabilities come first.
        assert!(rendered.contains("Supported capabilities:\n  - supportsStepBack\n"));
        // Exception filters section follows, sorted by filter id.
        assert!(rendered.contains("Exception breakpoint filters:\n"));
        let raised_pos = rendered.find("- raised").expect("raised line missing");
        let uncaught_pos = rendered.find("- uncaught").expect("uncaught line missing");
        assert!(
            raised_pos < uncaught_pos,
            "filters should be sorted by id; got:\n{rendered}"
        );
        // Annotations are present where expected.
        assert!(
            rendered.contains(
                "- uncaught (label: \"Uncaught\", default: true, supports_condition: true)"
            ),
            "expected uncaught annotations: {rendered}"
        );
        assert!(
            rendered.contains("- raised (label: \"Raised\")"),
            "expected raised annotations: {rendered}"
        );
    }

    #[test]
    fn format_capabilities_omits_section_when_array_missing() {
        let value = json!({"supportsStepBack": true});
        let rendered = format_capabilities(&value);
        assert!(!rendered.contains("Exception breakpoint filters:"));
    }

    #[test]
    fn format_capabilities_omits_section_when_array_empty() {
        let value = json!({
            "supportsStepBack": true,
            "exceptionBreakpointFilters": []
        });
        let rendered = format_capabilities(&value);
        assert!(!rendered.contains("Exception breakpoint filters:"));
    }

    #[test]
    fn format_capabilities_only_exception_filters_no_supported_caps() {
        let value = json!({
            "exceptionBreakpointFilters": [{"filter": "raised"}]
        });
        let rendered = format_capabilities(&value);
        // No "Supported capabilities:" preamble when there are no bool caps.
        assert!(!rendered.contains("Supported capabilities:"));
        assert!(rendered.contains("Exception breakpoint filters:\n  - raised\n"));
    }

    // -- parse_address: hex with prefix, decimal without, junk --

    #[test]
    fn parse_address_hex_prefix() {
        assert_eq!(parse_address("0xDEADBEEF"), Some(0xDEAD_BEEF));
        assert_eq!(parse_address("0XdeadBEEF"), Some(0xDEAD_BEEF));
        assert_eq!(parse_address("0x0"), Some(0));
        assert_eq!(parse_address("0x7fff5fbff8a0"), Some(0x7fff5fbff8a0));
    }

    #[test]
    fn parse_address_decimal_no_prefix() {
        // Per DAP spec, no 0x/0X prefix means decimal — "4660" must NOT parse as 0x4660.
        assert_eq!(parse_address("4660"), Some(4660));
        assert_eq!(parse_address("0"), Some(0));
    }

    #[test]
    fn parse_address_invalid_returns_none() {
        assert_eq!(parse_address(""), None);
        assert_eq!(parse_address("0xZZ"), None);
        assert_eq!(parse_address("not a number"), None);
        assert_eq!(parse_address("12.34"), None);
    }

    // -- hex_string_to_bytes: prefixes, parity, ASCII, payload cap, parse failures --

    #[test]
    fn hex_string_to_bytes_happy() {
        assert_eq!(hex_string_to_bytes("48656C6C6F").unwrap(), b"Hello");
        assert_eq!(hex_string_to_bytes("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn hex_string_to_bytes_with_prefix() {
        assert_eq!(hex_string_to_bytes("0x4142").unwrap(), b"AB");
        assert_eq!(hex_string_to_bytes("0X4142").unwrap(), b"AB");
    }

    #[test]
    fn hex_string_to_bytes_rejects_odd_length() {
        let err = hex_string_to_bytes("123").unwrap_err();
        assert!(err.contains("even number"), "got: {err}");
    }

    #[test]
    fn hex_string_to_bytes_rejects_non_ascii() {
        let err = hex_string_to_bytes("é4").unwrap_err();
        assert!(err.contains("ASCII"), "got: {err}");
    }

    #[test]
    fn hex_string_to_bytes_rejects_bad_digit() {
        let err = hex_string_to_bytes("ZZZZ").unwrap_err();
        assert!(err.contains("invalid hex"), "got: {err}");
        assert!(err.contains("position 0"), "got: {err}");
    }

    #[test]
    fn hex_string_to_bytes_rejects_oversized_payload() {
        // 2 hex chars = 1 byte; produce MAX_WRITE_BYTES + 1 bytes worth.
        let oversize = "AA".repeat(MAX_WRITE_BYTES + 1);
        let err = hex_string_to_bytes(&oversize).unwrap_err();
        assert!(err.contains("exceeds maximum"), "got: {err}");
    }

    // -- format_memory_read: header, ASCII sidebar, multi-chunk, decode failure, no-data --

    fn make_body(
        address: &str,
        data: Option<&str>,
        unreadable_bytes: Option<i64>,
    ) -> ReadMemoryResponseBody {
        ReadMemoryResponseBody {
            address: address.to_string(),
            data: data.map(String::from),
            unreadable_bytes,
            ..Default::default()
        }
    }

    #[test]
    fn format_memory_read_header_and_sidebar() {
        // "Hello World" + 5 nulls = 16 bytes, base64 = "SGVsbG8gV29ybGQAAAAAAA=="
        let body = make_body("0x7fff5fbff8a0", Some("SGVsbG8gV29ybGQAAAAAAA=="), None);
        let out = format_memory_read(&body).unwrap();
        assert!(out.starts_with("Memory at 0x7fff5fbff8a0 (16 bytes):\n"));
        assert!(
            out.contains("Hello World....."),
            "ASCII sidebar mismatch:\n{out}"
        );
        assert!(out.contains("0x00007FFF5FBFF8A0:"));
    }

    #[test]
    fn format_memory_read_short_partial_row_padding() {
        // 5 bytes "Hello" → one short row, must still align ASCII sidebar.
        let body = make_body("0x10", Some("SGVsbG8="), None);
        let out = format_memory_read(&body).unwrap();
        assert!(out.contains("48 65 6C 6C 6F"));
        assert!(out.ends_with("Hello\n"));
    }

    #[test]
    fn format_memory_read_decode_failure_is_err() {
        // "%%%" is not valid base64.
        let body = make_body("0x10", Some("%%%"), None);
        let err = format_memory_read(&body).unwrap_err();
        assert!(
            err.contains("0x10"),
            "error should reference address: {err}"
        );
    }

    #[test]
    fn format_memory_read_no_data_with_unreadable_bytes() {
        let body = make_body("0x10", None, Some(8));
        let out = format_memory_read(&body).unwrap();
        assert_eq!(out, "Address: 0x10\n8 byte(s) unreadable.");
    }

    #[test]
    fn format_memory_read_no_data_no_unreadable() {
        let body = make_body("0x10", None, None);
        let out = format_memory_read(&body).unwrap();
        assert_eq!(out, "Address: 0x10\nNo data returned.");
    }

    #[test]
    fn format_memory_read_unparseable_address_uses_relative_offsets() {
        // Address that doesn't parse as hex or decimal — must not silently render as 0x0.
        let body = make_body("garbage", Some("SGVsbG8="), None);
        let out = format_memory_read(&body).unwrap();
        assert!(out.contains("Memory at garbage"));
        assert!(
            out.contains("+0x00000000:"),
            "should fall back to relative offsets:\n{out}"
        );
        assert!(!out.contains("0x0000000000000000:"));
    }

    // -- thread_snapshot: include_stacks, stack_depth, max_threads --

    #[tokio::test]
    async fn thread_snapshot_no_params() {
        let result = call_tool_e2e("debug_thread_snapshot", json!({})).await;
        assert_params_accepted(&result, "thread_snapshot with no params (all defaults)");
    }

    #[tokio::test]
    async fn thread_snapshot_integer_params() {
        let result = call_tool_e2e(
            "debug_thread_snapshot",
            json!({"include_stacks": true, "stack_depth": 20, "max_threads": 100}),
        )
        .await;
        assert_params_accepted(&result, "thread_snapshot with integer params");
    }

    #[tokio::test]
    async fn thread_snapshot_string_numeric_params() {
        let result = call_tool_e2e(
            "debug_thread_snapshot",
            json!({"stack_depth": "20", "max_threads": "100"}),
        )
        .await;
        assert_params_accepted(&result, "thread_snapshot with string-encoded integers");
    }

    #[test]
    fn thread_snapshot_request_defaults() {
        let req: ThreadSnapshotRequest = from_value(json!({})).unwrap();
        assert!(req.include_stacks, "include_stacks defaults to true");
        assert_eq!(req.stack_depth, 10, "stack_depth defaults to 10");
        assert_eq!(req.max_threads, 50, "max_threads defaults to 50");
    }

    #[test]
    fn thread_snapshot_request_explicit() {
        let req: ThreadSnapshotRequest =
            from_value(json!({"include_stacks": false, "stack_depth": 5, "max_threads": 10}))
                .unwrap();
        assert!(!req.include_stacks);
        assert_eq!(req.stack_depth, 5);
        assert_eq!(req.max_threads, 10);
    }

    /// Hard caps protect against pathological inputs even if the deserializer accepts them.
    #[test]
    fn thread_snapshot_clamping_constants() {
        // Sanity-check the constants used by debug_thread_snapshot: large requested
        // values are clamped to MAX_STACK_DEPTH / MAX_THREADS_HARD_CAP rather than
        // passed through to the DAP adapter.
        let huge_depth: i64 = 100_000;
        let clamped_depth = huge_depth.clamp(1, MAX_STACK_DEPTH);
        assert_eq!(
            clamped_depth, MAX_STACK_DEPTH,
            "stack_depth must clamp to MAX_STACK_DEPTH"
        );

        let huge_threads: i64 = 100_000;
        let clamped_threads = (huge_threads.max(1) as usize).min(MAX_THREADS_HARD_CAP);
        assert_eq!(
            clamped_threads, MAX_THREADS_HARD_CAP,
            "max_threads must clamp to MAX_THREADS_HARD_CAP"
        );

        // Negative / zero requests floor at 1, not 0 (avoids empty stack request).
        let zero_threads: i64 = 0;
        let clamped_zero = (zero_threads.max(1) as usize).min(MAX_THREADS_HARD_CAP);
        assert_eq!(clamped_zero, 1, "max_threads of 0 must floor at 1");
    }
}
