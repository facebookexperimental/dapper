// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::future::Future;
use std::result::Result;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use dapper_control_proto::CapabilitiesRequest;
use dapper_control_proto::CapabilitiesResponse;
use dapper_control_proto::EvalRequest;
use dapper_control_proto::EvalResponse;
use dapper_control_proto::NavigateRequest;
use dapper_control_proto::NavigateResponse;
use dapper_control_proto::NavigationType;
use dapper_control_proto::RawDapRequest;
use dapper_control_proto::RawDapResponse;
use dapper_control_proto::ScopesRequest;
use dapper_control_proto::ScopesResponse;
use dapper_control_proto::SetBreakpointsRequest;
use dapper_control_proto::SetBreakpointsResponse;
use dapper_control_proto::SetExceptionBreakpointsRequest;
use dapper_control_proto::SetExceptionBreakpointsResponse;
use dapper_control_proto::SetVariableRequest;
use dapper_control_proto::SetVariableResponse;
use dapper_control_proto::StackTraceRequest;
use dapper_control_proto::StackTraceResponse;
use dapper_control_proto::StatusRequest;
use dapper_control_proto::StatusResponse;
use dapper_control_proto::StopRequest;
use dapper_control_proto::StopResponse;
use dapper_control_proto::ThreadsRequest;
use dapper_control_proto::ThreadsResponse;
use dapper_control_proto::VariablesRequest;
use dapper_control_proto::VariablesResponse;
use dapper_control_proto::dapper_control_plane_client;
use dapper_control_proto::dapper_control_plane_server;
use dapper_control_proto::dapper_control_plane_server::DapperControlPlaneServer;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::SessionInfo;
use dapper_session::SessionStore;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::Request;
use tonic::Response;
use tonic::Status;
use tonic::transport::Channel;
use tonic::transport::Endpoint;
use tonic::transport::Server;

use crate::ControlPlaneResult;
use crate::protocol::DapperControlPlane;

pub struct ControlPlaneServer {
    pub handle: JoinHandle<anyhow::Result<()>>,
    pub port: Port,
}

/// Serve the control plane on port
pub async fn serve<T>(port: Option<Port>, control_plane: T) -> anyhow::Result<ControlPlaneServer>
where
    T: DapperControlPlane + 'static,
{
    // Try to bind to the control plane port early
    let port_number = port.map_or(0, |p| p.get());
    let addr = format!("127.0.0.1:{}", port_number);
    let bind_result = tokio::net::TcpListener::bind(&addr).await;

    let (listener, actual_port) = match bind_result {
        Ok(listener) => {
            let port = listener.local_addr()?.port();
            tracing::info!("Bound control plane port: {}", port);
            let actual_port =
                Port::try_new(port).ok_or(anyhow::anyhow!("bound port should be non-zero"))?;
            (listener, actual_port)
        }
        Err(e) => {
            tracing::warn!(
                "Failed to bind control plane to port {}: {}",
                port_number,
                e
            );
            return Err(anyhow::Error::from(e));
        }
    };

    let handle = tokio::spawn(async move {
        let handler = DapperControlPlaneHandler { control_plane };
        let incoming = TcpListenerStream::new(listener);
        Server::builder()
            .add_service(DapperControlPlaneServer::new(handler))
            .serve_with_incoming(incoming)
            .await?;

        Ok(())
    });

    Ok(ControlPlaneServer {
        handle,
        port: actual_port,
    })
}

struct DapperControlPlaneHandler<T>
where
    T: DapperControlPlane,
{
    control_plane: T,
}

#[async_trait::async_trait]
impl<T> dapper_control_plane_server::DapperControlPlane for DapperControlPlaneHandler<T>
where
    T: DapperControlPlane + Send + Sync + 'static,
{
    async fn eval_repl(
        &self,
        request: Request<EvalRequest>,
    ) -> Result<Response<EvalResponse>, Status> {
        to_tonic(async {
            let EvalRequest { command, frame_id } = request.into_inner();
            let response = self
                .control_plane
                .eval_repl(&command, frame_id.map(Into::into))
                .await?;
            Ok(EvalResponse { response })
        })
        .await
    }

    async fn stop(&self, _request: Request<StopRequest>) -> Result<Response<StopResponse>, Status> {
        to_tonic(async {
            self.control_plane.stop().await?;
            Ok(StopResponse {})
        })
        .await
    }

    async fn threads(
        &self,
        _request: Request<ThreadsRequest>,
    ) -> Result<Response<ThreadsResponse>, Status> {
        to_tonic(async {
            let cp_result = self.control_plane.threads().await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(ThreadsResponse {
                result_json,
                context_json,
            })
        })
        .await
    }

    async fn stack_trace(
        &self,
        request: Request<StackTraceRequest>,
    ) -> Result<Response<StackTraceResponse>, Status> {
        to_tonic(async {
            let StackTraceRequest {
                thread_id,
                start_frame,
                levels,
            } = request.into_inner();
            let cp_result = self
                .control_plane
                .stack_trace(thread_id.into(), start_frame, levels)
                .await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(StackTraceResponse {
                result_json,
                context_json,
            })
        })
        .await
    }

    async fn scopes(
        &self,
        request: Request<ScopesRequest>,
    ) -> Result<Response<ScopesResponse>, Status> {
        to_tonic(async {
            let ScopesRequest { frame_id } = request.into_inner();
            let cp_result = self.control_plane.scopes(frame_id.into()).await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(ScopesResponse {
                result_json,
                context_json,
            })
        })
        .await
    }

    async fn variables(
        &self,
        request: Request<VariablesRequest>,
    ) -> Result<Response<VariablesResponse>, Status> {
        to_tonic(async {
            let VariablesRequest {
                variables_reference,
            } = request.into_inner();
            let cp_result = self
                .control_plane
                .variables(variables_reference.into())
                .await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(VariablesResponse {
                result_json,
                context_json,
            })
        })
        .await
    }
    async fn navigate(
        &self,
        request: Request<NavigateRequest>,
    ) -> Result<Response<NavigateResponse>, Status> {
        to_tonic(async {
            let NavigateRequest {
                thread_id,
                navigation_type,
                single_thread,
            } = request.into_inner();
            let navigation_type_enum = match NavigationType::try_from(navigation_type) {
                Ok(proto_navigation_type) => navigation_type_from_proto(proto_navigation_type),
                Err(_) => return Err(anyhow::anyhow!("Invalid navigation type")),
            };
            let cp_result = self
                .control_plane
                .navigate(navigation_type_enum, thread_id.into(), single_thread)
                .await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(NavigateResponse {
                result_json,
                context_json,
            })
        })
        .await
    }

    async fn set_variable(
        &self,
        request: Request<SetVariableRequest>,
    ) -> Result<Response<SetVariableResponse>, Status> {
        to_tonic(async {
            let SetVariableRequest {
                variables_reference,
                name,
                value,
            } = request.into_inner();
            let cp_result = self
                .control_plane
                .set_variable(variables_reference.into(), &name, &value)
                .await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(SetVariableResponse {
                result_json,
                context_json,
            })
        })
        .await
    }

    async fn set_breakpoints(
        &self,
        request: Request<SetBreakpointsRequest>,
    ) -> Result<Response<SetBreakpointsResponse>, Status> {
        to_tonic(async {
            let SetBreakpointsRequest {
                source_path,
                lines,
                clear_existing,
                breakpoints,
            } = request.into_inner();
            // Prefer per-breakpoint specs from `breakpoints` field;
            // fall back to bare `lines` for backward compatibility.
            let breakpoint_specs: Vec<SourceBreakpoint> = if !breakpoints.is_empty() {
                breakpoints
                    .into_iter()
                    .map(|bp| SourceBreakpoint {
                        line: bp.line,
                        column: bp.column,
                        condition: bp.condition,
                        hit_condition: bp.hit_condition,
                        log_message: bp.log_message,
                        mode: bp.mode,
                    })
                    .collect()
            } else {
                lines
                    .into_iter()
                    .map(|line| SourceBreakpoint {
                        line,
                        ..Default::default()
                    })
                    .collect()
            };
            let cp_result = self
                .control_plane
                .set_breakpoints(&source_path, clear_existing, &breakpoint_specs)
                .await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(SetBreakpointsResponse {
                result_json,
                context_json,
            })
        })
        .await
    }

    async fn set_exception_breakpoints(
        &self,
        request: Request<SetExceptionBreakpointsRequest>,
    ) -> Result<Response<SetExceptionBreakpointsResponse>, Status> {
        to_tonic(async {
            let SetExceptionBreakpointsRequest {
                filters,
                clear_existing,
            } = request.into_inner();
            let cp_result = self
                .control_plane
                .set_exception_breakpoints(&filters, clear_existing)
                .await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(SetExceptionBreakpointsResponse {
                result_json,
                context_json,
            })
        })
        .await
    }

    async fn send_dap_request(
        &self,
        request: Request<RawDapRequest>,
    ) -> Result<Response<RawDapResponse>, Status> {
        to_tonic(async {
            let RawDapRequest {
                command,
                arguments_json,
                wait_for_event,
                timeout_seconds,
            } = request.into_inner();

            let arguments: Option<serde_json::Value> = if arguments_json.is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str(&arguments_json)
                        .map_err(|e| anyhow::anyhow!("Invalid JSON arguments: {}", e))?,
                )
            };

            match self
                .control_plane
                .send_dap_request(&command, arguments, wait_for_event, timeout_seconds)
                .await
            {
                Ok(raw_dap_result) => {
                    let response_json = serde_json::to_string(&raw_dap_result)
                        .map_err(|e| anyhow::anyhow!("Failed to serialize RawDapResult: {}", e))?;
                    Ok(RawDapResponse {
                        success: true,
                        response_json,
                        error_message: String::new(),
                    })
                }
                Err(e) => Ok(RawDapResponse {
                    success: false,
                    response_json: String::new(),
                    error_message: format!("{:#}", e),
                }),
            }
        })
        .await
    }

    async fn capabilities(
        &self,
        _request: Request<CapabilitiesRequest>,
    ) -> Result<Response<CapabilitiesResponse>, Status> {
        to_tonic(async {
            let capabilities_json = self.control_plane.capabilities().await?.unwrap_or_default();
            Ok(CapabilitiesResponse { capabilities_json })
        })
        .await
    }

    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        to_tonic(async {
            let cp_result = self.control_plane.status().await?;
            let (result_json, context_json) = cp_result.to_json_fields()?;
            Ok(StatusResponse {
                result_json,
                context_json,
            })
        })
        .await
    }
}

pub(crate) async fn to_tonic<F, T>(fut: F) -> Result<tonic::Response<T>, tonic::Status>
where
    F: Future<Output = anyhow::Result<T>>,
{
    match fut.await {
        Ok(r) => Ok(tonic::Response::new(r)),
        Err(e) => Err(tonic::Status::unknown(format!("{e:#}"))),
    }
}

struct CachedConnection {
    channel: Channel,
    port: u16,
}

/// Resolve a unique session from a list of candidates produced by auto-discovery.
///
/// On success, the returned `SessionInfo` has `control_plane_port == Some(_)`.
/// Errors when the list is empty (no active sessions), when the sole candidate
/// has no control port, or when more than one candidate remains (ambiguous —
/// same-scope multi-session like dual-attach). The caller is expected to
/// disambiguate explicitly via `--control-port` (deterministic) or a tighter
/// `--scope-id` filter.
///
/// `extra_hint`, when present and non-empty, is inserted into the ambiguity
/// message immediately before the candidate list so callers like the MCP
/// handler can mention call-site-specific disambiguation paths (e.g. per-call
/// `session_id`).
pub fn resolve_unique_session(
    sessions: Vec<SessionInfo>,
    scope_id: &Option<ScopeId>,
    extra_hint: Option<&str>,
) -> anyhow::Result<SessionInfo> {
    use std::fmt::Write as _;

    let scope_clause = scope_id
        .as_ref()
        .map_or(String::new(), |s| format!(" in scope '{}'", s));
    match sessions.len() {
        0 => Err(anyhow::anyhow!(
            "No active debug sessions found{}.",
            scope_clause
        )),
        1 => {
            let session = sessions.into_iter().next().expect("len == 1");
            if session.control_plane_port.is_none() {
                return Err(anyhow::anyhow!(
                    "Active debug session '{}'{} has no control port.",
                    session.session_id,
                    scope_clause
                ));
            }
            Ok(session)
        }
        n => {
            // Parent-preferring: a child session in the parent's scope must not
            // break zero-config auto-discovery, so target the unique root
            // (`parent_session_id == None`); children need an explicit
            // --control-port/session_id. Only "exactly one parentless session"
            // is checked, not that the others descend from it — fine in practice
            // (a child spawns into its parent's scope); fall through otherwise.
            if sessions
                .iter()
                .filter(|s| s.parent_session_id.is_none())
                .count()
                == 1
            {
                let root = sessions
                    .into_iter()
                    .find(|s| s.parent_session_id.is_none())
                    .expect("exactly one root session");
                if root.control_plane_port.is_none() {
                    return Err(anyhow::anyhow!(
                        "Root debug session '{}'{} has no control port.",
                        root.session_id,
                        scope_clause
                    ));
                }
                return Ok(root);
            }

            let scope_id_hint = if scope_id.is_none() {
                " (or --scope-id, if it narrows to one)"
            } else {
                ""
            };
            let extra = extra_hint
                .filter(|h| !h.is_empty())
                .map(|h| format!(" {h}"))
                .unwrap_or_default();
            let mut msg = format!(
                "Multiple active debug sessions found{scope_clause} ({n}); auto-discovery is ambiguous. \
                 Pass --control-port to target a specific session{scope_id_hint}.{extra} Active candidates:"
            );
            for session in &sessions {
                let port_str = session
                    .control_plane_port
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let scope_str = session
                    .scope_id
                    .as_ref()
                    .map_or(String::new(), |s| format!(" scope='{}'", s));
                // Writing into a String is infallible.
                let _ = write!(
                    msg,
                    "\n  - session '{}' port={}{}",
                    session.session_id, port_str, scope_str
                );
            }
            Err(anyhow::anyhow!(msg))
        }
    }
}

/// Proto <-> domain conversions for `NavigationType`. Free functions: both
/// types are foreign to this crate (proto from dapper_control_proto, domain
/// from dapper_session), so `From` impls would violate the orphan rule.
fn navigation_type_from_proto(
    proto: dapper_control_proto::NavigationType,
) -> dapper_session::NavigationType {
    use dapper_control_proto::NavigationType as P;
    use dapper_session::NavigationType as N;
    match proto {
        P::StepIn => N::StepIn,
        P::StepOver => N::StepOver,
        P::StepOut => N::StepOut,
        P::Continue => N::Continue,
        P::Pause => N::Pause,
        P::StepBack => N::StepBack,
        P::ReverseContinue => N::ReverseContinue,
    }
}

fn navigation_type_to_proto(
    navigation_type: dapper_session::NavigationType,
) -> dapper_control_proto::NavigationType {
    use dapper_control_proto::NavigationType as P;
    use dapper_session::NavigationType as N;
    match navigation_type {
        N::StepIn => P::StepIn,
        N::StepOver => P::StepOver,
        N::StepOut => P::StepOut,
        N::Continue => P::Continue,
        N::Pause => P::Pause,
        N::StepBack => P::StepBack,
        N::ReverseContinue => P::ReverseContinue,
    }
}

/// How the client locates the control plane: an exact port, or discovery of
/// the unique active session from a store, re-resolved on each call.
#[derive(Debug, Clone)]
enum SessionTarget {
    Port(Port),
    Discover {
        store: SessionStore,
        scope_id: Option<ScopeId>,
    },
}

pub struct DapperControlPlaneClient {
    target: SessionTarget,
    /// Cached gRPC channel for connection reuse. Invalidated when port changes.
    cached_connection: Arc<Mutex<Option<CachedConnection>>>,
}

impl DapperControlPlaneClient {
    /// A client for the control plane listening on a known port.
    pub fn for_port(port: Port) -> Self {
        Self::with_target(SessionTarget::Port(port))
    }

    /// A client that discovers the unique active session in `store`
    /// (optionally narrowed by `scope_id`) and targets its control plane.
    pub fn discover(store: SessionStore, scope_id: Option<ScopeId>) -> Self {
        Self::with_target(SessionTarget::Discover { store, scope_id })
    }

    fn with_target(target: SessionTarget) -> Self {
        Self {
            target,
            cached_connection: Arc::new(Mutex::new(None)),
        }
    }

    fn resolve_port(&self) -> anyhow::Result<(Port, Option<SessionId>)> {
        match &self.target {
            SessionTarget::Port(p) => Ok((*p, None)),
            SessionTarget::Discover { store, scope_id } => {
                let sessions: Vec<SessionInfo> =
                    store.iter_active_sessions(scope_id.clone()).collect();
                let session = resolve_unique_session(sessions, scope_id, None)?;
                let port = session
                    .control_plane_port
                    .expect("resolve_unique_session guarantees control_plane_port is Some");
                Ok((port, Some(session.session_id)))
            }
        }
    }

    fn lock_cache(&self) -> MutexGuard<'_, Option<CachedConnection>> {
        self.cached_connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn get_client(
        &self,
    ) -> anyhow::Result<dapper_control_plane_client::DapperControlPlaneClient<Channel>> {
        let (port, proxy_session_id) = self.resolve_port()?;

        // Reuse cached connection if port matches
        {
            let cache = self.lock_cache();
            if let Some(cached) = cache.as_ref()
                && cached.port == port.get()
            {
                return Ok(dapper_control_plane_client::DapperControlPlaneClient::new(
                    cached.channel.clone(),
                ));
            }
        }

        // Port changed or no cached connection, create new connection
        tracing::info!(
            port = port.get(),
            proxy_session_id = proxy_session_id.as_ref().map(|s| s.as_str()).unwrap_or(""),
            "Establishing gRPC connection to control plane"
        );
        let channel =
            Endpoint::try_from(format!("http://127.0.0.1:{}", port.get()))?.connect_lazy();

        {
            let mut cache = self.lock_cache();
            *cache = Some(CachedConnection {
                channel: channel.clone(),
                port: port.get(),
            });
        }

        Ok(dapper_control_plane_client::DapperControlPlaneClient::new(
            channel,
        ))
    }
}

#[async_trait::async_trait]
impl DapperControlPlane for DapperControlPlaneClient {
    async fn eval_repl(&self, command: &str, frame_id: Option<FrameId>) -> anyhow::Result<String> {
        let mut client = self.get_client().await?;
        let EvalResponse { response } = client
            .eval_repl(EvalRequest {
                command: command.to_owned(),
                frame_id: frame_id.map(Into::into),
            })
            .await?
            .into_inner();
        Ok(response)
    }

    async fn stop(&self) -> anyhow::Result<()> {
        let mut client = self.get_client().await?;
        client.stop(StopRequest {}).await?;
        Ok(())
    }

    async fn threads(&self) -> anyhow::Result<ControlPlaneResult<dapper_session::ThreadsResult>> {
        let mut client = self.get_client().await?;
        let resp = client.threads(ThreadsRequest {}).await?.into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn stack_trace(
        &self,
        thread_id: ThreadId,
        start_frame: Option<i64>,
        levels: Option<i64>,
    ) -> anyhow::Result<ControlPlaneResult<dapper_session::StackTraceResult>> {
        let mut client = self.get_client().await?;
        let resp = client
            .stack_trace(StackTraceRequest {
                thread_id: thread_id.into(),
                start_frame,
                levels,
            })
            .await?
            .into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn scopes(
        &self,
        frame_id: FrameId,
    ) -> anyhow::Result<ControlPlaneResult<dapper_session::ScopesResult>> {
        let mut client = self.get_client().await?;
        let resp = client
            .scopes(ScopesRequest {
                frame_id: frame_id.into(),
            })
            .await?
            .into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn variables(
        &self,
        variables_reference: VariablesReference,
    ) -> anyhow::Result<ControlPlaneResult<dapper_session::VariablesResult>> {
        let mut client = self.get_client().await?;
        let resp = client
            .variables(VariablesRequest {
                variables_reference: variables_reference.into(),
            })
            .await?
            .into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn navigate(
        &self,
        navigation_type: dapper_session::NavigationType,
        thread_id: ThreadId,
        single_thread: Option<bool>,
    ) -> anyhow::Result<ControlPlaneResult<dapper_session::NavigationResult>> {
        let mut client = self.get_client().await?;
        let proto_navigation_type = navigation_type_to_proto(navigation_type);

        let resp = client
            .navigate(NavigateRequest {
                thread_id: thread_id.into(),
                navigation_type: proto_navigation_type as i32,
                single_thread,
            })
            .await?
            .into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn set_variable(
        &self,
        variables_reference: VariablesReference,
        name: &str,
        value: &str,
    ) -> anyhow::Result<ControlPlaneResult<dapper_session::SetVariableResult>> {
        let mut client = self.get_client().await?;
        let resp = client
            .set_variable(SetVariableRequest {
                variables_reference: variables_reference.into(),
                name: name.to_owned(),
                value: value.to_owned(),
            })
            .await?
            .into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn set_breakpoints(
        &self,
        source_path: &str,
        clear_existing: bool,
        breakpoint_specs: &[SourceBreakpoint],
    ) -> anyhow::Result<ControlPlaneResult<dapper_session::SetBreakpointsResult>> {
        let mut client = self.get_client().await?;
        let breakpoints: Vec<dapper_control_proto::SourceBreakpoint> = breakpoint_specs
            .iter()
            .map(|bp| dapper_control_proto::SourceBreakpoint {
                line: bp.line,
                condition: bp.condition.clone(),
                hit_condition: bp.hit_condition.clone(),
                log_message: bp.log_message.clone(),
                column: bp.column,
                mode: bp.mode.clone(),
            })
            .collect();
        let lines: Vec<i64> = breakpoint_specs.iter().map(|bp| bp.line).collect();
        let resp = client
            .set_breakpoints(SetBreakpointsRequest {
                source_path: source_path.to_owned(),
                lines,
                clear_existing,
                breakpoints,
            })
            .await?
            .into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn set_exception_breakpoints(
        &self,
        filters: &[String],
        clear_existing: bool,
    ) -> anyhow::Result<ControlPlaneResult<dapper_session::SetExceptionBreakpointsResult>> {
        let mut client = self.get_client().await?;
        let resp = client
            .set_exception_breakpoints(SetExceptionBreakpointsRequest {
                filters: filters.to_vec(),
                clear_existing,
            })
            .await?
            .into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }

    async fn send_dap_request(
        &self,
        command: &str,
        arguments: Option<serde_json::Value>,
        wait_for_event: bool,
        timeout_seconds: u64,
    ) -> anyhow::Result<dapper_session::RawDapResult> {
        let mut client = self.get_client().await?;

        let arguments_json = match arguments {
            Some(v) => v.to_string(),
            None => String::new(),
        };

        let RawDapResponse {
            success,
            response_json,
            error_message,
        } = client
            .send_dap_request(RawDapRequest {
                command: command.to_owned(),
                arguments_json,
                wait_for_event,
                timeout_seconds,
            })
            .await?
            .into_inner();

        if success {
            serde_json::from_str(&response_json)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize RawDapResult: {}", e))
        } else {
            Err(anyhow::anyhow!(error_message))
        }
    }

    async fn capabilities(&self) -> anyhow::Result<Option<String>> {
        let mut client = self.get_client().await?;
        let CapabilitiesResponse { capabilities_json } = client
            .capabilities(CapabilitiesRequest {})
            .await?
            .into_inner();
        if capabilities_json.is_empty() {
            Ok(None)
        } else {
            Ok(Some(capabilities_json))
        }
    }

    async fn status(&self) -> anyhow::Result<ControlPlaneResult<dapper_session::StatusResult>> {
        let mut client = self.get_client().await?;
        let resp = client.status(StatusRequest {}).await?.into_inner();
        ControlPlaneResult::from_proto_fields(resp.result_json, resp.context_json)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use dapper_dap_protocol::data_types::Scope;
    use dapper_dap_protocol::data_types::StackFrame;
    use dapper_dap_protocol::data_types::Thread;
    use dapper_dap_protocol::data_types::Variable;
    use dapper_dap_protocol::enums::ScopePresentationHint;
    use dapper_dap_protocol::responses::ThreadsResponseBody;
    use dapper_session::SessionInfo;

    use super::*;

    #[test]
    fn navigation_type_proto_round_trip_preserves_variants() {
        use dapper_session::NavigationType as N;
        for navigation_type in [
            N::StepIn,
            N::StepOver,
            N::StepOut,
            N::Continue,
            N::Pause,
            N::StepBack,
            N::ReverseContinue,
        ] {
            assert_eq!(
                navigation_type_from_proto(navigation_type_to_proto(navigation_type)),
                navigation_type,
                "proto round trip should preserve the variant"
            );
        }
    }

    struct TestServer {
        stop_was_called: Arc<AtomicBool>,
        breakpoint_specs: Arc<Mutex<Vec<SourceBreakpoint>>>,
    }
    impl TestServer {
        fn new(stop_was_called: Arc<AtomicBool>) -> Self {
            TestServer {
                stop_was_called,
                breakpoint_specs: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }
    #[async_trait::async_trait]
    impl DapperControlPlane for TestServer {
        async fn eval_repl(
            &self,
            command: &str,
            frame_id: Option<FrameId>,
        ) -> anyhow::Result<String> {
            match frame_id {
                Some(fid) => Ok(format!("eval result for: {command} (frame {fid})")),
                None => Ok(format!("eval result for: {command}")),
            }
        }

        async fn stop(&self) -> anyhow::Result<()> {
            self.stop_was_called.store(true, Ordering::Relaxed);
            Ok(())
        }

        async fn threads(
            &self,
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::ThreadsResult>> {
            Ok(ControlPlaneResult {
                result: dapper_session::ThreadsResult {
                    threads: vec![Thread {
                        id: 1.into(),
                        name: "MainThread".to_string(),
                    }],
                    ..Default::default()
                },
                context: Some(dapper_session::ResponseContext {
                    session: Some(SessionInfo {
                        session_id: "test-session".into(),
                        session_type: Some("debugpy".to_string()),
                        pid: 0,
                        control_plane_port: None,
                        started_at: 0,
                        command_line_args: vec![],
                        current_working_directory: None,
                        scope_id: None,
                        request_type: None,
                        program_path: None,
                        debuggee_process_id: None,
                        debugger_args: None,
                        parent_session_id: None,
                    }),
                    ..Default::default()
                }),
            })
        }

        async fn stack_trace(
            &self,
            thread_id: ThreadId,
            _start_frame: Option<i64>,
            _levels: Option<i64>,
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::StackTraceResult>> {
            Ok(ControlPlaneResult {
                result: dapper_session::StackTraceResult {
                    stack_frames: vec![StackFrame {
                        id: 10.into(),
                        name: "main_entry".to_string(),
                        ..Default::default()
                    }],
                    thread_id,
                    ..Default::default()
                },
                context: None,
            })
        }

        async fn scopes(
            &self,
            frame_id: FrameId,
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::ScopesResult>> {
            Ok(ControlPlaneResult {
                result: dapper_session::ScopesResult {
                    scopes: vec![Scope {
                        name: "Locals".to_string(),
                        presentation_hint: Some(ScopePresentationHint::Locals),
                        variables_reference: 10.into(),
                        ..Default::default()
                    }],
                    frame_id,
                    ..Default::default()
                },
                context: None,
            })
        }

        async fn variables(
            &self,
            variables_reference: VariablesReference,
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::VariablesResult>> {
            Ok(ControlPlaneResult {
                result: dapper_session::VariablesResult {
                    variables: vec![Variable {
                        name: "x".to_string(),
                        value: "42".to_string(),
                        ..Default::default()
                    }],
                    variables_reference,
                    ..Default::default()
                },
                context: None,
            })
        }

        async fn navigate(
            &self,
            navigation_type: dapper_session::NavigationType,
            _thread_id: ThreadId,
            _single_thread: Option<bool>,
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::NavigationResult>> {
            Ok(ControlPlaneResult {
                result: dapper_session::NavigationResult {
                    result: dapper_session::NavigateResult::CommandExecuted,
                    navigation_type,
                    extra: Default::default(),
                },
                context: None,
            })
        }

        async fn set_variable(
            &self,
            _variables_reference: VariablesReference,
            name: &str,
            _value: &str,
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::SetVariableResult>> {
            Ok(ControlPlaneResult {
                result: dapper_session::SetVariableResult {
                    name: name.to_string(),
                    ..Default::default()
                },
                context: None,
            })
        }

        async fn set_breakpoints(
            &self,
            source_path: &str,
            clear_existing: bool,
            breakpoint_specs: &[SourceBreakpoint],
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::SetBreakpointsResult>> {
            *self
                .breakpoint_specs
                .lock()
                .expect("breakpoint recorder mutex should not be poisoned") =
                breakpoint_specs.to_vec();
            let breakpoints = breakpoint_specs
                .iter()
                .map(|bp| dapper_session::BreakpointInfo {
                    line: bp.line,
                    verified: true,
                    ..Default::default()
                })
                .collect::<Vec<_>>();
            let new_count = breakpoints.len();
            let existing_count = if clear_existing { 0 } else { 1 };
            Ok(ControlPlaneResult {
                result: dapper_session::SetBreakpointsResult {
                    source_path: source_path.to_string(),
                    breakpoints,
                    new_count,
                    existing_count,
                    ..Default::default()
                },
                context: None,
            })
        }

        async fn set_exception_breakpoints(
            &self,
            filters: &[String],
            _clear_existing: bool,
        ) -> anyhow::Result<ControlPlaneResult<dapper_session::SetExceptionBreakpointsResult>>
        {
            // Stateless mock: pretend nothing was previously installed.
            // Every requested filter is "new"; nothing was "existing". The
            // gRPC end-to-end test asserts these values against the
            // request shape, not against any stateful merge logic, and
            // intentionally doesn't model intra-request dedup either —
            // that contract is unit-tested in
            // `dapper_proxy_server::client::tests::merge_exception_filters_*`.
            let installed = filters
                .iter()
                .map(|f| dapper_session::ExceptionFilterEntry {
                    filter: f.clone(),
                    condition: None,
                })
                .collect::<Vec<_>>();
            let new_count = filters.len();
            let existing_count = 0;
            Ok(ControlPlaneResult {
                result: dapper_session::SetExceptionBreakpointsResult {
                    installed,
                    new_count,
                    existing_count,
                    ..Default::default()
                },
                context: None,
            })
        }

        async fn send_dap_request(
            &self,
            command: &str,
            _arguments: Option<serde_json::Value>,
            _wait_for_event: bool,
            _timeout_seconds: u64,
        ) -> anyhow::Result<dapper_session::RawDapResult> {
            let body = match command {
                "pause" => dapper_dap_protocol::responses::ResponseBody::Pause,
                "threads" => {
                    dapper_dap_protocol::responses::ResponseBody::Threads(ThreadsResponseBody {
                        threads: vec![Thread {
                            id: 1.into(),
                            name: "main".to_string(),
                        }],
                        ..Default::default()
                    })
                }
                _ => dapper_dap_protocol::responses::ResponseBody::Unknown(
                    dapper_dap_protocol::responses::UnknownResponseBody {
                        command: command.to_owned(),
                        body: None,
                        extra: Default::default(),
                    },
                ),
            };

            Ok(dapper_session::RawDapResult {
                body,
                event: None,
                extra: Default::default(),
            })
        }

        async fn capabilities(&self) -> anyhow::Result<Option<String>> {
            Ok(Some(
                r#"{"supportsStepBack":true,"supportsSetVariable":true}"#.to_string(),
            ))
        }

        async fn status(&self) -> anyhow::Result<ControlPlaneResult<dapper_session::StatusResult>> {
            Ok(ControlPlaneResult {
                result: dapper_session::StatusResult::default(),
                context: None,
            })
        }
    }

    async fn round_trip_breakpoint_specs(
        breakpoint_specs: &[SourceBreakpoint],
    ) -> anyhow::Result<Vec<SourceBreakpoint>> {
        let server = TestServer::new(Arc::new(AtomicBool::new(false)));
        let recorded_specs = Arc::clone(&server.breakpoint_specs);
        let control_plane_server = serve(None, server).await?;
        tokio::task::yield_now().await;

        let client = DapperControlPlaneClient::for_port(control_plane_server.port);
        let result = client
            .set_breakpoints("/path/to/file.py", true, breakpoint_specs)
            .await;
        control_plane_server.handle.abort();
        result?;

        let specs = recorded_specs
            .lock()
            .expect("breakpoint recorder mutex should not be poisoned")
            .clone();
        Ok(specs)
    }

    #[tokio::test]
    async fn set_breakpoints_grpc_preserves_source_breakpoint_metadata() -> anyhow::Result<()> {
        let expected = SourceBreakpoint {
            line: 42,
            column: Some(7),
            condition: Some("x > 0".into()),
            hit_condition: Some("3".into()),
            log_message: Some("hit {x}".into()),
            mode: Some("hardware".into()),
        };

        let recorded = round_trip_breakpoint_specs(std::slice::from_ref(&expected)).await?;
        assert_eq!(recorded, vec![expected]);
        Ok(())
    }

    #[tokio::test]
    async fn set_breakpoints_grpc_preserves_present_empty_strings() -> anyhow::Result<()> {
        let expected = SourceBreakpoint {
            line: 42,
            condition: Some(String::new()),
            hit_condition: Some(String::new()),
            log_message: Some(String::new()),
            mode: Some(String::new()),
            ..Default::default()
        };

        let recorded = round_trip_breakpoint_specs(std::slice::from_ref(&expected)).await?;
        assert_eq!(recorded, vec![expected]);
        Ok(())
    }

    #[tokio::test]
    async fn client_calls_server() -> anyhow::Result<()> {
        let stop_was_called = Arc::new(AtomicBool::new(false));

        let stop_was_called_for_server = Arc::clone(&stop_was_called);
        let server = TestServer::new(stop_was_called_for_server);
        let control_plane_server = serve(None, server).await?;

        // give the server task a chance to run
        tokio::task::yield_now().await;

        let client = DapperControlPlaneClient::for_port(control_plane_server.port);

        client.stop().await?;
        assert!(stop_was_called.load(Ordering::Relaxed));

        let eval_result = client.eval_repl("step", None).await?;
        assert_eq!(eval_result, "eval result for: step");

        let eval_with_frame_result = client.eval_repl("step", Some(2.into())).await?;
        assert_eq!(eval_with_frame_result, "eval result for: step (frame 2)");

        let (threads, threads_ctx) = client.threads().await?.into_parts();
        assert_eq!(threads.threads.len(), 1);
        assert_eq!(threads.threads[0].name, "MainThread");
        let ctx = threads_ctx.expect("expected context");
        let session = ctx.session.expect("expected session");
        assert_eq!(session.session_id.as_str(), "test-session");
        assert_eq!(session.session_type.as_deref(), Some("debugpy"));

        let (stack_trace, _) = client.stack_trace(1.into(), None, None).await?.into_parts();
        assert_eq!(stack_trace.stack_frames.len(), 1);
        assert_eq!(stack_trace.stack_frames[0].name, "main_entry");
        assert_eq!(stack_trace.thread_id, 1.into());

        let (scopes, _) = client.scopes(2.into()).await?.into_parts();
        assert_eq!(scopes.scopes.len(), 1);
        assert_eq!(scopes.scopes[0].name, "Locals");
        assert_eq!(scopes.frame_id, 2.into());

        let (variables, _) = client.variables(7.into()).await?.into_parts();
        assert_eq!(variables.variables.len(), 1);
        assert_eq!(variables.variables[0].name, "x");
        assert_eq!(variables.variables[0].value, "42");
        assert_eq!(variables.variables_reference, 7.into());

        let (navigate, _) = client
            .navigate(dapper_session::NavigationType::StepOver, 1.into(), None)
            .await?
            .into_parts();
        assert_eq!(
            navigate.navigation_type,
            dapper_session::NavigationType::StepOver
        );
        assert!(matches!(
            navigate.result,
            dapper_session::NavigateResult::CommandExecuted
        ));

        let (set_var, _) = client
            .set_variable(7.into(), "x", "100")
            .await?
            .into_parts();
        assert_eq!(set_var.name, "x");

        let (bp_append, _) = client
            .set_breakpoints(
                "/path/to/file.py",
                false,
                &[SourceBreakpoint {
                    line: 42,
                    ..Default::default()
                }],
            )
            .await?
            .into_parts();
        assert_eq!(bp_append.source_path, "/path/to/file.py");
        assert_eq!(bp_append.breakpoints.len(), 1);
        assert_eq!(bp_append.breakpoints[0].line, 42);
        assert_eq!(bp_append.new_count, 1);
        assert_eq!(bp_append.existing_count, 1);

        let (bp_clear, _) = client
            .set_breakpoints(
                "/path/to/file.py",
                true,
                &[
                    SourceBreakpoint {
                        line: 10,
                        ..Default::default()
                    },
                    SourceBreakpoint {
                        line: 20,
                        ..Default::default()
                    },
                ],
            )
            .await?
            .into_parts();
        assert_eq!(bp_clear.source_path, "/path/to/file.py");
        assert_eq!(bp_clear.breakpoints.len(), 2);
        assert_eq!(bp_clear.new_count, 2);
        assert_eq!(bp_clear.existing_count, 0);

        // The stateless TestServer mock pretends nothing was previously
        // installed, so both calls report the requested filters as new.
        // The point of this test is to exercise the proto/serialization
        // round-trip; the real merge accounting is unit-tested separately
        // in `dapper_proxy_server::client::tests`.
        let (exc_append, _) = client
            .set_exception_breakpoints(&["uncaught".to_string()], false)
            .await?
            .into_parts();
        assert_eq!(exc_append.installed.len(), 1);
        assert_eq!(exc_append.installed[0].filter, "uncaught");
        assert_eq!(exc_append.new_count, 1);
        assert_eq!(exc_append.existing_count, 0);

        let (exc_clear, _) = client
            .set_exception_breakpoints(&["raised".to_string()], true)
            .await?
            .into_parts();
        assert_eq!(exc_clear.installed.len(), 1);
        assert_eq!(exc_clear.installed[0].filter, "raised");
        assert_eq!(exc_clear.new_count, 1);
        assert_eq!(exc_clear.existing_count, 0);

        let dap_request_result = client
            .send_dap_request("pause", Some(serde_json::json!({"threadId": 1})), false, 60)
            .await?;
        assert!(matches!(
            dap_request_result.body,
            dapper_dap_protocol::responses::ResponseBody::Pause
        ));
        assert!(dap_request_result.event.is_none());

        let dap_request_no_args = client.send_dap_request("threads", None, false, 60).await?;
        assert!(matches!(
            dap_request_no_args.body,
            dapper_dap_protocol::responses::ResponseBody::Threads(_)
        ));
        assert!(dap_request_no_args.event.is_none());

        let caps = client.capabilities().await?;
        assert_eq!(
            caps,
            Some(r#"{"supportsStepBack":true,"supportsSetVariable":true}"#.to_string())
        );

        Ok(())
    }

    fn fake_session(id: &str, port: Option<u16>, scope: Option<&str>) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            pid: 0,
            control_plane_port: port.and_then(Port::try_new),
            started_at: 0,
            command_line_args: vec![],
            current_working_directory: None,
            scope_id: scope.map(Into::into),
            request_type: None,
            session_type: None,
            program_path: None,
            debuggee_process_id: None,
            debugger_args: None,
            parent_session_id: None,
        }
    }

    #[test]
    fn lock_cache_recovers_from_poison() {
        let client = DapperControlPlaneClient::for_port(Port::try_new(4321).unwrap());
        // Poison the mutex: panic while holding the guard on another thread.
        let cached = client.cached_connection.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = cached.lock().unwrap();
                panic!("poison the mutex");
            })
            .join()
            .is_err()
        );
        // The helper must recover the poisoned lock, not propagate the error.
        assert!(client.lock_cache().is_none());
    }

    #[test]
    fn resolve_unique_session_empty() {
        let err = resolve_unique_session(vec![], &None, None).unwrap_err();
        assert!(
            err.to_string().contains("No active debug sessions"),
            "got: {err}"
        );
    }

    #[test]
    fn resolve_unique_session_empty_with_scope() {
        let err = resolve_unique_session(vec![], &Some("vscode-1".into()), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("No active debug sessions"), "got: {msg}");
        assert!(msg.contains("scope 'vscode-1'"), "got: {msg}");
    }

    #[test]
    fn resolve_unique_session_single() {
        let sessions = vec![fake_session("only", Some(12345), None)];
        let session = resolve_unique_session(sessions, &None, None).unwrap();
        assert_eq!(session.control_plane_port.unwrap().get(), 12345);
        assert_eq!(session.session_id, "only".into());
    }

    #[test]
    fn resolve_unique_session_ambiguous_no_scope() {
        let sessions = vec![
            fake_session("a", Some(11111), None),
            fake_session("b", Some(22222), None),
        ];
        let err = resolve_unique_session(sessions, &None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Multiple active debug sessions"), "got: {msg}");
        assert!(msg.contains("--control-port"), "got: {msg}");
        assert!(msg.contains("--scope-id"), "got: {msg}");
        assert!(msg.contains("session 'a' port=11111"), "got: {msg}");
        assert!(msg.contains("session 'b' port=22222"), "got: {msg}");
    }

    #[test]
    fn resolve_unique_session_ambiguous_same_scope() {
        // Same-scope multi-session (e.g. dual-attach C++/Java) is the canonical
        // case where --scope-id alone cannot disambiguate.
        let sessions = vec![
            fake_session("cpp", Some(11111), Some("vscode-7")),
            fake_session("java", Some(22222), Some("vscode-7")),
        ];
        let err = resolve_unique_session(sessions, &Some("vscode-7".into()), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Multiple active debug sessions"), "got: {msg}");
        assert!(msg.contains("scope 'vscode-7'"), "got: {msg}");
        assert!(msg.contains("--control-port"), "got: {msg}");
        // When scope is already set, the hint should NOT recommend --scope-id again.
        assert!(
            !msg.contains("if it narrows to one"),
            "scope-id hint should be suppressed when scope is already set; got: {msg}"
        );
        assert!(msg.contains("session 'cpp' port=11111"), "got: {msg}");
        assert!(msg.contains("session 'java' port=22222"), "got: {msg}");
    }

    #[test]
    fn resolve_unique_session_ambiguous_with_extra_hint() {
        let sessions = vec![
            fake_session("a", Some(11111), None),
            fake_session("b", Some(22222), None),
        ];
        let err = resolve_unique_session(
            sessions,
            &None,
            Some("MCP tool calls also accept a session_id argument."),
        )
        .unwrap_err();
        let msg = err.to_string();
        // Hint must appear before the candidate list so the message reads
        // coherently end-to-end.
        let hint_pos = msg
            .find("MCP tool calls also accept a session_id argument.")
            .expect("extra hint missing");
        let candidates_pos = msg
            .find("Active candidates:")
            .expect("candidate header missing");
        assert!(
            hint_pos < candidates_pos,
            "extra hint should come before the candidate list; got: {msg}"
        );
    }

    #[test]
    fn resolve_unique_session_single_no_port() {
        let err = resolve_unique_session(vec![fake_session("noport", None, None)], &None, None)
            .unwrap_err();
        assert!(err.to_string().contains("no control port"), "got: {err}");
    }

    #[test]
    fn resolve_unique_session_prefers_root_over_child() {
        // A child session appearing in the parent's scope must not break
        // zero-config targeting: the unique root (no parent) is preferred.
        let sessions = vec![
            fake_session("parent", Some(11111), Some("vscode-7")),
            fake_session("child", Some(22222), Some("vscode-7"))
                .with_parent_session_id(Some("parent".into())),
        ];
        let session = resolve_unique_session(sessions, &Some("vscode-7".into()), None).unwrap();
        assert_eq!(
            session.session_id,
            "parent".into(),
            "should resolve to the root, not the child"
        );
        assert_eq!(session.control_plane_port.unwrap().get(), 11111);
    }

    #[test]
    fn resolve_unique_session_prefers_root_over_multiple_children() {
        let sessions = vec![
            fake_session("parent", Some(11111), None),
            fake_session("child1", Some(22222), None).with_parent_session_id(Some("parent".into())),
            fake_session("child2", Some(33333), None).with_parent_session_id(Some("parent".into())),
        ];
        let session = resolve_unique_session(sessions, &None, None).unwrap();
        assert_eq!(session.session_id, "parent".into());
    }

    #[test]
    fn resolve_unique_session_two_roots_still_ambiguous() {
        // Two roots (no children) remain ambiguous — there is no unique root to
        // prefer, so the user must disambiguate explicitly.
        let sessions = vec![
            fake_session("root-a", Some(11111), None),
            fake_session("root-b", Some(22222), None),
        ];
        let err = resolve_unique_session(sessions, &None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Multiple active debug sessions"), "got: {msg}");
        // The error must render the candidate list so the user can pick one.
        assert!(
            msg.contains("root-a"),
            "candidates should be listed, got: {msg}"
        );
        assert!(
            msg.contains("root-b"),
            "candidates should be listed, got: {msg}"
        );
    }

    #[test]
    fn resolve_unique_session_children_only_is_ambiguous() {
        // No root in scope (only children of parents elsewhere): there is no
        // unique root to prefer, so it stays ambiguous — targeting a child
        // requires an explicit --control-port or session_id.
        let sessions = vec![
            fake_session("child1", Some(11111), None).with_parent_session_id(Some("p1".into())),
            fake_session("child2", Some(22222), None).with_parent_session_id(Some("p2".into())),
        ];
        let err = resolve_unique_session(sessions, &None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Multiple active debug sessions"), "got: {msg}");
        assert!(
            msg.contains("child1"),
            "candidates should be listed, got: {msg}"
        );
        assert!(
            msg.contains("child2"),
            "candidates should be listed, got: {msg}"
        );
    }

    #[test]
    fn resolve_unique_session_root_no_port_errors() {
        // The unique root is preferred, but if it has no control port we report
        // the root-specific error rather than falling through to ambiguity.
        let sessions = vec![
            fake_session("parent", None, Some("vscode-7")),
            fake_session("child", Some(22222), Some("vscode-7"))
                .with_parent_session_id(Some("parent".into())),
        ];
        let err = resolve_unique_session(sessions, &Some("vscode-7".into()), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("has no control port"), "got: {msg}");
        assert!(
            !msg.contains("Multiple active debug sessions"),
            "should not fall through to ambiguity; got: {msg}"
        );
    }
}
