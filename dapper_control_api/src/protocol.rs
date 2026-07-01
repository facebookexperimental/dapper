// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;

use crate::ControlPlaneResult;
use crate::NavigationResult;
use crate::RawDapResult;
use crate::ScopesResult;
use crate::SetBreakpointsResult;
use crate::SetExceptionBreakpointsResult;
use crate::SetVariableResult;
use crate::StackTraceResult;
use crate::StatusResult;
use crate::ThreadsResult;
use crate::VariablesResult;

#[async_trait::async_trait]
pub trait DapperControlPlane: Send + Sync {
    /// Evaluate a REPL command
    async fn eval_repl(&self, command: &str, frame_id: Option<FrameId>) -> anyhow::Result<String>;

    /// Request the server to shutdown
    async fn stop(&self) -> anyhow::Result<()>;

    /// List all available threads in the debugged process
    async fn threads(&self) -> anyhow::Result<ControlPlaneResult<ThreadsResult>>;

    /// Get stack trace for a specific thread
    async fn stack_trace(
        &self,
        thread_id: ThreadId,
        start_frame: Option<i64>,
        levels: Option<i64>,
    ) -> anyhow::Result<ControlPlaneResult<StackTraceResult>>;

    /// Get scopes for a specific stack frame
    async fn scopes(&self, frame_id: FrameId) -> anyhow::Result<ControlPlaneResult<ScopesResult>>;

    /// Get variables for a specific variables reference
    async fn variables(
        &self,
        variables_reference: VariablesReference,
    ) -> anyhow::Result<ControlPlaneResult<VariablesResult>>;

    /// Navigate debugger execution (step in/over/out or continue) for
    /// all threads or specified thread id.
    /// When `single_thread` is true, all other suspended threads are not resumed.
    /// Requires the adapter to advertise `supportsSingleThreadExecutionRequests`.
    async fn navigate(
        &self,
        navigation_type: crate::NavigationType,
        thread_id: ThreadId,
        single_thread: Option<bool>,
    ) -> anyhow::Result<ControlPlaneResult<NavigationResult>>;

    /// Set a variable value in a specific variables reference
    async fn set_variable(
        &self,
        variables_reference: VariablesReference,
        name: &str,
        value: &str,
    ) -> anyhow::Result<ControlPlaneResult<SetVariableResult>>;

    /// Add breakpoints at specific lines in a source file
    ///
    /// Each SourceBreakpoint specifies a line and optional per-breakpoint condition/log_message.
    /// When clear_existing is false (default), new breakpoints are appended to existing ones
    /// When clear_existing is true, all existing breakpoints in the file are removed before adding new ones
    async fn set_breakpoints(
        &self,
        source_path: &str,
        clear_existing: bool,
        breakpoint_specs: &[SourceBreakpoint],
    ) -> anyhow::Result<ControlPlaneResult<SetBreakpointsResult>>;

    /// Set the active exception breakpoint filters at the debug adapter.
    ///
    /// `filters` is a list of adapter-advertised filter ids (e.g.
    /// "raised", "uncaught"). With `clear_existing=true`, the new set
    /// replaces the active filters verbatim. With `clear_existing=false`,
    /// the explicit list is merged with the currently-installed set;
    /// existing conditions on re-specified filters are preserved (since
    /// the v1 surface has no way to express conditions).
    ///
    /// Empty filters with `clear_existing=false` is a permissive no-op
    /// at the library boundary (returns the current installed state).
    /// Empty filters with `clear_existing=true` clears all filters.
    async fn set_exception_breakpoints(
        &self,
        filters: &[String],
        clear_existing: bool,
    ) -> anyhow::Result<ControlPlaneResult<SetExceptionBreakpointsResult>>;

    /// Send a raw DAP request with the given command and JSON arguments
    async fn send_dap_request(
        &self,
        command: &str,
        arguments: Option<serde_json::Value>,
        wait_for_event: bool,
        timeout_seconds: u64,
    ) -> anyhow::Result<RawDapResult>;

    /// Query the debug adapter's capabilities
    async fn capabilities(&self) -> anyhow::Result<Option<String>>;

    /// Get session status and context (execution state, stop reason, breakpoints, etc.)
    async fn status(&self) -> anyhow::Result<ControlPlaneResult<StatusResult>>;
}
