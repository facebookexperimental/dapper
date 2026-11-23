// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_access::NotPermitted;
use dapper_access::Operation;
use dapper_access::Policy;
use dapper_access::operations_needed_by;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_session::CapabilitiesResult;
use dapper_session::Expansion;
use dapper_session::NavigationResult;
use dapper_session::NavigationType;
use dapper_session::RawDapResult;
use dapper_session::ScopesResult;
use dapper_session::SetBreakpointsResult;
use dapper_session::SetExceptionBreakpointsResult;
use dapper_session::SetVariableResult;
use dapper_session::StackTraceResult;
use dapper_session::StatusResult;
use dapper_session::ThreadsResult;
use dapper_session::VariablesResult;
use enumset::EnumSet;

use crate::ControlPlaneResult;
use crate::DapperControlPlane;

/// A control plane that refuses the calls `policy` does not allow before they
/// reach `inner`.
pub(crate) struct Enforced<T> {
    pub(crate) inner: T,
    pub(crate) policy: Policy,
}

impl<T> Enforced<T> {
    fn check(&self, call: &str, needed: impl Into<EnumSet<Operation>>) -> Result<(), NotPermitted> {
        self.policy.check(call, needed.into())
    }
}

#[async_trait::async_trait]
impl<T> DapperControlPlane for Enforced<T>
where
    T: DapperControlPlane,
{
    async fn eval_repl(&self, command: &str, frame_id: Option<FrameId>) -> anyhow::Result<String> {
        self.check("eval_repl", Operation::Evaluate)?;
        self.inner.eval_repl(command, frame_id).await
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.check("stop", Operation::Stop)?;
        self.inner.stop().await
    }

    async fn threads(
        &self,
        expansion: Expansion,
    ) -> anyhow::Result<ControlPlaneResult<ThreadsResult>> {
        self.check("threads", Operation::Inspect)?;
        self.inner.threads(expansion).await
    }

    async fn stack_trace(
        &self,
        thread_id: ThreadId,
        start_frame: Option<i64>,
        levels: Option<i64>,
        expansion: Expansion,
    ) -> anyhow::Result<ControlPlaneResult<StackTraceResult>> {
        self.check("stack_trace", Operation::Inspect)?;
        self.inner
            .stack_trace(thread_id, start_frame, levels, expansion)
            .await
    }

    async fn scopes(
        &self,
        frame_id: FrameId,
        expansion: Expansion,
    ) -> anyhow::Result<ControlPlaneResult<ScopesResult>> {
        self.check("scopes", Operation::Inspect)?;
        self.inner.scopes(frame_id, expansion).await
    }

    async fn variables(
        &self,
        variables_reference: VariablesReference,
    ) -> anyhow::Result<ControlPlaneResult<VariablesResult>> {
        self.check("variables", Operation::ReadValues)?;
        self.inner.variables(variables_reference).await
    }

    async fn navigate(
        &self,
        navigation_type: NavigationType,
        thread_id: ThreadId,
        single_thread: Option<bool>,
    ) -> anyhow::Result<ControlPlaneResult<NavigationResult>> {
        self.check("navigate", Operation::Navigate)?;
        self.inner
            .navigate(navigation_type, thread_id, single_thread)
            .await
    }

    async fn set_variable(
        &self,
        variables_reference: VariablesReference,
        name: &str,
        value: &str,
    ) -> anyhow::Result<ControlPlaneResult<SetVariableResult>> {
        self.check("set_variable", Operation::SetVariable)?;
        self.inner
            .set_variable(variables_reference, name, value)
            .await
    }

    async fn set_breakpoints(
        &self,
        source_path: &str,
        clear_existing: bool,
        breakpoint_specs: &[SourceBreakpoint],
    ) -> anyhow::Result<ControlPlaneResult<SetBreakpointsResult>> {
        self.check("set_breakpoints", Operation::Breakpoints)?;
        self.inner
            .set_breakpoints(source_path, clear_existing, breakpoint_specs)
            .await
    }

    async fn set_exception_breakpoints(
        &self,
        filters: &[String],
        clear_existing: bool,
    ) -> anyhow::Result<ControlPlaneResult<SetExceptionBreakpointsResult>> {
        self.check("set_exception_breakpoints", Operation::Breakpoints)?;
        self.inner
            .set_exception_breakpoints(filters, clear_existing)
            .await
    }

    async fn send_dap_request(
        &self,
        command: RequestCommand,
        wait_for_event: bool,
        timeout_seconds: u64,
    ) -> anyhow::Result<RawDapResult> {
        self.check(command.command_name(), operations_needed_by(&command))?;
        self.inner
            .send_dap_request(command, wait_for_event, timeout_seconds)
            .await
    }

    async fn capabilities(&self) -> anyhow::Result<ControlPlaneResult<CapabilitiesResult>> {
        self.inner.capabilities().await
    }

    async fn status(&self) -> anyhow::Result<ControlPlaneResult<StatusResult>> {
        self.inner.status().await
    }
}
