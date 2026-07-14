// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#![warn(clippy::all)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dapper_control_api::ControlPlaneResult;
use dapper_control_api::ControlPlaneServer;
use dapper_control_api::DapperControlPlane;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_dap_protocol::protocol as dap;
use dapper_dap_protocol::requests::DisconnectArguments;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_proxy_server::ControlPlaneStatus;
use dapper_proxy_server::DapperEvent;
use dapper_proxy_server::ProxyClient;
use dapper_session::NavigationResult;
use dapper_session::NavigationType;
use dapper_session::Port;
use dapper_session::RawDapResult;
use dapper_session::ScopeId;
use dapper_session::ScopesResult;
use dapper_session::SessionId;
use dapper_session::SetBreakpointsResult;
use dapper_session::SetExceptionBreakpointsResult;
use dapper_session::SetVariableResult;
use dapper_session::StackTraceResult;
use dapper_session::StatusResult;
use dapper_session::ThreadsResult;
use dapper_session::VariablesResult;

/// A hook that tears down child sessions before the parent proxy shuts down.
/// Kept generic (a boxed async closure) so this crate has no upward dependency
/// on the CLI's child supervisor.
pub type ChildTeardownHook =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[derive(Clone)]
pub struct DapperControlPlaneServiceImpl {
    proxy_server_abort: tokio::task::AbortHandle,
    proxy_client: ProxyClient,
    /// Optional hook to tear down child sessions before this proxy stops, so an
    /// MCP/CLI `stop` of the parent does not orphan its children.
    child_teardown: Option<ChildTeardownHook>,
}

impl DapperControlPlaneServiceImpl {
    fn structured<T>(&self, result: T) -> ControlPlaneResult<T> {
        let context = self.proxy_client.debug_session_tracker().response_context();
        ControlPlaneResult { result, context }
    }
}

#[async_trait]
impl DapperControlPlane for DapperControlPlaneServiceImpl {
    async fn eval_repl(&self, command: &str, frame_id: Option<FrameId>) -> anyhow::Result<String> {
        self.proxy_client.repl(command.to_owned(), frame_id).await
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // Tear down child sessions first (children-before-parent) so an MCP/CLI
        // `stop` of the parent doesn't orphan them.
        if let Some(teardown) = &self.child_teardown {
            teardown().await;
        }
        let request = dap::Request::new(RequestCommand::Disconnect(Some(DisconnectArguments {
            terminate_debuggee: Some(false),
            suspend_debuggee: Some(false),
            ..Default::default()
        })));
        let _ = self
            .proxy_client
            .send_message_with_timeout(request.into(), Duration::from_secs(5))
            .await;

        self.proxy_server_abort.abort();
        Ok(())
    }

    async fn threads(&self) -> anyhow::Result<ControlPlaneResult<ThreadsResult>> {
        let result = self.proxy_client.threads().await?;
        Ok(self.structured(result))
    }

    async fn stack_trace(
        &self,
        thread_id: ThreadId,
        start_frame: Option<i64>,
        levels: Option<i64>,
    ) -> anyhow::Result<ControlPlaneResult<StackTraceResult>> {
        let result = self
            .proxy_client
            .stack_trace(thread_id, start_frame, levels)
            .await?;
        Ok(self.structured(result))
    }

    async fn scopes(&self, frame_id: FrameId) -> anyhow::Result<ControlPlaneResult<ScopesResult>> {
        let result = self.proxy_client.scopes(frame_id).await?;
        Ok(self.structured(result))
    }

    async fn variables(
        &self,
        variables_reference: VariablesReference,
    ) -> anyhow::Result<ControlPlaneResult<VariablesResult>> {
        let result = self.proxy_client.variables(variables_reference).await?;
        Ok(self.structured(result))
    }

    async fn navigate(
        &self,
        navigation_type: NavigationType,
        thread_id: ThreadId,
        single_thread: Option<bool>,
    ) -> anyhow::Result<ControlPlaneResult<NavigationResult>> {
        let result = self
            .proxy_client
            .navigate(navigation_type, thread_id, single_thread)
            .await?;
        Ok(self.structured(result))
    }

    async fn set_variable(
        &self,
        variables_reference: VariablesReference,
        name: &str,
        value: &str,
    ) -> anyhow::Result<ControlPlaneResult<SetVariableResult>> {
        let result = self
            .proxy_client
            .set_variable(variables_reference, name, value)
            .await?;
        Ok(self.structured(result))
    }

    async fn set_breakpoints(
        &self,
        source_path: &str,
        clear_existing: bool,
        breakpoint_specs: &[SourceBreakpoint],
    ) -> anyhow::Result<ControlPlaneResult<SetBreakpointsResult>> {
        let result = self
            .proxy_client
            .set_breakpoints(source_path, clear_existing, breakpoint_specs)
            .await?;
        Ok(self.structured(result))
    }

    async fn set_exception_breakpoints(
        &self,
        filters: &[String],
        clear_existing: bool,
    ) -> anyhow::Result<ControlPlaneResult<SetExceptionBreakpointsResult>> {
        let result = self
            .proxy_client
            .set_exception_breakpoints(filters, clear_existing)
            .await?;
        Ok(self.structured(result))
    }

    async fn send_dap_request(
        &self,
        command: &str,
        arguments: Option<serde_json::Value>,
        wait_for_event: bool,
        timeout_seconds: u64,
    ) -> anyhow::Result<RawDapResult> {
        self.proxy_client
            .send_raw_dap_request(command, arguments, wait_for_event, timeout_seconds)
            .await
    }

    async fn capabilities(&self) -> anyhow::Result<Option<String>> {
        let caps = self
            .proxy_client
            .debug_session_tracker()
            .adapter_capabilities();
        match caps {
            Some(c) => Ok(Some(serde_json::to_string(&c)?)),
            None => Ok(None),
        }
    }

    async fn status(&self) -> anyhow::Result<ControlPlaneResult<StatusResult>> {
        Ok(self.structured(StatusResult::default()))
    }
}

fn send_dapper_event(
    proxy_client: ProxyClient,
    serve_result: &anyhow::Result<ControlPlaneServer>,
    session_id: &SessionId,
) {
    let event = match serve_result.as_ref() {
        Ok(control_plane_server) => DapperEvent::ControlPlaneStatus(ControlPlaneStatus::success(
            session_id.clone(),
            control_plane_server.port,
        )),
        Err(err) => DapperEvent::ControlPlaneStatus(ControlPlaneStatus::failure(
            session_id.clone(),
            format!("{err:#}"),
        )),
    };

    let _ = proxy_client.send_dapper_event(event);
}

pub async fn start_control_plane(
    port: Option<Port>,
    proxy_client: ProxyClient,
    proxy_server_abort: tokio::task::AbortHandle,
    session_id: &SessionId,
    scope_id: Option<ScopeId>,
    child_teardown: Option<ChildTeardownHook>,
) -> anyhow::Result<ControlPlaneServer> {
    let handler = DapperControlPlaneServiceImpl {
        proxy_server_abort,
        proxy_client: proxy_client.clone(),
        child_teardown,
    };

    let result = dapper_control_api::serve(port, handler).await;

    let port = result.as_ref().ok().map(|server| server.port);
    proxy_client
        .debug_session_tracker()
        .register_control_plane(port, scope_id);

    send_dapper_event(proxy_client, &result, session_id);

    result
}
