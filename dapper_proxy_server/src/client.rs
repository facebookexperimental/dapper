// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use dapper_config::DapperConfig;
use dapper_control_api::ExceptionFilterEntry;
use dapper_control_api::NavigateResult;
use dapper_control_api::NavigationType;
use dapper_control_api::RawDapResult;
use dapper_control_api::SetExceptionBreakpointsResult;
use dapper_control_api::WaitedEvent;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::data_types::Source as DapSource;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_dap_protocol::enums::BreakpointEventReason;
use dapper_dap_protocol::enums::EvaluateContext;
use dapper_dap_protocol::events::BreakpointEventBody;
use dapper_dap_protocol::events::EventKind;
use dapper_dap_protocol::protocol as dap;
use dapper_dap_protocol::requests::EvaluateArguments;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::requests::ScopesArguments;
use dapper_dap_protocol::requests::SetBreakpointsArguments;
use dapper_dap_protocol::requests::SetVariableArguments;
use dapper_dap_protocol::requests::StackTraceArguments;
use dapper_dap_protocol::requests::UnknownCommand;
use dapper_dap_protocol::requests::VariablesArguments;
use dapper_dap_protocol::responses::ResponseBody;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::dapper_event::DapperEvent;
use crate::debug_session_tracker::BreakpointInfo;
use crate::debug_session_tracker::DebugSessionTracker;
use crate::session_init::build_set_exception_breakpoints_request;

/// A wrapper around an mpsc sender that only allows sending DAP events
#[derive(Clone)]
pub struct EventChannel {
    sender: mpsc::UnboundedSender<dap::Message>,
}

impl EventChannel {
    /// Create a new EventChannel pair (sender wrapped in EventChannel, receiver for merged task)
    pub fn new_pair() -> (Self, mpsc::UnboundedReceiver<dap::Message>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (EventChannel { sender: tx }, rx)
    }

    /// Send an event to the proxy server
    pub fn send_event(&self, event_kind: EventKind) -> anyhow::Result<()> {
        let event = dap::Event::new(event_kind);

        self.sender
            .send(event.into())
            .context("Failed to send event to proxy server")?;
        Ok(())
    }
}

pub struct Request<C, R> {
    pub(crate) client_id: ClientId,
    pub(crate) command: C,
    pub(crate) result: oneshot::Sender<R>,
}

/// A unique identifier for a client connection. This identifier is used for
/// message routing and sequence number remapping.
///
/// NOTE: use textual ID for easier debugging.
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct ClientId(Arc<str>);

impl ClientId {
    pub fn new(id: &str) -> Self {
        Self(Arc::from(id))
    }
}

pub struct Client<C, R> {
    id: ClientId,
    to_server: mpsc::UnboundedSender<Request<C, R>>,
    event_channel: EventChannel,
    debug_session_tracker: DebugSessionTracker,
    config: DapperConfig,
}

impl<C, R> Clone for Client<C, R> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            to_server: self.to_server.clone(),
            event_channel: self.event_channel.clone(),
            debug_session_tracker: self.debug_session_tracker.clone(),
            config: self.config.clone(),
        }
    }
}

impl<C, R> Client<C, R>
where
    C: Send + Sync + 'static,
    R: Send + 'static,
{
    pub fn new(
        id: ClientId,
        to_server: mpsc::UnboundedSender<Request<C, R>>,
        event_channel: EventChannel,
        debug_session_tracker: DebugSessionTracker,
        config: DapperConfig,
    ) -> Self {
        Self {
            id,
            to_server,
            event_channel,
            debug_session_tracker,
            config,
        }
    }

    pub async fn send(&self, command: C) -> anyhow::Result<R> {
        let (result_sender, result_receiver) = oneshot::channel();
        let command_handle = Request {
            client_id: self.id.clone(),
            command,
            result: result_sender,
        };
        self.to_server
            .send(command_handle)
            .context("Failed to send message from client to the server")?;

        let result = result_receiver
            .await
            .context("Failed to receive a result in client receiver's oneshot")?;
        Ok(result)
    }

    pub fn client_id(&self) -> &ClientId {
        &self.id
    }

    pub fn debug_session_tracker(&self) -> &DebugSessionTracker {
        &self.debug_session_tracker
    }

    pub fn config(&self) -> &DapperConfig {
        &self.config
    }

    #[cfg(test)]
    pub(crate) fn event_channel(&self) -> &EventChannel {
        &self.event_channel
    }
}

pub type ProxyRequest = Request<Command, CommandResult>;
pub type ProxyClient = Client<Command, CommandResult>;

impl ProxyClient {
    pub async fn status(&self) -> anyhow::Result<()> {
        let result = self.send(Command::Control(ControlCommand::Status)).await?;
        match result {
            CommandResult::Control(ControlResult::Status) => Ok(()),
            _ => anyhow::bail!("Unexpected result type"),
        }
    }

    pub async fn send_message(&self, message: dap::Message) -> anyhow::Result<ListenerPayload> {
        let result = self.send(Command::Debugger(message)).await?;
        match result {
            CommandResult::Debugger(payload) => Ok(payload),
            _ => anyhow::bail!("Unexpected result type"),
        }
    }

    pub async fn send_message_with_timeout(
        &self,
        message: dap::Message,
        timeout: std::time::Duration,
    ) -> anyhow::Result<ListenerPayload> {
        match tokio::time::timeout(timeout, self.send_message(message)).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!(
                "send_message timed out after {:?}",
                timeout,
            )),
        }
    }

    pub async fn send_message_and_forget(&self, message: dap::Message) -> anyhow::Result<Seq> {
        let ListenerPayload { seq, .. } = self.send_message(message).await?;
        Ok(seq)
    }

    pub async fn repl(&self, cmd: String, frame_id: Option<FrameId>) -> anyhow::Result<String> {
        let request = dap::Request::new(RequestCommand::Evaluate(EvaluateArguments {
            expression: cmd,
            frame_id,
            context: Some(EvaluateContext::Repl),
            ..Default::default()
        }));

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        // Extract result from the response body
        let result = match response.body {
            ResponseBody::Evaluate(body) => body.result,
            _ => "No result returned".to_owned(),
        };

        Ok(result)
    }

    pub async fn threads(&self) -> anyhow::Result<dapper_control_api::ThreadsResult> {
        let request = dap::Request::new(RequestCommand::Threads);

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        let threads = match response.body {
            ResponseBody::Threads(body) => body.threads,
            _ => Vec::new(),
        };

        let stack_trace = if self.config.threads.show_stacktrace
            && threads.len() <= self.config.threads.expand_stacktrace_threshold
            && let Some(first_thread) = threads.first()
        {
            self.stack_trace(first_thread.id, None, None).await.ok()
        } else {
            None
        };

        Ok(dapper_control_api::ThreadsResult {
            threads,
            stack_trace,
            ..Default::default()
        })
    }

    pub async fn stack_trace(
        &self,
        thread_id: ThreadId,
        start_frame: Option<i64>,
        levels: Option<i64>,
    ) -> anyhow::Result<dapper_control_api::StackTraceResult> {
        let effective_start_frame = start_frame.unwrap_or(0);
        let effective_levels = levels.unwrap_or(self.config.stack_trace.max_frames as i64);
        let frames_to_request = if effective_levels > 0 {
            effective_levels + 1
        } else {
            0
        };

        let request = dap::Request::new(RequestCommand::StackTrace(StackTraceArguments {
            thread_id,
            levels: Some(frames_to_request),
            start_frame: Some(effective_start_frame),
            ..Default::default()
        }));

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        let all_frames = match response.body {
            ResponseBody::StackTrace(body) => body.stack_frames,
            _ => Vec::new(),
        };

        let frames_to_show = if frames_to_request > 0 {
            all_frames.len().min(effective_levels as usize)
        } else {
            all_frames.len()
        };
        let has_more_frames = all_frames.len() > frames_to_show;
        let stack_frames: Vec<_> = all_frames.into_iter().take(frames_to_show).collect();

        let scopes = if effective_start_frame == 0
            && self.config.stack_trace.expand_scopes
            && let Some(first_frame) = stack_frames.first()
        {
            self.scopes(first_frame.id).await.ok()
        } else {
            None
        };

        Ok(dapper_control_api::StackTraceResult {
            stack_frames,
            start_frame: effective_start_frame,
            has_more_frames,
            scopes,
            thread_id,
            ..Default::default()
        })
    }

    pub async fn scopes(
        &self,
        frame_id: FrameId,
    ) -> anyhow::Result<dapper_control_api::ScopesResult> {
        let request = dap::Request::new(RequestCommand::Scopes(ScopesArguments {
            frame_id,
            ..Default::default()
        }));

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        let scopes = match response.body {
            ResponseBody::Scopes(body) => body.scopes,
            _ => Vec::new(),
        };

        let locals = if self.config.scopes.expand_locals {
            let locals_scope = scopes
                .iter()
                .find(|s| s.is_locals() && s.variables_reference.has_children());

            match locals_scope {
                Some(scope) => self
                    .variables(scope.variables_reference)
                    .await
                    .ok()
                    .map(|r| r.variables),
                None => None,
            }
        } else {
            None
        };

        Ok(dapper_control_api::ScopesResult {
            scopes,
            locals,
            frame_id,
            ..Default::default()
        })
    }

    pub async fn variables(
        &self,
        variables_reference: VariablesReference,
    ) -> anyhow::Result<dapper_control_api::VariablesResult> {
        let request = dap::Request::new(RequestCommand::Variables(VariablesArguments {
            variables_reference,
            ..Default::default()
        }));

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        let variables = match response.body {
            ResponseBody::Variables(body) => body.variables,
            _ => Vec::new(),
        };

        Ok(dapper_control_api::VariablesResult {
            variables,
            variables_reference,
            ..Default::default()
        })
    }

    pub async fn set_variable(
        &self,
        variables_reference: VariablesReference,
        name: &str,
        value: &str,
    ) -> anyhow::Result<dapper_control_api::SetVariableResult> {
        let request = dap::Request::new(RequestCommand::SetVariable(SetVariableArguments {
            variables_reference,
            name: name.to_owned(),
            value: value.to_owned(),
            ..Default::default()
        }));

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        let body = match response.body {
            ResponseBody::SetVariable(body) => body,
            _ => return Err(anyhow::anyhow!("Unexpected response body for setVariable")),
        };

        Ok(dapper_control_api::SetVariableResult {
            body,
            name: name.to_string(),
            ..Default::default()
        })
    }

    pub async fn navigate(
        &self,
        navigation_type: NavigationType,
        thread_id: ThreadId,
        single_thread: Option<bool>,
    ) -> anyhow::Result<dapper_control_api::NavigationResult> {
        tracing::debug!(
            navigation_type = %navigation_type,
            thread_id = thread_id.as_i64(),
            command_name = navigation_type.command_name(),
            "Executing navigation command"
        );

        // Reverse-debugging requests are only meaningful when the connected
        // adapter advertises Capabilities.supportsStepBack. Reject early so
        // both MCP and gRPC consumers get a structured error instead of a
        // cryptic adapter-side failure.
        if matches!(
            navigation_type,
            NavigationType::StepBack | NavigationType::ReverseContinue
        ) {
            // Error messages render `navigation_type` directly (snake_case via
            // strum::Display) so they match what users invoked from CLI / MCP
            // / control plane. The two arms below intentionally treat
            // supports_step_back == None and Some(false) the same way: per
            // the DAP spec, missing capability bits default to false, so
            // both signal "adapter does not advertise the capability".
            match self.debug_session_tracker.adapter_capabilities() {
                None => anyhow::bail!(
                    "capabilities unknown (initialize not yet received); cannot execute {}",
                    navigation_type
                ),
                Some(caps) if caps.supports_step_back != Some(true) => anyhow::bail!(
                    "adapter does not advertise the DAP `supportsStepBack` capability; cannot execute {}",
                    navigation_type
                ),
                _ => {}
            }
        }

        // Gate singleThread on the adapter advertising
        // supportsSingleThreadExecutionRequests.
        let effective_single_thread = if single_thread == Some(true) {
            match self.debug_session_tracker.adapter_capabilities() {
                Some(caps) if caps.supports_single_thread_execution_requests == Some(true) => {
                    Some(true)
                }
                _ => anyhow::bail!(
                    "adapter does not advertise the DAP `supportsSingleThreadExecutionRequests` capability; \
                     cannot use single_thread for {}",
                    navigation_type
                ),
            }
        } else {
            None
        };

        let request = dap::Request::new(
            navigation_type.to_request_command(thread_id, effective_single_thread),
        );

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        if !matches!(
            navigation_type,
            NavigationType::Continue | NavigationType::ReverseContinue | NavigationType::Pause
        ) {
            return Ok(dapper_control_api::NavigationResult {
                result: NavigateResult::CommandExecuted,
                navigation_type,
                extra: Default::default(),
            });
        }

        let (timeout, timeout_seconds) = match navigation_type {
            NavigationType::Pause => (
                self.config.navigate.pause_timeout(),
                self.config.navigate.pause_timeout_seconds,
            ),
            // ReverseContinue reuses the forward continue timeout: both run
            // until a breakpoint or terminal condition (end-of-recording vs.
            // exit), so the wait shape is the same. Split into a dedicated
            // config knob if reverse recordings need significantly different
            // bounds in practice.
            NavigationType::Continue | NavigationType::ReverseContinue => (
                self.config.navigate.continue_timeout(),
                self.config.navigate.continue_timeout_seconds,
            ),
            _ => unreachable!(),
        };

        let result = match helpers::wait_for_events_timeout(
            |e| {
                matches!(
                    e,
                    EventKind::Stopped(_) | EventKind::Exited(_) | EventKind::Terminated(_)
                )
            },
            &mut messages,
            timeout,
        )
        .await
        {
            Ok(event) => match event.event {
                EventKind::Stopped(stopped) => NavigateResult::Stopped(stopped),
                EventKind::Exited(exited) => NavigateResult::Exited(exited),
                EventKind::Terminated(_) => NavigateResult::Terminated,
                _ => NavigateResult::CommandExecuted,
            },
            Err(_) => NavigateResult::TimedOut { timeout_seconds },
        };

        Ok(dapper_control_api::NavigationResult {
            result,
            navigation_type,
            extra: Default::default(),
        })
    }

    /// Set breakpoints in a source file
    ///
    /// When clear_existing is false (default), new breakpoints are appended to existing ones
    /// When clear_existing is true, all existing breakpoints in the file are removed before adding new ones
    pub async fn set_breakpoints(
        &self,
        source_path: &str,
        clear_existing: bool,
        breakpoint_specs: &[SourceBreakpoint],
    ) -> anyhow::Result<dapper_control_api::SetBreakpointsResult> {
        // Get existing breakpoints to track what's being removed
        let existing_breakpoints = self.debug_session_tracker.get_breakpoints(source_path);
        let existing_lines: Vec<i64> = existing_breakpoints.iter().map(|bp| bp.line).collect();

        tracing::debug!(
            source_path = %source_path,
            existing_breakpoint_count = existing_breakpoints.len(),
            existing_lines = ?existing_lines,
            "Retrieved existing breakpoints in set_breakpoints"
        );

        // Determine which specs to set based on clear_existing flag
        let merged_specs = if clear_existing {
            let mut specs = breakpoint_specs.to_vec();
            specs.sort_by_key(|s| s.line);
            specs
        } else {
            // Start with existing breakpoints (which include conditions) as specs
            let mut specs: Vec<SourceBreakpoint> = existing_breakpoints
                .iter()
                .map(SourceBreakpoint::from)
                .collect();
            // Merge new specs: replace existing on same line, append otherwise
            for new_spec in breakpoint_specs {
                if let Some(existing) = specs.iter_mut().find(|s| s.line == new_spec.line) {
                    *existing = new_spec.clone();
                } else {
                    specs.push(new_spec.clone());
                }
            }
            specs.sort_by_key(|s| s.line);

            tracing::debug!(
                combined_count = specs.len(),
                existing_count = existing_breakpoints.len(),
                new_count = breakpoint_specs.len(),
                "Combined existing and new breakpoint specs"
            );

            specs
        };

        // Extract the filename from the path for the name field
        let source_name = Path::new(source_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(source_path);

        let request = dap::Request::new(RequestCommand::SetBreakpoints(SetBreakpointsArguments {
            source: DapSource {
                name: Some(source_name.to_string()),
                path: Some(source_path.to_string()),
                ..Default::default()
            },
            breakpoints: Some(merged_specs.clone()),
            lines: Some(merged_specs.iter().map(|s| s.line).collect()),
            ..Default::default()
        }));

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;

        let response = helpers::wait_for_response(seq, &mut messages).await?;

        response.check_success()?;

        let ResponseBody::SetBreakpoints(bp_body) = &response.body else {
            let error_msg = format!(
                "Response body does not contain expected setBreakpoints data.\nRaw JSON response:\n{}",
                serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|_| "Failed to serialize response".to_string())
            );
            return Err(anyhow::anyhow!(error_msg));
        };

        let resolved_breakpoints = crate::debug_session_tracker::breakpoints_with_fallback(
            &bp_body.breakpoints,
            &merged_specs,
        );

        let effective_source_path =
            crate::debug_session_tracker::resolved_source_path(&bp_body.breakpoints)
                .unwrap_or(source_path);

        let effective_source_name = Path::new(effective_source_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(effective_source_path);

        let new_count = breakpoint_specs.len();
        let existing_count = merged_specs.len().saturating_sub(new_count);

        let breakpoint_diff = self.debug_session_tracker.track_breakpoints_with_source(
            source_path,
            effective_source_path,
            resolved_breakpoints.clone(),
        );

        // Send events to client based on the diff
        if let Some(diff) = breakpoint_diff {
            // Send "removed" events for breakpoints to remove
            if !diff.to_remove.is_empty() {
                self.send_breakpoint_events(
                    BreakpointEventReason::Removed,
                    effective_source_path,
                    effective_source_name,
                    &diff.to_remove,
                )
                .await;
            }
            // Send "new" events for breakpoints to add
            if !diff.to_add.is_empty() {
                self.send_breakpoint_events(
                    BreakpointEventReason::New,
                    effective_source_path,
                    effective_source_name,
                    &diff.to_add,
                )
                .await;
            }
        }

        Ok(dapper_control_api::SetBreakpointsResult {
            breakpoints: resolved_breakpoints,
            source_path: effective_source_path.to_owned(),
            new_count,
            existing_count,
            ..Default::default()
        })
    }

    /// Set the active exception breakpoint filters at the debug adapter.
    /// See the trait doc on `DapperControlPlane::set_exception_breakpoints`
    /// for empty-input + merge semantics.
    ///
    /// `new_count` and `existing_count` in the returned result are
    /// computed against the requested-merge view (pre-builder), not the
    /// post-sanitization `installed` set. For the v1 surface (no
    /// caller-supplied conditions) the two views are equivalent because
    /// the builder never drops entire entries — only conditions when the
    /// adapter doesn't support them.
    pub async fn set_exception_breakpoints(
        &self,
        filters: &[String],
        clear_existing: bool,
    ) -> anyhow::Result<SetExceptionBreakpointsResult> {
        // Permissive empty-input early return — the no-op path must succeed
        // even if capabilities are unknown or the adapter advertises no
        // filters. Strict rejection of `empty + !clear` lives in the MCP
        // and CLI layers so user mistakes get caught before reaching this
        // method, but programmatic callers can rely on no-op semantics.
        if filters.is_empty() && !clear_existing {
            let installed = self.debug_session_tracker.get_installed_exception_filters();
            return Ok(SetExceptionBreakpointsResult {
                installed,
                new_count: 0,
                existing_count: 0,
                ..Default::default()
            });
        }

        // Capability gate: require the adapter to have advertised at least
        // one filter. Returns a structured error to the caller.
        let caps = self
            .debug_session_tracker
            .adapter_capabilities()
            .context(
                "adapter capabilities unknown (initialize not yet received); cannot set exception breakpoints",
            )?;
        let advertised = caps.exception_breakpoint_filters.as_deref().unwrap_or(&[]);
        if advertised.is_empty() {
            // `clear_existing=true` against a no-filter adapter is a no-op
            // semantically (nothing to clear), so short-circuit instead of
            // erroring. The non-empty-`filters` case still bails because
            // the user is asking us to install something the adapter
            // doesn't expose. (At this point `filters.is_empty()` implies
            // `clear_existing == true` because the empty + !clear path
            // already early-returned above.)
            if filters.is_empty() {
                // Defensive: tracker should already be empty since you
                // can't install filters the adapter doesn't advertise,
                // but make the post-condition explicit so any external
                // mutation gets reset.
                self.debug_session_tracker
                    .update_exception_filters(Vec::new());
                return Ok(SetExceptionBreakpointsResult::default());
            }
            anyhow::bail!(
                "adapter advertises no exception breakpoint filters; cannot set exception breakpoints"
            );
        }

        // Materialize the advertised set into a HashSet once so the
        // unknown-id check and the existing-vs-new accounting below avoid
        // repeated O(advertised) scans.
        let advertised_ids: HashSet<&str> = advertised.iter().map(|f| f.filter.as_str()).collect();

        // Validate every requested filter id against the advertised set
        // before sending any DAP request. Collect all unknowns (deduped
        // and sorted for stable error output) so a caller with multiple
        // typos sees the full list at once.
        let unknown_set: std::collections::BTreeSet<&str> = filters
            .iter()
            .filter(|f| !advertised_ids.contains(f.as_str()))
            .map(|f| f.as_str())
            .collect();
        if !unknown_set.is_empty() {
            let unknown: Vec<&str> = unknown_set.into_iter().collect();
            let mut valid: Vec<&str> = advertised_ids.iter().copied().collect();
            valid.sort_unstable();
            anyhow::bail!(
                "unknown exception breakpoint filter(s) {:?}; valid ids: {:?}",
                unknown,
                valid,
            );
        }

        let installed = self.debug_session_tracker.get_installed_exception_filters();
        let MergedExceptionFilters {
            merged: merged_vec,
            new_count,
            existing_count,
        } = merge_exception_filters(installed, filters, clear_existing);

        // Build the DAP request via the shared partition+gating builder.
        // `effective` reflects the post-sanitization set (with any
        // unsupported conditions dropped) — that's what we record in the
        // tracker, not the desired-merge view.
        let (request, effective) =
            build_set_exception_breakpoints_request(&merged_vec, Some(&caps));

        let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
        let response = helpers::wait_for_response(seq, &mut messages).await?;
        response.check_success()?;

        self.debug_session_tracker
            .update_exception_filters(effective.clone());

        Ok(SetExceptionBreakpointsResult {
            installed: effective,
            new_count,
            existing_count,
            ..Default::default()
        })
    }

    const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 60;

    pub async fn send_raw_dap_request(
        &self,
        command: &str,
        arguments: Option<serde_json::Value>,
        wait_for_event: bool,
        timeout_seconds: u64,
    ) -> anyhow::Result<dapper_control_api::RawDapResult> {
        let timeout = std::time::Duration::from_secs(if timeout_seconds > 0 {
            timeout_seconds
        } else {
            Self::DEFAULT_REQUEST_TIMEOUT_SECS
        });

        let request = dap::Request::new(RequestCommand::Unknown(UnknownCommand {
            command: command.to_owned(),
            arguments,
            extra: Default::default(),
        }));

        let round_trip = async {
            let ListenerPayload { seq, mut messages } = self.send_message(request.into()).await?;
            let response = helpers::wait_for_response(seq, &mut messages).await?;
            response.check_success()?;
            anyhow::Ok((response.body, messages))
        };
        let (body, mut messages) =
            tokio::time::timeout(timeout, round_trip)
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "DAP request '{}' timed out after {}s",
                        command,
                        timeout.as_secs()
                    )
                })??;

        if wait_for_event {
            let event = match helpers::wait_for_events_timeout(
                |e| {
                    matches!(
                        e,
                        EventKind::Stopped(_) | EventKind::Exited(_) | EventKind::Terminated(_)
                    )
                },
                &mut messages,
                Some(timeout),
            )
            .await
            {
                Ok(event) => Some(WaitedEvent::Received(event.event)),
                Err(_) => Some(WaitedEvent::TimedOut {
                    timeout_seconds: timeout.as_secs(),
                }),
            };

            return Ok(RawDapResult {
                body,
                event,
                extra: Default::default(),
            });
        }

        Ok(RawDapResult {
            body,
            event: None,
            extra: Default::default(),
        })
    }

    pub fn send_dapper_event(&self, event: DapperEvent) -> anyhow::Result<()> {
        self.event_channel.send_event(
            event
                .try_into()
                .context("Failed to serialize DapperEvent")?,
        )
    }

    /// Send breakpoint events for the given reason.
    /// Logpoints are excluded to not confuse users as they do not stop execution
    async fn send_breakpoint_events(
        &self,
        reason: BreakpointEventReason,
        source_path: &str,
        source_name: &str,
        breakpoints: &[BreakpointInfo],
    ) {
        tracing::debug!(
            source_path = %source_path,
            reason = %reason,
            count = breakpoints.len(),
            "Sending breakpoint events"
        );

        for (idx, bp) in breakpoints.iter().enumerate() {
            if bp.log_message.is_some() {
                tracing::debug!(
                    index = idx,
                    line = bp.line,
                    "Skipping breakpoint event for logpoint"
                );
                continue;
            }

            // Skip sending events for breakpoints without an ID to avoid addressing wrong breakpoints
            if bp.id.is_none() {
                tracing::warn!(
                    index = idx,
                    line = bp.line,
                    "Skipping breakpoint event for breakpoint without ID"
                );
                continue;
            }

            let dap_breakpoint = bp.to_dap_breakpoint(source_path, source_name);

            let event_kind = EventKind::Breakpoint(BreakpointEventBody {
                reason: reason.clone(),
                breakpoint: dap_breakpoint,
                extra: Default::default(),
            });

            match self.event_channel.send_event(event_kind) {
                Ok(_) => {
                    tracing::debug!(index = idx, reason = %reason, "Successfully sent breakpoint event")
                }
                Err(e) => {
                    tracing::warn!(index = idx, reason = %reason, error = ?e, "Failed to send breakpoint event")
                }
            }
        }
    }
}

pub(crate) mod helpers {
    use anyhow::bail;

    use super::*;

    pub async fn wait_for_response(
        seq: Seq,
        messages: &mut broadcast::Receiver<Arc<dap::Message>>,
    ) -> anyhow::Result<dap::Response> {
        loop {
            match messages.recv().await {
                Ok(message) => {
                    // Check if this is a DAP response message with matching request_seq
                    if let dap::Message::Response(response) = message.as_ref()
                        && response.request_seq == seq
                    {
                        return Ok(response.clone());
                    }
                    // If it's not the response we're looking for, continue waiting
                }
                Err(err) => {
                    return Err(err).context("Failed to receive a response from the server");
                }
            }
        }
    }

    /// Wait for specific DAP events with an optional timeout
    ///
    /// If timeout is None, waits indefinitely. If timeout is Some(duration), waits up to that duration.
    pub async fn wait_for_events_timeout(
        matches: impl Fn(&EventKind) -> bool,
        messages: &mut broadcast::Receiver<Arc<dap::Message>>,
        timeout: Option<std::time::Duration>,
    ) -> anyhow::Result<dap::Event> {
        let wait_future = async {
            loop {
                match messages.recv().await {
                    Ok(message) => {
                        // Check if this is a DAP event message matching our predicate
                        if let dap::Message::Event(event) = message.as_ref()
                            && matches(&event.event)
                        {
                            return Ok(event.clone());
                        }
                        // If it's not the event we're looking for, continue waiting
                    }
                    Err(err) => {
                        return Err(err).context("Failed to receive an event from the server");
                    }
                }
            }
        };

        match timeout {
            Some(duration) => match tokio::time::timeout(duration, wait_future).await {
                Ok(result) => result,
                Err(_) => {
                    bail!("Timeout waiting for events (waited {:?})", duration);
                }
            },
            None => {
                // No timeout - wait indefinitely
                wait_future.await
            }
        }
    }
}

/// Result of merging an explicit `set_exception_breakpoints` request into
/// the currently-installed set. Counts are computed against the pre-call
/// snapshot and dedupe intra-batch duplicates so a request like
/// `["raised", "raised"]` against an empty installed set scores
/// `(new=1, existing=0)` rather than `(new=1, existing=1)`.
pub(crate) struct MergedExceptionFilters {
    pub merged: Vec<ExceptionFilterEntry>,
    pub new_count: usize,
    pub existing_count: usize,
}

/// Merge an explicit `filters` request into `installed`, returning the
/// post-merge entry list (sorted by filter id via BTreeMap) plus the
/// new/existing counts.
///
/// `clear_existing == true`: ignore `installed`; the explicit list becomes
/// the new set verbatim with `condition: None` for every entry.
///
/// `clear_existing == false`: start from `installed`; only add explicit
/// filters that aren't already there. Preserves existing conditions on
/// re-specified filters (the v1 surface has no way to express conditions,
/// so silently dropping user-set conditions from config/IDE would be a
/// footgun).
pub(crate) fn merge_exception_filters(
    installed: Vec<ExceptionFilterEntry>,
    filters: &[String],
    clear_existing: bool,
) -> MergedExceptionFilters {
    // When clear_existing=true the prior installed set is wholly replaced
    // by the request, so every requested filter is "new" relative to the
    // post-call state — even ones that share an id with a previous entry.
    // Counting against an empty set in that case keeps the contract on
    // `existing_count` ("filter was already there and we kept it") honest.
    let installed_ids: HashSet<&str> = if clear_existing {
        HashSet::new()
    } else {
        installed.iter().map(|e| e.filter.as_str()).collect()
    };
    let mut new_count = 0usize;
    let mut existing_count = 0usize;
    let mut seen_in_request: HashSet<&str> = HashSet::with_capacity(filters.len());
    for filter in filters {
        if !seen_in_request.insert(filter.as_str()) {
            continue;
        }
        if installed_ids.contains(filter.as_str()) {
            existing_count += 1;
        } else {
            new_count += 1;
        }
    }

    let mut merged: BTreeMap<String, ExceptionFilterEntry> = if clear_existing {
        BTreeMap::new()
    } else {
        installed
            .into_iter()
            .map(|entry| (entry.filter.clone(), entry))
            .collect()
    };
    for filter in filters {
        merged
            .entry(filter.clone())
            .or_insert_with(|| ExceptionFilterEntry {
                filter: filter.clone(),
                condition: None,
            });
    }

    MergedExceptionFilters {
        merged: merged.into_values().collect(),
        new_count,
        existing_count,
    }
}

#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing `Debugger` would require broader command plumbing updates"
)]
pub enum Command {
    Control(ControlCommand),
    Debugger(dap::Message),
}

#[derive(Debug, Clone)]
pub enum ControlCommand {
    Status,
}

#[derive(Debug)]
pub enum CommandResult {
    Control(ControlResult),
    Debugger(ListenerPayload),
}

#[derive(Debug)]
pub enum ControlResult {
    // TODO: this will become a real status result later
    Status,
}

#[derive(Debug)]
pub struct ListenerPayload {
    /// Sequence number of the message submitted to the backend.
    /// This could be a real sequence number for the request messages or 0 for
    /// other types of messages, and is implementation dependent.
    pub seq: Seq,
    /// Stream of messages from the server directly before the request was submitted.
    /// Uses Arc<Message> so receivers get a cheap refcount clone instead of deep-cloning.
    pub messages: broadcast::Receiver<Arc<dap::Message>>,
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::capabilities::Capabilities;
    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::enums::StoppedReason;
    use dapper_dap_protocol::events::StoppedEventBody;
    use dapper_dap_protocol::responses::ThreadsResponseBody;

    use super::*;

    /// Create a `ProxyClient` with a `DebugSessionTracker` seeded with the
    /// given adapter capabilities. The server side of the channel is returned
    /// so callers can optionally drive responses.
    fn make_client_with_caps(
        caps: Option<Capabilities>,
    ) -> (ProxyClient, mpsc::UnboundedReceiver<ProxyRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (event_channel, _event_rx) = EventChannel::new_pair();
        let tracker = DebugSessionTracker::new(
            "test-session".into(),
            DapperConfig::default(),
            Some(dapper_session::SessionStore::at(
                dapper_session::get_user_temp_dir().join("client_test_sessions"),
            )),
        );

        if let Some(caps) = caps {
            let response = dap::Response {
                seq: Seq(1),
                request_seq: Seq(1),
                success: true,
                message: None,
                body: ResponseBody::Initialize(Some(caps)),
            };
            tracker.track_message_to_client(&dap::Message::Response(response));
        }

        let client = ProxyClient::new(
            ClientId::new("test-client"),
            tx,
            event_channel,
            tracker,
            DapperConfig::default(),
        );
        (client, rx)
    }

    fn entry(filter: &str, condition: Option<&str>) -> ExceptionFilterEntry {
        ExceptionFilterEntry {
            filter: filter.to_string(),
            condition: condition.map(String::from),
        }
    }

    #[test]
    fn test_merge_exception_filters_clear_existing_replaces_wholesale() {
        let installed = vec![entry("old", Some("c"))];
        let result = merge_exception_filters(
            installed,
            &["raised".to_string(), "uncaught".to_string()],
            true,
        );
        // BTreeMap iteration → sorted output.
        assert_eq!(
            result.merged,
            vec![entry("raised", None), entry("uncaught", None)]
        );
        // Both inputs are new because clear_existing=true ignores the prior set.
        assert_eq!(result.new_count, 2);
        assert_eq!(result.existing_count, 0);
    }

    #[test]
    fn test_merge_exception_filters_clear_existing_with_overlapping_filter() {
        // Bug guard: when clear_existing=true AND an explicit filter shares
        // an id with the prior installed set, the explicit filter must be
        // counted as new (not existing) — the wholesale replace means
        // there's no continuity with the prior entry.
        let installed = vec![entry("raised", Some("c"))];
        let result = merge_exception_filters(installed, &["raised".to_string()], true);
        assert_eq!(result.merged, vec![entry("raised", None)]);
        assert_eq!(
            result.new_count, 1,
            "clear_existing=true should treat all explicit filters as new"
        );
        assert_eq!(result.existing_count, 0);
    }

    #[test]
    fn test_merge_exception_filters_preserve_condition_on_re_specified_filter() {
        // installed has `raised` with a condition; the explicit list also
        // mentions `raised` (without a condition). The condition must be
        // preserved — v1 control-plane API has no way to express
        // conditions, so we mustn't drop them silently.
        let installed = vec![entry("raised", Some("x>5"))];
        let result = merge_exception_filters(
            installed,
            &["raised".to_string(), "uncaught".to_string()],
            false,
        );
        assert_eq!(
            result.merged,
            vec![entry("raised", Some("x>5")), entry("uncaught", None)]
        );
        // raised was already present (existing); uncaught is new.
        assert_eq!(result.new_count, 1);
        assert_eq!(result.existing_count, 1);
    }

    #[test]
    fn test_merge_exception_filters_intra_batch_duplicates_dedupe_and_count_once() {
        // Bug guard: ["raised", "raised"] against an empty installed set
        // must score (new=1, existing=0) — the second occurrence is an
        // intra-request duplicate, not a "previously installed" hit.
        let result =
            merge_exception_filters(vec![], &["raised".to_string(), "raised".to_string()], false);
        assert_eq!(result.merged, vec![entry("raised", None)]);
        assert_eq!(result.new_count, 1);
        assert_eq!(result.existing_count, 0);
    }

    #[test]
    fn test_merge_exception_filters_empty_input_yields_empty_or_installed() {
        // clear_existing=true + empty input → empty merged, zero counts.
        let result = merge_exception_filters(vec![entry("old", None)], &[], true);
        assert!(result.merged.is_empty());
        assert_eq!(result.new_count, 0);
        assert_eq!(result.existing_count, 0);

        // clear_existing=false + empty input → installed preserved.
        let installed = vec![entry("uncaught", None)];
        let result = merge_exception_filters(installed.clone(), &[], false);
        assert_eq!(result.merged, installed);
        assert_eq!(result.new_count, 0);
        assert_eq!(result.existing_count, 0);
    }

    #[test]
    fn test_merge_exception_filters_output_sorted_by_filter_id() {
        // Pass unsorted input; assert merged output is sorted via BTreeMap.
        let result = merge_exception_filters(
            vec![entry("uncaught", None)],
            &["thrown".to_string(), "raised".to_string()],
            false,
        );
        let ids: Vec<&str> = result.merged.iter().map(|e| e.filter.as_str()).collect();
        assert_eq!(ids, vec!["raised", "thrown", "uncaught"]);
    }

    fn make_response(request_seq: Seq) -> dap::Message {
        dap::Response {
            seq: Seq(1),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::Threads(ThreadsResponseBody {
                threads: vec![],
                ..Default::default()
            }),
        }
        .into()
    }

    fn make_event() -> dap::Message {
        dap::Event::new(EventKind::Stopped(StoppedEventBody {
            reason: StoppedReason::Breakpoint,
            thread_id: Some(ThreadId(1)),
            all_threads_stopped: Some(true),
            ..Default::default()
        }))
        .into()
    }

    #[tokio::test]
    async fn test_wait_for_response_finds_matching_seq() {
        let (tx, _) = broadcast::channel::<Arc<dap::Message>>(16);
        let mut rx = tx.subscribe();

        // Send an event (should be skipped), a non-matching response, then the matching one
        tx.send(Arc::new(make_event())).unwrap();
        tx.send(Arc::new(make_response(Seq(99)))).unwrap();
        tx.send(Arc::new(make_response(Seq(42)))).unwrap();

        let response = helpers::wait_for_response(Seq(42), &mut rx).await.unwrap();
        assert_eq!(response.request_seq, Seq(42));
        assert!(response.success);
    }

    #[tokio::test]
    async fn test_wait_for_response_ignores_non_responses() {
        let (tx, _) = broadcast::channel::<Arc<dap::Message>>(16);
        let mut rx = tx.subscribe();

        // Send several events before the matching response
        tx.send(Arc::new(make_event())).unwrap();
        tx.send(Arc::new(make_event())).unwrap();
        tx.send(Arc::new(make_event())).unwrap();
        tx.send(Arc::new(make_response(Seq(7)))).unwrap();

        let response = helpers::wait_for_response(Seq(7), &mut rx).await.unwrap();
        assert_eq!(response.request_seq, Seq(7));
    }

    #[tokio::test]
    async fn test_wait_for_response_channel_closed() {
        let (tx, _) = broadcast::channel::<Arc<dap::Message>>(16);
        let mut rx = tx.subscribe();

        // Drop the sender — channel is closed
        drop(tx);

        let result = helpers::wait_for_response(Seq(1), &mut rx).await;
        assert!(result.is_err(), "Should error when channel is closed");
    }

    #[tokio::test]
    async fn test_wait_for_events_timeout_finds_matching_event() {
        let (tx, _) = broadcast::channel::<Arc<dap::Message>>(16);
        let mut rx = tx.subscribe();

        // Send a non-matching response, then the matching stopped event
        tx.send(Arc::new(make_response(Seq(1)))).unwrap();
        tx.send(Arc::new(make_event())).unwrap();

        let event = helpers::wait_for_events_timeout(
            |kind| matches!(kind, EventKind::Stopped(_)),
            &mut rx,
            Some(std::time::Duration::from_secs(1)),
        )
        .await
        .unwrap();

        assert!(matches!(event.event, EventKind::Stopped(_)));
    }

    #[tokio::test]
    async fn test_wait_for_events_timeout_times_out() {
        let (tx, _) = broadcast::channel::<Arc<dap::Message>>(16);
        let mut rx = tx.subscribe();

        // Send only a response (no matching event)
        tx.send(Arc::new(make_response(Seq(1)))).unwrap();

        let result = helpers::wait_for_events_timeout(
            |kind| matches!(kind, EventKind::Stopped(_)),
            &mut rx,
            Some(std::time::Duration::from_millis(50)),
        )
        .await;

        assert!(
            result.is_err(),
            "Should timeout when no matching event arrives"
        );
    }

    // ---------------------------------------------------------------
    // navigate: single_thread capability gating
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn test_navigate_single_thread_bails_when_capabilities_unknown() {
        let (client, _rx) = make_client_with_caps(None);

        let err = client
            .navigate(NavigationType::Continue, ThreadId(1), Some(true))
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("supportsSingleThreadExecutionRequests"),
            "expected supportsSingleThreadExecutionRequests error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_navigate_single_thread_bails_when_capability_not_advertised() {
        let caps = Capabilities {
            supports_single_thread_execution_requests: Some(false),
            ..Default::default()
        };
        let (client, _rx) = make_client_with_caps(Some(caps));

        let err = client
            .navigate(NavigationType::StepOver, ThreadId(1), Some(true))
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("supportsSingleThreadExecutionRequests"),
            "expected supportsSingleThreadExecutionRequests error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_navigate_single_thread_bails_when_capability_absent() {
        // Capabilities present but the field is None (not advertised).
        let caps = Capabilities::default();
        let (client, _rx) = make_client_with_caps(Some(caps));

        let err = client
            .navigate(NavigationType::StepIn, ThreadId(1), Some(true))
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("supportsSingleThreadExecutionRequests"),
            "expected supportsSingleThreadExecutionRequests error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_navigate_single_thread_passes_when_capability_advertised() {
        let caps = Capabilities {
            supports_single_thread_execution_requests: Some(true),
            ..Default::default()
        };
        let (client, mut rx) = make_client_with_caps(Some(caps));

        // Spawn a mock server that replies with a successful Next response.
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let (msg_tx, _) = broadcast::channel::<Arc<dap::Message>>(16);
                let messages = msg_tx.subscribe();

                let Command::Debugger(dap::Message::Request(dap_req)) = &req.command else {
                    panic!("expected a debugger request");
                };
                let response = dap::Response {
                    seq: Seq(2),
                    request_seq: dap_req.seq,
                    success: true,
                    message: None,
                    body: ResponseBody::Next,
                };
                msg_tx.send(Arc::new(response.into())).unwrap();

                let _ = req.result.send(CommandResult::Debugger(ListenerPayload {
                    seq: dap_req.seq,
                    messages,
                }));
            }
        });

        let result = client
            .navigate(NavigationType::StepOver, ThreadId(1), Some(true))
            .await;

        assert!(result.is_ok(), "navigate should succeed, got: {result:?}");
    }

    #[tokio::test]
    async fn test_navigate_no_single_thread_skips_capability_check() {
        // Capabilities don't advertise singleThread, but single_thread is
        // None so the gate should be skipped entirely.
        let caps = Capabilities::default();
        let (client, mut rx) = make_client_with_caps(Some(caps));

        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let (msg_tx, _) = broadcast::channel::<Arc<dap::Message>>(16);
                let messages = msg_tx.subscribe();

                let Command::Debugger(dap::Message::Request(dap_req)) = &req.command else {
                    panic!("expected a debugger request");
                };
                let response = dap::Response {
                    seq: Seq(2),
                    request_seq: dap_req.seq,
                    success: true,
                    message: None,
                    body: ResponseBody::Next,
                };
                msg_tx.send(Arc::new(response.into())).unwrap();

                let _ = req.result.send(CommandResult::Debugger(ListenerPayload {
                    seq: dap_req.seq,
                    messages,
                }));
            }
        });

        let result = client
            .navigate(NavigationType::StepOver, ThreadId(1), None)
            .await;

        assert!(
            result.is_ok(),
            "navigate without single_thread should succeed, got: {result:?}"
        );
    }
}
