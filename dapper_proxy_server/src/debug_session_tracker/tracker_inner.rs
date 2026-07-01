// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::capabilities::Capabilities;
use dapper_session::Port;
use dapper_session::RequestType;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::SessionInfo;

use super::breakpoint_state::BreakpointState;
use super::exception_filter_state::ExceptionFilterState;
use super::execution_state::ExecutionState;
use super::output_state::OutputState;

/// Mutable debug session state protected by the tracker's Mutex.
///
/// Immutable fields (`session_id`, `config`) live on `DebugSessionTracker`
/// itself so they can be read without locking.
#[derive(Debug)]
pub(super) struct DebugSessionTrackerInner {
    /// Breakpoint tracking state
    pub breakpoint_state: BreakpointState,
    /// Exception breakpoint filter tracking state
    pub exception_filter_state: ExceptionFilterState,
    /// Execution state of the debuggee
    pub execution_state: ExecutionState,
    /// Output messages buffer
    pub output_state: OutputState,
    /// Control plane port (set after control plane starts)
    pub control_plane_port: Option<Port>,
    /// Scope ID for session filtering
    pub scope_id: Option<ScopeId>,
    /// Request type from launch/attach command
    pub request_type: Option<RequestType>,
    /// Debugger arguments from launch/attach request
    pub debugger_args: Option<serde_json::Value>,
    /// Session info to write to file (created when both port and debugger_args are available)
    pub session_info: Option<SessionInfo>,
    /// Whether the session file has been written
    pub session_file_written: bool,
    /// Full adapter capabilities from the initialize response
    pub adapter_capabilities: Option<Capabilities>,
}

impl DebugSessionTrackerInner {
    pub fn new(session_id: &SessionId, max_output_lines: usize) -> Self {
        let output_state = OutputState::new(session_id, max_output_lines);
        Self {
            breakpoint_state: BreakpointState::new(),
            exception_filter_state: ExceptionFilterState::new(),
            execution_state: ExecutionState::default(),
            output_state,
            control_plane_port: None,
            scope_id: None,
            request_type: None,
            debugger_args: None,
            session_info: None,
            session_file_written: false,
            adapter_capabilities: None,
        }
    }

    pub fn try_finalize_session(
        &mut self,
        session_id: &SessionId,
        parent_session_id: Option<&SessionId>,
    ) {
        if self.session_file_written {
            return;
        }

        if self.session_info.is_none()
            && let (Some(port), Some(args)) = (self.control_plane_port, &self.debugger_args)
        {
            self.session_info = Some(
                SessionInfo::generate(
                    session_id.clone(),
                    Some(port),
                    self.scope_id.clone(),
                    self.request_type,
                    Some(args.clone()),
                )
                .with_parent_session_id(parent_session_id.cloned()),
            );
        }

        if let Some(ref session_info) = self.session_info {
            match session_info.write_to_file() {
                Ok(path) => {
                    tracing::info!("Session file written to: {}", path.display());
                    self.session_file_written = true;
                }
                Err(e) => {
                    tracing::warn!("Failed to write session file: {}", e);
                }
            }
        }
    }
}
