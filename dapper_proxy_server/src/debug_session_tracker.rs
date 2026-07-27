// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

mod breakpoint_state;
mod exception_filter_state;
mod execution_state;
mod output_state;
mod tracker_inner;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

pub use breakpoint_state::BreakpointDiff;
pub(crate) use breakpoint_state::breakpoints_with_fallback;
pub(crate) use breakpoint_state::resolved_source_path;
use dapper_config::DapperConfig;
use dapper_dap_protocol::capabilities::Capabilities;
use dapper_dap_protocol::capabilities::apply_capabilities_event;
use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::events::EventKind;
use dapper_dap_protocol::protocol::Event;
use dapper_dap_protocol::protocol::Message;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::responses::ResponseBody;
pub use dapper_session::BreakpointInfo;
pub use dapper_session::ExceptionFilterEntry;
use dapper_session::Port;
use dapper_session::RequestType;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::SessionInfo;
use dapper_session::SessionStore;
pub use execution_state::ExecutionState;
use tracker_inner::DebugSessionTrackerInner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientType {
    Main,
    Secondary,
}

#[derive(Debug, Clone)]
pub struct DebugSessionTracker {
    /// Session ID — immutable after construction, readable without locking.
    session_id: SessionId,
    /// The parent proxy's session id, when this proxy was spawned as a child
    /// via a `startDebugging` reverse request. Immutable after construction;
    /// stamped into the written `SessionInfo`. `None` for root sessions.
    parent_session_id: Option<SessionId>,
    /// Configuration — immutable after construction, readable without locking.
    config: DapperConfig,
    /// Where session files are written and other sessions are discovered.
    /// `None` when no sessions dir is available: session files are skipped
    /// (with a warning) and other-session discovery is empty.
    sessions: Option<SessionStore>,
    inner: Arc<Mutex<DebugSessionTrackerInner>>,
}

impl DebugSessionTracker {
    /// Create a new debug session tracker
    ///
    /// Tracking is done via direct function calls (track_message_from_client, track_message_to_client).
    ///
    /// # Arguments
    /// * `session_id` - Unique identifier for this debug session
    /// * `config` - Dapper configuration (the caller's single loaded copy)
    /// * `sessions` - Session store for session files, if one is available
    pub fn new(
        session_id: SessionId,
        config: DapperConfig,
        sessions: Option<SessionStore>,
    ) -> Self {
        let inner = Arc::new(Mutex::new(DebugSessionTrackerInner::new(
            &session_id,
            config.context.max_output_lines,
        )));
        Self {
            session_id,
            parent_session_id: None,
            config,
            sessions,
            inner,
        }
    }

    /// Set the parent session id (the proxy that spawned this one as a child).
    /// Must be set before any client is created so all tracker clones observe
    /// it. `None` for root sessions.
    pub fn with_parent_session_id(mut self, parent_session_id: Option<SessionId>) -> Self {
        self.parent_session_id = parent_session_id;
        self
    }

    /// Acquire the inner lock and apply `f`, recovering from poisoning.
    /// Tracker state is advisory bookkeeping: skipping updates on a
    /// poisoned lock would silently drop every subsequent update, while
    /// recovery risks at most the one update a panicking writer left
    /// unfinished.
    fn with_inner<R>(&self, f: impl FnOnce(&mut DebugSessionTrackerInner) -> R) -> R {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Track a message from a client going to the backend.
    pub fn track_message_from_client(&self, message: &Message, client_type: ClientType) {
        if let Message::Request(request) = message {
            match &request.command {
                // Only track the set breakpoints request for the main client
                // As the 2nd clients already track the breakpoints they created explicitly in
                // their set breakpoints function.
                RequestCommand::SetBreakpoints(bp_args) if client_type == ClientType::Main => {
                    if let Some(source_path) = &bp_args.source.path {
                        let specs = bp_args.breakpoints.as_deref().unwrap_or_default().to_vec();
                        self.track_breakpoint_request(request.seq, source_path, specs);
                    }
                }
                // Only track for the main client; secondary clients get their
                // responses via the broadcast subscription (not through
                // `track_message_to_client`) and update the tracker via the
                // explicit `update_exception_filters` setter instead.
                RequestCommand::SetExceptionBreakpoints(args)
                    if client_type == ClientType::Main =>
                {
                    let entries = exception_filter_state::parse_request_entries(args);
                    self.track_exception_filter_request(request.seq, entries);
                }
                // Track launch/attach requests to capture session info
                RequestCommand::Launch(args) => {
                    self.track_launch_or_attach(
                        RequestType::Launch,
                        serde_json::to_value(&args.extra).ok(),
                    );
                }
                RequestCommand::Attach(args) => {
                    self.track_launch_or_attach(
                        RequestType::Attach,
                        serde_json::to_value(&args.extra).ok(),
                    );
                }
                _ => {}
            }
        }

        ExecutionState::track_message_from_client(&self.inner, message);
    }

    /// Track a message from the backend going to the main client
    pub fn track_message_to_client(&self, message: &Message) {
        if let Message::Response(response) = message {
            if response.success {
                if let ResponseBody::SetBreakpoints(bp_body) = &response.body {
                    self.with_inner(|inner| {
                        inner.breakpoint_state.update_breakpoints_from_response(
                            response.request_seq,
                            &bp_body.breakpoints,
                        );
                    });
                }
                if let ResponseBody::Initialize(caps) = &response.body {
                    self.with_inner(|inner| {
                        inner.adapter_capabilities = caps.clone();
                    });
                }
            }
            // Process exception breakpoint responses regardless of success
            // so the pending request entry is always cleaned up. The
            // installed set is only replaced on success. (This differs
            // from the source-line breakpoint pattern above, which is
            // success-gated; the asymmetry is intentional — the new
            // approach prevents stale pending entries from accumulating
            // on adapter-rejected requests.)
            if let ResponseBody::SetExceptionBreakpoints(_) = &response.body {
                let success = response.success;
                let request_seq = response.request_seq;
                self.with_inner(|inner| {
                    inner
                        .exception_filter_state
                        .complete_response(request_seq, success);
                });
            }
        }
        if let Message::Event(event) = message {
            match &event.event {
                EventKind::Output(_) => self.track_output_event(event),
                EventKind::Breakpoint(bp_event) => {
                    self.with_inner(|inner| {
                        inner
                            .breakpoint_state
                            .apply_breakpoint_event(&bp_event.reason, &bp_event.breakpoint);
                    });
                }
                EventKind::Capabilities(caps_event) => {
                    // Absorb post-initialize capability updates so gates see them.
                    self.with_inner(|inner| {
                        apply_capabilities_event(
                            &mut inner.adapter_capabilities,
                            caps_event.capabilities.clone(),
                        );
                    });
                }
                _ => {}
            }
        }

        ExecutionState::track_message_to_client(&self.inner, message);
    }

    pub fn get_breakpoints(&self, source_path: &str) -> Vec<BreakpointInfo> {
        self.with_inner(|inner| inner.breakpoint_state.get_breakpoints(source_path))
    }

    pub fn register_control_plane(
        &self,
        control_plane_port: Option<Port>,
        scope_id: Option<ScopeId>,
    ) {
        let to_save = self.with_inner(|inner| {
            inner.control_plane_port = control_plane_port;
            inner.scope_id = scope_id;
            inner.try_finalize_session(&self.session_id, self.parent_session_id.as_ref())
        });
        self.save_session_file(to_save);
    }

    pub fn get_session_info(&self) -> Option<SessionInfo> {
        self.with_inner(|inner| inner.session_info.clone())
    }

    pub fn adapter_capabilities(&self) -> Option<Capabilities> {
        self.with_inner(|inner| inner.adapter_capabilities.clone())
    }

    pub fn get_execution_state(&self) -> ExecutionState {
        self.with_inner(|inner| inner.execution_state.clone())
    }

    pub fn is_stopped(&self) -> bool {
        self.with_inner(|inner| inner.execution_state.is_all_stopped())
    }

    /// Track a setBreakpoints request's source path to match with its response later
    fn track_breakpoint_request(&self, seq: Seq, source_path: &str, specs: Vec<SourceBreakpoint>) {
        self.with_inner(|inner| {
            inner
                .breakpoint_state
                .track_request(seq, source_path.to_string(), specs);
        });
    }

    /// Track a setExceptionBreakpoints request keyed by its main-client seq
    /// so we can commit the entries on a successful response.
    fn track_exception_filter_request(&self, seq: Seq, entries: Vec<ExceptionFilterEntry>) {
        self.with_inner(|inner| {
            inner.exception_filter_state.track_request(seq, entries);
        });
    }

    /// Replace the installed exception filter set wholesale. Used by
    /// secondary clients (control-plane MCP/CLI) whose responses don't
    /// reach `track_message_to_client`. Caller is responsible for passing
    /// the post-builder *effective* set (i.e. what the adapter actually
    /// accepted, with any unsupported conditions already dropped).
    pub fn update_exception_filters(&self, entries: Vec<ExceptionFilterEntry>) {
        self.with_inner(|inner| {
            inner.exception_filter_state.replace(entries);
        });
    }

    /// Read the current installed exception filter set, sorted by filter id.
    pub fn get_installed_exception_filters(&self) -> Vec<ExceptionFilterEntry> {
        self.with_inner(|inner| inner.exception_filter_state.get_installed().to_vec())
    }

    /// Track launch or attach requests to capture session information
    fn track_launch_or_attach(
        &self,
        request_type: RequestType,
        debugger_args: Option<serde_json::Value>,
    ) {
        let session_id = &self.session_id;
        let to_save = self.with_inner(|inner| {
            inner.debugger_args = debugger_args;
            inner.request_type = Some(request_type);

            tracing::debug!(
                request_type = ?request_type,
                "Captured session info from {:?} request",
                request_type
            );

            inner.try_finalize_session(session_id, self.parent_session_id.as_ref())
        });
        self.save_session_file(to_save);
    }

    /// Persist a finalized `SessionInfo` outside the tracker lock (file IO
    /// must not block message tracking). Racing finalizers may both save;
    /// each save atomically renames a consistent snapshot, so either may win.
    fn save_session_file(&self, info: Option<SessionInfo>) {
        let Some(info) = info else { return };
        let Some(store) = &self.sessions else {
            tracing::warn!("Session store unavailable; session file not written");
            return;
        };
        match store.save(&info) {
            Ok(path) => {
                tracing::info!("Session file written to: {}", path.display());
                self.with_inner(|inner| inner.session_file_written = true);
            }
            Err(e) => {
                tracing::warn!("Failed to write session file: {}", e);
            }
        }
    }

    /// Track breakpoints when source path is explicitly known.
    /// This is used by the MCP set_breakpoints command where the source path is known upfront.
    /// `requested_source_path` is the original path from the caller; `effective_source_path`
    /// is the resolved path from the debug adapter response (may be the same).
    /// Returns a diff of breakpoints to add and remove.
    pub fn track_breakpoints_with_source(
        &self,
        requested_source_path: &str,
        effective_source_path: &str,
        new_breakpoints: Vec<BreakpointInfo>,
    ) -> BreakpointDiff {
        self.with_inner(|inner| {
            let old_breakpoints = inner
                .breakpoint_state
                .get_breakpoints(effective_source_path);
            let diff = Self::compute_breakpoint_diff(&old_breakpoints, &new_breakpoints);
            inner
                .breakpoint_state
                .update_breakpoints(effective_source_path, new_breakpoints);
            if requested_source_path != effective_source_path {
                inner
                    .breakpoint_state
                    .record_path_alias(requested_source_path, effective_source_path);
            }
            diff
        })
    }

    /// Track output events from the debugger (stdout, stderr, console)
    fn track_output_event(&self, event: &Event) {
        if let EventKind::Output(output) = &event.event {
            let dap_seq = event.seq;
            self.with_inner(|inner| {
                if let Err(e) =
                    inner
                        .output_state
                        .add_output(&output.output, output.category.as_ref(), dap_seq)
                {
                    tracing::warn!(
                        error = %e,
                        event = "output",
                        dap_seq = dap_seq.as_i64(),
                        "Failed to write output to file"
                    );
                }
            });
        }
    }

    pub fn response_context(&self) -> Option<dapper_session::ResponseContext> {
        if !self.config.context.enable {
            let session = if self.config.context.show_session {
                self.with_inner(|inner| inner.session_info.clone())
            } else {
                None
            };
            return Some(dapper_session::ResponseContext {
                session,
                ..Default::default()
            });
        }

        let ctx = &self.config.context;

        // Single lock acquisition for all mutable state, then drop
        // the lock before any filesystem I/O.
        let (
            session,
            execution_state,
            breakpoints,
            installed_exception_filters,
            output,
            output_history_file,
        ) = self.with_inner(|inner| {
            let session = if ctx.show_session {
                inner.session_info.clone()
            } else {
                None
            };

            let execution_state = if ctx.show_execution_state {
                Some(inner.execution_state.summary())
            } else {
                None
            };

            let breakpoints = if ctx.show_breakpoints {
                inner.breakpoint_state.breakpoints.clone()
            } else {
                Default::default()
            };

            let installed_exception_filters = if ctx.show_exception_breakpoints {
                inner.exception_filter_state.get_installed().to_vec()
            } else {
                Vec::new()
            };

            let (output, output_history_file) =
                if ctx.max_output_lines > 0 && inner.output_state.has_buffered_output() {
                    let path = inner.output_state.output_file_path().to_path_buf();
                    let buffered = inner.output_state.take_buffered_output();
                    (buffered, Some(path))
                } else {
                    (Default::default(), None)
                };

            (
                session,
                execution_state,
                breakpoints,
                installed_exception_filters,
                output,
                output_history_file,
            )
        });

        // Filesystem I/O happens here, outside the lock.
        let other_sessions = match (&self.sessions, ctx.show_sessions) {
            (Some(store), true) => {
                let scope_id = store
                    .find_active_session_with_id(None, &self.session_id)
                    .and_then(|s| s.scope_id);

                store
                    .iter_active_sessions(scope_id)
                    .filter(|s| s.session_id != self.session_id)
                    .collect()
            }
            _ => vec![],
        };

        Some(dapper_session::ResponseContext {
            session,
            other_sessions,
            execution_state,
            breakpoints,
            installed_exception_filters,
            output,
            output_history_file,
            ..Default::default()
        })
    }

    /// Compute the difference between old and new breakpoints
    fn compute_breakpoint_diff(
        old_breakpoints: &[BreakpointInfo],
        new_breakpoints: &[BreakpointInfo],
    ) -> BreakpointDiff {
        let mut to_add = Vec::new();
        let mut to_remove = Vec::new();

        // Find breakpoints to add (in new but not in old, or changed)
        for new_bp in new_breakpoints {
            let existing = old_breakpoints
                .iter()
                .find(|old_bp| old_bp.line == new_bp.line);

            match existing {
                Some(old_bp) if old_bp == new_bp => {
                    // Same line, same contents - no change needed
                }
                Some(old_bp) => {
                    // Same line, different contents - remove old, add new
                    to_remove.push(old_bp.clone());
                    to_add.push(new_bp.clone());
                }
                None => {
                    // New breakpoint - add it
                    to_add.push(new_bp.clone());
                }
            }
        }

        // Find breakpoints to remove (in old but not in new)
        for old_bp in old_breakpoints {
            if !new_breakpoints
                .iter()
                .any(|new_bp| new_bp.line == old_bp.line)
            {
                to_remove.push(old_bp.clone());
            }
        }

        tracing::debug!(
            to_add_count = to_add.len(),
            to_add_lines = ?to_add.iter().map(|bp| (bp.line, bp.id)).collect::<Vec<_>>(),
            to_remove_count = to_remove.len(),
            to_remove_lines = ?to_remove.iter().map(|bp| (bp.line, bp.id)).collect::<Vec<_>>(),
            "Breakpoint diff computed"
        );

        BreakpointDiff { to_add, to_remove }
    }
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::data_types::Breakpoint as DapBreakpoint;
    use dapper_dap_protocol::data_types::Seq;
    use dapper_dap_protocol::data_types::Source as DapSource;
    use dapper_dap_protocol::data_types::SourceBreakpoint;
    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::enums::BreakpointEventReason;
    use dapper_dap_protocol::enums::OutputCategory;
    use dapper_dap_protocol::enums::StoppedReason;
    use dapper_dap_protocol::events::BreakpointEventBody;
    use dapper_dap_protocol::events::CapabilitiesEventBody;
    use dapper_dap_protocol::events::EventKind;
    use dapper_dap_protocol::events::OutputEventBody;
    use dapper_dap_protocol::events::StoppedEventBody;
    use dapper_dap_protocol::protocol::Event;
    use dapper_dap_protocol::protocol::Request;
    use dapper_dap_protocol::protocol::Response;
    use dapper_dap_protocol::requests::ContinueArguments;
    use dapper_dap_protocol::requests::RequestCommand;
    use dapper_dap_protocol::requests::SetBreakpointsArguments;
    use dapper_dap_protocol::responses::ContinueResponseBody;
    use dapper_dap_protocol::responses::ResponseBody;
    use dapper_dap_protocol::responses::SetBreakpointsResponseBody;

    use super::*;

    fn test_store() -> SessionStore {
        SessionStore::at(dapper_session::get_user_temp_dir().join("tracker_test_sessions"))
    }

    fn test_tracker_with_id(session_id: SessionId) -> DebugSessionTracker {
        DebugSessionTracker::new(session_id, DapperConfig::default(), Some(test_store()))
    }

    fn test_tracker() -> DebugSessionTracker {
        test_tracker_with_id("test-session".into())
    }

    #[test]
    fn test_parse_breakpoints_with_source_in_response() {
        let tracker = test_tracker();

        let source_path = "/some/path/to/main.cpp";
        let request_seq = Seq(42);

        // Create a setBreakpoints request message
        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![
                    SourceBreakpoint {
                        line: 16,
                        ..Default::default()
                    },
                    SourceBreakpoint {
                        line: 18,
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
        };

        // Track the request using the public API
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        // Construct a response with breakpoints containing source info (C++ debugger style)
        let response = Response {
            seq: 100.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![
                    DapBreakpoint {
                        column: Some(32),
                        id: Some(12.into()),
                        line: Some(16),
                        source: Some(DapSource {
                            name: Some("main.cpp".to_string()),
                            path: Some(source_path.to_string()),
                            ..Default::default()
                        }),
                        verified: true,
                        ..Default::default()
                    },
                    DapBreakpoint {
                        column: Some(32),
                        id: Some(13.into()),
                        line: Some(18),
                        source: Some(DapSource {
                            name: Some("main.cpp".to_string()),
                            path: Some(source_path.to_string()),
                            ..Default::default()
                        }),
                        verified: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
        };

        // Track the response using the public API
        tracker.track_message_to_client(&Message::Response(response));

        // Verify the breakpoints were stored
        let breakpoints = tracker.get_breakpoints(source_path);
        assert_eq!(breakpoints.len(), 2);
        assert_eq!(breakpoints[0].line, 16);
        assert!(breakpoints[0].verified);
        assert_eq!(breakpoints[0].id, Some(12.into()));
        assert_eq!(breakpoints[1].line, 18);
        assert!(breakpoints[1].verified);
        assert_eq!(breakpoints[1].id, Some(13.into()));
    }

    #[test]
    fn test_clear_breakpoints_with_empty_array() {
        let tracker = test_tracker();

        let source_path = "/some/path/to/main.cpp";

        // First, set some breakpoints
        let request_seq_set = Seq(42);
        let set_request = Request {
            seq: request_seq_set,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![SourceBreakpoint {
                    line: 16,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
        };

        // Track the set request using the public API
        tracker.track_message_from_client(&Message::Request(set_request), ClientType::Main);

        let set_response = Response {
            seq: 100.into(),
            request_seq: request_seq_set,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![DapBreakpoint {
                    id: Some(12.into()),
                    line: Some(16),
                    verified: true,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        };

        // Track the set response using the public API
        tracker.track_message_to_client(&Message::Response(set_response));

        // Verify breakpoints were set
        let breakpoints = tracker.get_breakpoints(source_path);
        assert_eq!(breakpoints.len(), 1);

        // Now clear the breakpoints with an empty array response (no source path in response)
        let request_seq_clear = Seq(43);
        let clear_request = Request {
            seq: request_seq_clear,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![]),
                ..Default::default()
            }),
        };

        // Track the clear request using the public API
        tracker.track_message_from_client(&Message::Request(clear_request), ClientType::Main);

        let clear_response = Response {
            seq: 101.into(),
            request_seq: request_seq_clear,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![],
                ..Default::default()
            }),
        };

        // Track the clear response using the public API
        tracker.track_message_to_client(&Message::Response(clear_response));

        // Verify breakpoints were cleared
        let breakpoints = tracker.get_breakpoints(source_path);
        assert_eq!(breakpoints.len(), 0);
    }

    #[test]
    fn test_track_output_event() {
        let session_id: SessionId = format!(
            "test-output-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
        .into();
        let tracker = test_tracker_with_id(session_id);

        let event = Event {
            seq: 100.into(),
            event: EventKind::Output(OutputEventBody {
                category: Some(OutputCategory::Stdout),
                output: "Hello, World!\n".to_string(),
                ..Default::default()
            }),
        };

        // Track the event using the public API
        tracker.track_message_to_client(&Message::Event(event));

        // Verify the output state was updated
        match tracker.inner.lock() {
            Ok(mut inner) => {
                let output_state = &mut inner.output_state;
                assert!(output_state.has_any());
                let content = output_state.read_last_lines(10).unwrap();
                assert!(content.contains("Hello, World!"));
                // Cleanup
                output_state.cleanup();
            }
            Err(e) => {
                panic!("Intentional test failure: failed to get inner lock: {}", e);
            }
        }
    }

    #[test]
    fn test_track_multiple_output_events() {
        let session_id: SessionId = format!(
            "test-multi-output-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
        .into();
        let tracker = test_tracker_with_id(session_id);

        // Send multiple output events
        for i in 1..=5 {
            let event = Event {
                seq: Seq(100 + i),
                event: EventKind::Output(OutputEventBody {
                    category: Some(OutputCategory::Stdout),
                    output: format!("Output line {}\n", i),
                    ..Default::default()
                }),
            };
            tracker.track_message_to_client(&Message::Event(event));
        }

        // Verify all output was captured
        match tracker.inner.lock() {
            Ok(mut inner) => {
                let output_state = &mut inner.output_state;
                assert!(output_state.has_any());
                let content = output_state.read_last_lines(10).unwrap();
                assert!(content.contains("Output line 1"));
                assert!(content.contains("Output line 5"));
                // Cleanup
                output_state.cleanup();
            }
            Err(e) => {
                panic!("Intentional test failure: failed to get inner lock: {}", e);
            }
        }
    }

    #[test]
    fn test_track_typed_set_breakpoints_request_and_response() {
        let tracker = test_tracker();

        let request_seq = Seq(50);
        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some("/test.rs".to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![SourceBreakpoint {
                    line: 10,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = Response {
            seq: 200.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![DapBreakpoint {
                    id: Some(1.into()),
                    verified: true,
                    line: Some(10),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(response));

        let breakpoints = tracker.get_breakpoints("/test.rs");
        assert_eq!(breakpoints.len(), 1);
        assert_eq!(breakpoints[0].line, 10);
        assert!(breakpoints[0].verified);
        assert_eq!(breakpoints[0].id, Some(1.into()));
    }

    #[test]
    fn test_track_typed_launch_request() {
        use dapper_dap_protocol::requests::LaunchRequestArguments;

        let tracker = test_tracker();

        let request = Request {
            seq: Seq(1),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let execution_state = tracker.get_execution_state();
        assert!(execution_state.is_all_running());

        match tracker.inner.lock() {
            Ok(inner) => {
                assert_eq!(inner.request_type, Some(RequestType::Launch));
            }
            Err(e) => {
                panic!("Intentional test failure: failed to get inner lock: {}", e);
            }
        }
    }

    #[test]
    fn test_track_typed_continue_and_stopped() {
        let tracker = test_tracker();

        let initial_stop = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(initial_stop));
        assert!(tracker.is_stopped());

        let continue_request = Request {
            seq: Seq(10),
            command: RequestCommand::Continue(ContinueArguments {
                thread_id: ThreadId(1),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(continue_request), ClientType::Main);

        let continue_response = Response {
            seq: 200.into(),
            request_seq: Seq(10),
            success: true,
            message: None,
            body: ResponseBody::Continue(ContinueResponseBody {
                all_threads_continued: Some(true),
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(continue_response));
        assert!(!tracker.is_stopped());

        let second_stop = Event {
            seq: 101.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Step,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(second_stop));
        assert!(tracker.is_stopped());
    }

    #[test]
    fn test_track_breakpoints_without_line_in_response() {
        let tracker = test_tracker();
        let source_path = "/some/path/to/main.php";
        let request_seq = Seq(42);

        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![
                    SourceBreakpoint {
                        line: 10,
                        ..Default::default()
                    },
                    SourceBreakpoint {
                        line: 20,
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = Response {
            seq: 100.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![
                    DapBreakpoint {
                        id: Some(1.into()),
                        verified: true,
                        line: None,
                        ..Default::default()
                    },
                    DapBreakpoint {
                        id: Some(2.into()),
                        verified: true,
                        line: None,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(response));

        let breakpoints = tracker.get_breakpoints(source_path);
        assert_eq!(breakpoints.len(), 2);
        assert_eq!(breakpoints[0].line, 10);
        assert!(breakpoints[0].verified);
        assert_eq!(breakpoints[0].id, Some(1.into()));
        assert_eq!(breakpoints[1].line, 20);
        assert!(breakpoints[1].verified);
        assert_eq!(breakpoints[1].id, Some(2.into()));
    }

    #[test]
    fn test_track_breakpoints_partial_line_in_response() {
        let tracker = test_tracker();
        let source_path = "/some/path/to/main.php";
        let request_seq = Seq(42);

        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![
                    SourceBreakpoint {
                        line: 10,
                        ..Default::default()
                    },
                    SourceBreakpoint {
                        line: 20,
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = Response {
            seq: 100.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![
                    DapBreakpoint {
                        id: Some(1.into()),
                        verified: true,
                        line: Some(10),
                        ..Default::default()
                    },
                    DapBreakpoint {
                        id: Some(2.into()),
                        verified: true,
                        line: None,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(response));

        let breakpoints = tracker.get_breakpoints(source_path);
        assert_eq!(breakpoints.len(), 2);
        assert_eq!(breakpoints[0].line, 10);
        assert_eq!(breakpoints[1].line, 20);
    }

    #[test]
    fn test_track_breakpoints_with_source_missing_line() {
        let tracker = test_tracker();
        let source_path = "/some/path/to/main.php";

        let requested_specs = vec![
            SourceBreakpoint {
                line: 30,
                condition: Some("x > 5".to_string()),
                ..Default::default()
            },
            SourceBreakpoint {
                line: 40,
                ..Default::default()
            },
        ];

        let response_breakpoints = vec![
            DapBreakpoint {
                id: Some(5.into()),
                verified: true,
                line: None,
                ..Default::default()
            },
            DapBreakpoint {
                id: Some(6.into()),
                verified: true,
                line: None,
                ..Default::default()
            },
        ];

        let resolved =
            breakpoint_state::breakpoints_with_fallback(&response_breakpoints, &requested_specs);

        let diff = tracker.track_breakpoints_with_source(source_path, source_path, resolved);
        assert_eq!(diff.to_add.len(), 2);
        assert_eq!(diff.to_remove.len(), 0);

        let breakpoints = tracker.get_breakpoints(source_path);
        assert_eq!(breakpoints.len(), 2);
        assert_eq!(breakpoints[0].line, 30);
        assert_eq!(breakpoints[0].condition, Some("x > 5".to_string()));
        assert_eq!(breakpoints[1].line, 40);
    }

    #[test]
    fn test_track_breakpoint_changed_event() {
        let tracker = test_tracker();
        let source_path = "/some/path/to/main.cpp";
        let request_seq = Seq(42);

        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![SourceBreakpoint {
                    line: 10,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = Response {
            seq: 100.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![DapBreakpoint {
                    id: Some(1.into()),
                    verified: false,
                    line: Some(10),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(response));

        let bps = tracker.get_breakpoints(source_path);
        assert_eq!(bps.len(), 1);
        assert!(!bps[0].verified);

        let bp_event = Event {
            seq: 200.into(),
            event: EventKind::Breakpoint(BreakpointEventBody {
                reason: BreakpointEventReason::Changed,
                breakpoint: DapBreakpoint {
                    id: Some(1.into()),
                    verified: true,
                    line: Some(12),
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(bp_event));

        let bps = tracker.get_breakpoints(source_path);
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 12);
        assert!(bps[0].verified);
    }

    #[test]
    fn test_deferred_breakpoint_resolution() {
        let tracker = test_tracker();
        let source_path = "/some/path/to/main.php";
        let request_seq = Seq(42);

        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![SourceBreakpoint {
                    line: 10,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = Response {
            seq: 100.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![DapBreakpoint {
                    id: Some(1.into()),
                    verified: true,
                    line: None,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(response));

        let bps = tracker.get_breakpoints(source_path);
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 10, "should fall back to requested line");

        let bp_event = Event {
            seq: 200.into(),
            event: EventKind::Breakpoint(BreakpointEventBody {
                reason: BreakpointEventReason::Changed,
                breakpoint: DapBreakpoint {
                    id: Some(1.into()),
                    verified: true,
                    line: Some(12),
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(bp_event));

        let bps = tracker.get_breakpoints(source_path);
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 12, "should update to resolved line");
        assert!(bps[0].verified);
    }

    #[test]
    fn test_deferred_resolution_updates_context_lines() {
        let tracker = test_tracker();
        let source_path = "/some/path/to/main.php";
        let request_seq = Seq(42);

        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(source_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![
                    SourceBreakpoint {
                        line: 10,
                        ..Default::default()
                    },
                    SourceBreakpoint {
                        line: 30,
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = Response {
            seq: 100.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![
                    DapBreakpoint {
                        id: Some(1.into()),
                        verified: true,
                        line: None,
                        ..Default::default()
                    },
                    DapBreakpoint {
                        id: Some(2.into()),
                        verified: true,
                        line: None,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(response));

        let bps = tracker.get_breakpoints(source_path);
        assert_eq!(bps[0].line, 10);
        assert_eq!(bps[1].line, 30);

        let bp_event_1 = Event {
            seq: 200.into(),
            event: EventKind::Breakpoint(BreakpointEventBody {
                reason: BreakpointEventReason::Changed,
                breakpoint: DapBreakpoint {
                    id: Some(1.into()),
                    verified: true,
                    line: Some(12),
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(bp_event_1));

        let bp_event_2 = Event {
            seq: 201.into(),
            event: EventKind::Breakpoint(BreakpointEventBody {
                reason: BreakpointEventReason::Changed,
                breakpoint: DapBreakpoint {
                    id: Some(2.into()),
                    verified: true,
                    line: Some(32),
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(bp_event_2));

        let bps = tracker.get_breakpoints(source_path);
        assert_eq!(bps.len(), 2);
        assert_eq!(
            bps[0].line, 12,
            "first breakpoint should be at resolved line"
        );
        assert_eq!(
            bps[1].line, 32,
            "second breakpoint should be at resolved line"
        );
    }

    #[test]
    fn test_breakpoint_event_moves_source_path() {
        let tracker = test_tracker();
        let original_path = "/some/path/to/main.php";
        let resolved_path = "/canonical/path/to/main.php";
        let request_seq = Seq(42);

        let request = Request {
            seq: request_seq,
            command: RequestCommand::SetBreakpoints(SetBreakpointsArguments {
                source: DapSource {
                    path: Some(original_path.to_string()),
                    ..Default::default()
                },
                breakpoints: Some(vec![SourceBreakpoint {
                    line: 10,
                    ..Default::default()
                }]),
                ..Default::default()
            }),
        };
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = Response {
            seq: 100.into(),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::SetBreakpoints(SetBreakpointsResponseBody {
                breakpoints: vec![DapBreakpoint {
                    id: Some(1.into()),
                    verified: true,
                    line: Some(10),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Response(response));

        assert_eq!(tracker.get_breakpoints(original_path).len(), 1);

        let bp_event = Event {
            seq: 200.into(),
            event: EventKind::Breakpoint(BreakpointEventBody {
                reason: BreakpointEventReason::Changed,
                breakpoint: DapBreakpoint {
                    id: Some(1.into()),
                    verified: true,
                    line: Some(12),
                    source: Some(DapSource {
                        path: Some(resolved_path.to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(bp_event));

        assert!(tracker.get_breakpoints(original_path).is_empty());
        let bps = tracker.get_breakpoints(resolved_path);
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 12);
        assert_eq!(bps[0].id, Some(1.into()));
    }

    #[test]
    fn test_track_breakpoints_with_source_alias_fallback() {
        let tracker = test_tracker();
        let original_path = "/home/user/main.py";
        let resolved_path = "/canonical/main.py";

        let breakpoints = vec![BreakpointInfo {
            line: 10,
            verified: true,
            id: Some(1.into()),
            condition: None,
            log_message: None,
            ..Default::default()
        }];

        tracker.track_breakpoints_with_source(original_path, resolved_path, breakpoints);

        let bps_by_resolved = tracker.get_breakpoints(resolved_path);
        assert_eq!(bps_by_resolved.len(), 1);
        assert_eq!(bps_by_resolved[0].line, 10);

        let bps_by_original = tracker.get_breakpoints(original_path);
        assert_eq!(
            bps_by_original.len(),
            1,
            "alias fallback should find breakpoints via original path"
        );
        assert_eq!(bps_by_original[0].line, 10);
    }

    /// Drive a synthetic Initialize response through the tracker and return the
    /// `supports_step_back` value the tracker captured.
    fn capture_step_back_after_initialize(caps: Option<Capabilities>) -> Option<Option<bool>> {
        let tracker = test_tracker();
        let response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Initialize(caps),
        };
        tracker.track_message_to_client(&Message::Response(response));
        tracker.adapter_capabilities().map(|c| c.supports_step_back)
    }

    #[test]
    fn captures_supports_step_back_true() {
        let caps = Capabilities {
            supports_step_back: Some(true),
            ..Default::default()
        };
        assert_eq!(
            capture_step_back_after_initialize(Some(caps)),
            Some(Some(true)),
        );
    }

    #[test]
    fn captures_supports_step_back_false() {
        let caps = Capabilities {
            supports_step_back: Some(false),
            ..Default::default()
        };
        assert_eq!(
            capture_step_back_after_initialize(Some(caps)),
            Some(Some(false)),
        );
    }

    #[test]
    fn captures_supports_step_back_absent() {
        // Field omitted: tracker stores Capabilities but the option is None.
        assert_eq!(
            capture_step_back_after_initialize(Some(Capabilities::default())),
            Some(None),
        );
    }

    #[test]
    fn capabilities_none_when_initialize_body_is_none() {
        // Adapter responded to initialize without a body; tracker stores None,
        // so adapter_capabilities() returns None. Mirrors the `Initialize(caps)`
        // arm in `DebugSessionTracker::track_message_to_client`, which clones
        // the Option<Capabilities> verbatim.
        assert_eq!(capture_step_back_after_initialize(None), None);
    }

    #[test]
    fn capabilities_none_when_initialize_never_received() {
        let tracker = test_tracker();
        assert!(tracker.adapter_capabilities().is_none());
    }

    #[test]
    fn does_not_capture_capabilities_when_initialize_failed() {
        // The tracker's `track_message_to_client` only stores capabilities for
        // successful Initialize responses. Lock that in so a future change to
        // the success-gating doesn't silently let a half-initialized session
        // pass the reverse-debugging gate downstream.
        let tracker = test_tracker();
        let response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: false,
            message: Some("init failed".into()),
            body: ResponseBody::Initialize(Some(Capabilities {
                supports_step_back: Some(true),
                ..Default::default()
            })),
        };
        tracker.track_message_to_client(&Message::Response(response));
        assert!(tracker.adapter_capabilities().is_none());
    }

    #[test]
    fn capabilities_event_overlays_and_preserves_baseline() {
        let tracker = test_tracker();

        // Baseline from `initialize`: single-thread supported, stepBack unset.
        let init = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Initialize(Some(Capabilities {
                supports_single_thread_execution_requests: Some(true),
                ..Default::default()
            })),
        };
        tracker.track_message_to_client(&Message::Response(init));

        // A later `capabilities` event advertises stepBack (a partial delta).
        let event = Event {
            seq: 2.into(),
            event: EventKind::Capabilities(CapabilitiesEventBody {
                capabilities: Capabilities {
                    supports_step_back: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            }),
        };
        tracker.track_message_to_client(&Message::Event(event));

        let caps = tracker
            .adapter_capabilities()
            .expect("capabilities should be present after initialize");
        assert_eq!(
            caps.supports_step_back,
            Some(true),
            "the capabilities event should advertise stepBack post-initialize"
        );
        assert_eq!(
            caps.supports_single_thread_execution_requests,
            Some(true),
            "the initialize baseline must survive the capabilities merge"
        );
    }

    fn make_set_exception_breakpoints_request(seq: Seq, filters: Vec<&str>) -> Request {
        use dapper_dap_protocol::requests::SetExceptionBreakpointsArguments;
        Request {
            seq,
            command: RequestCommand::SetExceptionBreakpoints(SetExceptionBreakpointsArguments {
                filters: filters.into_iter().map(String::from).collect(),
                ..Default::default()
            }),
        }
    }

    fn make_set_exception_breakpoints_response(
        seq: Seq,
        request_seq: Seq,
        success: bool,
    ) -> Response {
        Response {
            seq,
            request_seq,
            success,
            message: if success {
                None
            } else {
                Some("rejected".into())
            },
            body: ResponseBody::SetExceptionBreakpoints(None),
        }
    }

    #[test]
    fn test_track_set_exception_breakpoints_request_response() {
        // IDE-driven round-trip: request → success response → installed
        // matches the request entries, sorted by filter id.
        let tracker = test_tracker();
        let request = make_set_exception_breakpoints_request(Seq(7), vec!["uncaught", "raised"]);
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = make_set_exception_breakpoints_response(20.into(), Seq(7), true);
        tracker.track_message_to_client(&Message::Response(response));

        let installed = tracker.get_installed_exception_filters();
        let ids: Vec<&str> = installed.iter().map(|e| e.filter.as_str()).collect();
        assert_eq!(ids, vec!["raised", "uncaught"]);
    }

    #[test]
    fn test_secondary_client_request_not_tracked_in_pending() {
        // Secondary clients (control-plane MCP/CLI) shouldn't add pending
        // entries via `track_message_from_client` because their responses
        // don't reach `track_message_to_client`. They use the explicit
        // `update_exception_filters` setter instead.
        let tracker = test_tracker();
        let request = make_set_exception_breakpoints_request(Seq(7), vec!["uncaught"]);
        tracker.track_message_from_client(&Message::Request(request), ClientType::Secondary);

        // A subsequent successful response should NOT commit anything,
        // because no pending entry was recorded.
        let response = make_set_exception_breakpoints_response(20.into(), Seq(7), true);
        tracker.track_message_to_client(&Message::Response(response));

        assert!(tracker.get_installed_exception_filters().is_empty());
    }

    #[test]
    fn test_explicit_update_exception_filters_replaces() {
        // Control-plane setter path: explicit replace bypasses the
        // pending-request bookkeeping and updates `installed` directly.
        let tracker = test_tracker();
        tracker.update_exception_filters(vec![
            ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            },
            ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: Some("x>5".to_string()),
            },
        ]);
        let installed = tracker.get_installed_exception_filters();
        let ids: Vec<&str> = installed.iter().map(|e| e.filter.as_str()).collect();
        assert_eq!(ids, vec!["raised", "uncaught"]);
        assert_eq!(installed[0].condition.as_deref(), Some("x>5"));
    }

    #[test]
    fn test_failed_response_pops_pending_without_replacing() {
        // Pending entry recorded → failure response arrives → installed
        // unchanged, pending cleared.
        let tracker = test_tracker();
        // Pre-seed installed with a value via the explicit setter so we
        // can verify it's *not* replaced by the failed response.
        tracker.update_exception_filters(vec![ExceptionFilterEntry {
            filter: "old".to_string(),
            condition: None,
        }]);

        let request = make_set_exception_breakpoints_request(Seq(7), vec!["new"]);
        tracker.track_message_from_client(&Message::Request(request), ClientType::Main);

        let response = make_set_exception_breakpoints_response(20.into(), Seq(7), false);
        tracker.track_message_to_client(&Message::Response(response));

        let installed = tracker.get_installed_exception_filters();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].filter, "old");
    }

    #[test]
    fn test_response_context_includes_exception_filters() {
        let tracker = test_tracker();
        tracker.update_exception_filters(vec![ExceptionFilterEntry {
            filter: "uncaught".to_string(),
            condition: None,
        }]);

        let ctx = tracker
            .response_context()
            .expect("response_context should be Some");
        assert_eq!(ctx.installed_exception_filters.len(), 1);
        assert_eq!(ctx.installed_exception_filters[0].filter, "uncaught");
    }

    #[test]
    fn test_parent_session_id_threaded_into_finalized_session_info() {
        use dapper_dap_protocol::requests::LaunchRequestArguments;
        use dapper_session::Port;

        // `try_finalize_session` produces the `SessionInfo` only once both the
        // control-plane port and a launch/attach request are seen. Drive both,
        // return the finalized info, and clean up the written session file.
        fn finalize(tracker: &DebugSessionTracker) -> SessionInfo {
            tracker.track_message_from_client(
                &Message::Request(Request {
                    seq: Seq(1),
                    command: RequestCommand::Launch(LaunchRequestArguments::default()),
                }),
                ClientType::Main,
            );
            tracker.register_control_plane(Port::try_new(12345), Some("test-scope".into()));
            let info = tracker
                .get_session_info()
                .expect("session should be finalized");
            let _ = test_store().delete(&info);
            info
        }

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        // Child proxy: the parent id threads all the way into the SessionInfo.
        let child = test_tracker_with_id(format!("child-{unique}").into())
            .with_parent_session_id(Some("the-parent".into()));
        assert_eq!(
            finalize(&child).parent_session_id,
            Some("the-parent".into()),
            "child tracker must stamp parent_session_id into the finalized SessionInfo"
        );

        // Root proxy: no parent id is recorded.
        let root = test_tracker_with_id(format!("root-{unique}").into());
        assert_eq!(
            finalize(&root).parent_session_id,
            None,
            "root tracker must leave parent_session_id unset"
        );
    }
}
