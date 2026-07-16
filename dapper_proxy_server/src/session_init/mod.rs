// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Session initialization for headless mode.
//!
//! This module provides the `SessionInitializer` which handles the DAP
//! initialization sequence for headless operation.
//!
//! See <https://microsoft.github.io/debug-adapter-protocol/overview> for the DAP spec.

mod breakpoints;
mod requests;

use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use dapper_dap_protocol::capabilities::Capabilities;
use dapper_dap_protocol::data_types::ExceptionBreakpointsFilter;
use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::events::EventKind;
use dapper_dap_protocol::events::UnknownEvent;
use dapper_dap_protocol::protocol as dap;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::requests::StartDebuggingRequestArguments;
use dapper_dap_protocol::responses::ResponseBody;
use dapper_dap_protocol::responses::RunInTerminalResponseBody;
use dapper_dap_protocol::responses::UnknownResponseBody;
use dapper_session::ExceptionFilterEntry;
use dapper_session::Port;
use dapper_session::SessionId;
use dapper_session::config::DebugSessionConfig;
use dapper_session::config::can_resolve_for_parent_backend;
use dapper_session::config::resolve_child_session;
// Re-exported so `client.rs` can build the same partition+gating-aware
// setExceptionBreakpoints request that the headless install path uses,
// without widening the whole `requests` submodule. Consumed by the
// control-plane `ProxyClient::set_exception_breakpoints` method added in
// a follow-up PR.
pub(crate) use requests::build_set_exception_breakpoints_request;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::trace;
use tracing::warn;

use self::breakpoints::BreakpointGroups;
use crate::dapper_event::ControlPlaneStatus;
use crate::dapper_event::DapperEvent;
use crate::transport::DuplexChannel;

/// Default timeout for waiting for DAP messages.
///
/// NOTE: assumed to be generous enough for real world use. Let's see if we need
/// to make it configurable.
const DEFAULT_INIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Status of an initialization stage.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Started,
    Completed,
    Failed,
}

/// Progress events emitted during session initialization.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProgressEvent {
    /// DAP initialization sequence progress
    SessionInit {
        status: Status,
        message: String,
        #[serde(rename = "elapsed_ms", with = "duration_ms")]
        elapsed: Duration,
    },
    /// Session is fully ready for debugging
    SessionReady {
        #[serde(rename = "elapsed_ms", with = "duration_ms")]
        elapsed: Duration,
    },
    /// Dapper control plane is ready
    DapperReady {
        session_id: SessionId,
        control_port: Port,
        #[serde(rename = "elapsed_ms", with = "duration_ms")]
        elapsed: Duration,
    },
    /// Debug adapter reported the debuggee process
    ProcessStarted {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        system_process_id: Option<i64>,
        #[serde(rename = "elapsed_ms", with = "duration_ms")]
        elapsed: Duration,
    },
    /// Program has stopped (breakpoint, step, exception, exit, etc.)
    ProgramStopped {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
        #[serde(rename = "elapsed_ms", with = "duration_ms")]
        elapsed: Duration,
    },
}

mod duration_ms {
    use std::time::Duration;

    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serializer;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(d)?;
        Ok(Duration::from_millis(ms))
    }
}

pub enum EventWriter {
    Stdout,
    Writer(Box<dyn Write + Send>),
}

impl EventWriter {
    pub fn stdout() -> Self {
        EventWriter::Stdout
    }

    fn from_writer(writer: impl Write + Send + 'static) -> Self {
        EventWriter::Writer(Box::new(writer))
    }

    #[cfg(unix)]
    pub fn from_raw_fd(fd: i32) -> anyhow::Result<Self> {
        use std::os::unix::io::FromRawFd;

        if fd < 0 {
            anyhow::bail!("--events-fd must be a non-negative file descriptor, got {fd}");
        }
        if fd <= 2 {
            anyhow::bail!("--events-fd must not be stdin (0), stdout (1), or stderr (2), got {fd}");
        }
        // SAFETY: fcntl(F_GETFD) is safe for any fd value — it either
        // returns flags or -1 with errno set, with no side effects.
        let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if fd_flags == -1 {
            anyhow::bail!(
                "--events-fd {fd}: not a valid file descriptor ({})",
                std::io::Error::last_os_error()
            );
        }
        // SAFETY: fcntl(F_GETFL) is safe — fd was validated above via
        // F_GETFD, and F_GETFL only reads status flags with no side effects.
        let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if status_flags == -1 {
            anyhow::bail!(
                "--events-fd {fd}: failed to get file status ({})",
                std::io::Error::last_os_error()
            );
        }
        let access_mode = status_flags & libc::O_ACCMODE;
        if access_mode != libc::O_WRONLY && access_mode != libc::O_RDWR {
            anyhow::bail!("--events-fd {fd}: file descriptor is not writable");
        }
        // SAFETY: fd has been validated as a valid, writable file descriptor.
        // The caller is responsible for ensuring exclusive ownership (standard
        // Unix fd-passing pattern, see gpg --status-fd).
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        // Set FD_CLOEXEC so this events fd is not inherited by any processes this
        // proxy itself spawns (e.g. grandchildren via its own child sessions).
        // Without this the supervisor's read end would not observe EOF until the
        // entire descendant subtree exits, not just its direct child.
        // SAFETY: fcntl(F_GETFD/F_SETFD) only reads/sets this fd's flags; `file`
        // now owns the fd for the rest of its lifetime.
        let cloexec_set = unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            flags != -1 && libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) != -1
        };
        if !cloexec_set {
            // Essentially impossible for a fd already validated above, but log it
            // rather than silently re-introduce the grandchild-leak this guards.
            warn!(
                "--events-fd {fd}: failed to set FD_CLOEXEC ({}); it may leak into grandchild processes",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self::from_writer(file))
    }

    fn emit(&mut self, event: &ProgressEvent) {
        let Ok(json) = serde_json::to_string(event) else {
            return;
        };
        match self {
            EventWriter::Stdout => {
                if let Err(e) = writeln!(std::io::stdout(), "[DAPPER_SESSION] {}", json) {
                    warn!("Failed to write progress event to stdout: {}", e);
                }
            }
            EventWriter::Writer(writer) => {
                if let Err(e) = writeln!(writer, "{}", json) {
                    warn!("Failed to write progress event to events fd: {}", e);
                } else {
                    let _ = writer.flush();
                }
            }
        }
    }
}

/// Bounded time to wait for the child supervisor to confirm a child session
/// spawned, before failing the `startDebugging` reverse request. The supervisor
/// confirms right after `Command::spawn`, so this only needs to absorb brief
/// scheduling/IO latency.
const CHILD_SPAWN_TIMEOUT: Duration = Duration::from_secs(30);

/// A request from a headless `SessionInitializer` to the child supervisor to
/// spawn a peer `dapper proxy from-config` process for a resolved child session.
///
/// Defined here (rather than in the CLI) so `SessionInitializer`'s spawn channel
/// has no upward dependency on the supervisor implementation.
pub struct ChildSpawnRequest {
    /// The fully-resolved child session configuration.
    pub config: DebugSessionConfig,
    /// Reply channel: the supervisor sends `Ok(())` once the child process has
    /// been spawned (and its temp config written), or `Err` with a reason.
    pub reply: oneshot::Sender<anyhow::Result<()>>,
}

pub struct SessionInitializer {
    config: DebugSessionConfig,
    next_seq: Seq,
    /// Sequence number of the launch/attach request, for matching its response.
    debug_request_seq: Option<Seq>,
    /// Stores the launch/attach response if it arrives early (before we wait for it).
    pending_debug_response: Option<dap::Response>,
    timeout: Duration,
    /// Time when initialization started, for progress reporting.
    start_time: Option<Instant>,
    /// Control plane port if dapper event was received.
    control_plane_port: Option<Port>,
    event_writer: EventWriter,
    /// Capabilities reported by the adapter in the `initialize` response.
    /// Captured eagerly so later steps (e.g. `set_exception_breakpoints`)
    /// can consult `exceptionBreakpointFilters` without re-issuing the
    /// request.
    adapter_capabilities: Option<Capabilities>,
    /// Channel to the child supervisor for spawning child sessions in response
    /// to `startDebugging` reverse requests. `None` disables child spawning, so
    /// the reverse request fails closed.
    child_spawn_tx: Option<mpsc::Sender<ChildSpawnRequest>>,
}

impl SessionInitializer {
    pub fn new(config: DebugSessionConfig) -> Self {
        let timeout = config
            .init_timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_INIT_TIMEOUT);
        Self {
            config,
            next_seq: Seq(1),
            debug_request_seq: None,
            pending_debug_response: None,
            timeout,
            start_time: None,
            control_plane_port: None,
            event_writer: EventWriter::stdout(),
            adapter_capabilities: None,
            child_spawn_tx: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_event_writer(mut self, event_writer: EventWriter) -> Self {
        self.event_writer = event_writer;
        self
    }

    /// Install the channel to the child supervisor, enabling this headless
    /// session to spawn child sessions in response to `startDebugging` reverse
    /// requests (subject to the `childSessions` config gates).
    pub fn with_child_spawn_tx(mut self, tx: mpsc::Sender<ChildSpawnRequest>) -> Self {
        self.child_spawn_tx = Some(tx);
        self
    }

    fn elapsed(&self) -> Duration {
        self.start_time.map(|t| t.elapsed()).unwrap_or_default()
    }

    /// Whether to advertise the `supportsStartDebuggingRequest` capability. Only
    /// when we can honor it: a supervisor channel is installed (`current_exe`
    /// can fail, leaving none) and, on Unix, spawning is enabled with positive
    /// budgets and at least one rule whose action fits the parent backend. The
    /// per-rule `when` clauses are request-dependent, so they're not checked here
    /// — `resolve_child_session` still fails closed if none match.
    fn supports_start_debugging(&self) -> bool {
        if !cfg!(unix) {
            return false;
        }
        if self.child_spawn_tx.is_none() {
            return false;
        }
        self.config.child_sessions.as_ref().is_some_and(|c| {
            c.auto_spawn
                && c.max_depth > 0
                && c.max_children > 0
                && c.profile
                    .rules
                    .iter()
                    .any(|rule| can_resolve_for_parent_backend(rule, &self.config.spawn_config))
        })
    }

    /// Receive the next message, processing any that need special handling.
    /// Stashes debug responses, handles dapper events, then returns the message.
    async fn recv_message(
        &mut self,
        channel: &mut DuplexChannel,
    ) -> anyhow::Result<Option<dap::Message>> {
        let msg = match channel.recv().await.context("Failed to receive message")? {
            None => return Ok(None),
            Some(msg) => msg,
        };

        match &msg {
            // This arm only matches the launch/attach response (keyed by
            // debug_request_seq).  If it failed, bail immediately so
            // wait_for_initialized_event doesn't block forever waiting for an
            // `initialized` event that will never arrive.
            dap::Message::Response(response)
                if self.debug_request_seq == Some(response.request_seq) =>
            {
                debug!("Stashing debug response (seq={})", response.request_seq);
                if !response.success {
                    let err_msg = response.message.as_deref().unwrap_or("unknown error");
                    error!("Launch/attach request failed: {}", err_msg);
                    self.event_writer.emit(&ProgressEvent::SessionInit {
                        status: Status::Failed,
                        message: format!("Launch/attach request failed: {}", err_msg),
                        elapsed: self.elapsed(),
                    });
                    anyhow::bail!(
                        "Debug adapter returned error for launch/attach request: {}",
                        err_msg
                    );
                }
                self.pending_debug_response = Some(response.clone());
            }
            dap::Message::Event(event) => match &event.event {
                // Handle dapper control plane event
                EventKind::Unknown(unknown) if unknown.event == "dapper" => {
                    if let Err(e) = self.handle_dapper_event(unknown) {
                        error!("Failed to handle dapper event: {:#}", e);
                    }
                }
                // Handle process event (reports debuggee PID)
                EventKind::Process(process) => {
                    self.event_writer.emit(&ProgressEvent::ProcessStarted {
                        name: process.name.clone(),
                        system_process_id: process.system_process_id,
                        elapsed: self.elapsed(),
                    });
                }
                // Handle program stopped/exited/terminated events
                EventKind::Stopped(_) | EventKind::Exited(_) | EventKind::Terminated(_) => {
                    self.handle_program_stopped_event(event);
                }
                _ => {}
            },
            // Headless mode has no DAP client; route the adapter's reverse
            // requests (`startDebugging` → spawn a child; else → fail closed).
            dap::Message::Request(request) => {
                self.handle_reverse_request(channel, request).await?;
            }
            _ => {}
        }

        Ok(Some(msg))
    }

    /// Route an adapter-originated reverse request. `startDebugging` is handled
    /// by spawning a child session when configured; everything else (and any
    /// `startDebugging` that isn't spawnable) fails closed with a proper
    /// response so the adapter never hangs.
    async fn handle_reverse_request(
        &mut self,
        channel: &mut DuplexChannel,
        request: &dap::Request,
    ) -> anyhow::Result<()> {
        if let RequestCommand::StartDebugging(args) = &request.command {
            return self.handle_start_debugging(channel, request, args).await;
        }
        self.decline_reverse_request(channel, request).await
    }

    /// Handle a `startDebugging` reverse request by resolving and spawning a
    /// child session via the supervisor, replying success only once the child
    /// process is confirmed spawned. Fails closed (a proper failure response —
    /// never a panic, drop, or hang) on any gate: child sessions not configured,
    /// autoSpawn off, depth exhausted, no supervisor channel installed, an
    /// unresolvable request, or a spawn error/timeout.
    async fn handle_start_debugging(
        &mut self,
        channel: &mut DuplexChannel,
        request: &dap::Request,
        args: &StartDebuggingRequestArguments,
    ) -> anyhow::Result<()> {
        // Gate in the handler too — a non-compliant adapter can send this even
        // unadvertised. `max_children > 0` is part of the gate so a deliberate
        // `maxChildren: 0` declines here rather than hitting the no-channel
        // branch below (no channel is installed for a zero cap).
        let enabled = self
            .config
            .child_sessions
            .as_ref()
            .is_some_and(|c| c.auto_spawn && c.max_depth > 0 && c.max_children > 0);
        if !enabled {
            return self.decline_reverse_request(channel, request).await;
        }

        // autoSpawn is on but no supervisor channel is installed — either
        // `setup_child_supervisor` returned `None` (e.g. `current_exe()` failed,
        // so the capability was never advertised) or a non-compliant adapter
        // sent this unprompted. Fail closed rather than panic, drop, or hang.
        let Some(spawn_tx) = self.child_spawn_tx.clone() else {
            warn!("startDebugging requested but no child-spawn channel is installed; declining");
            return self
                .fail_reverse_request(
                    channel,
                    request,
                    "child-session spawning is not available (no supervisor)",
                )
                .await;
        };

        let child_config = match resolve_child_session(&self.config, args) {
            Ok(config) => config,
            Err(e) => {
                warn!("startDebugging could not be resolved into a child session: {e}");
                return self
                    .fail_reverse_request(
                        channel,
                        request,
                        &format!("startDebugging could not be resolved: {e}"),
                    )
                    .await;
            }
        };

        // Send to the supervisor and await spawn confirmation (bounded) so the
        // ack is sent only after the child process actually spawned.
        let (reply_tx, reply_rx) = oneshot::channel();
        let spawn_request = ChildSpawnRequest {
            config: child_config,
            reply: reply_tx,
        };
        if spawn_tx.send(spawn_request).await.is_err() {
            return self
                .fail_reverse_request(channel, request, "child supervisor is unavailable")
                .await;
        }

        match tokio::time::timeout(CHILD_SPAWN_TIMEOUT, reply_rx).await {
            Ok(Ok(Ok(()))) => {
                info!("Spawned child debug session for startDebugging reverse request");
                self.send_reverse_response(channel, request, true, None)
                    .await
            }
            Ok(Ok(Err(e))) => {
                self.fail_reverse_request(
                    channel,
                    request,
                    &format!("failed to spawn child session: {e}"),
                )
                .await
            }
            Ok(Err(_)) => {
                self.fail_reverse_request(
                    channel,
                    request,
                    "child supervisor dropped the spawn request",
                )
                .await
            }
            Err(_) => {
                self.fail_reverse_request(
                    channel,
                    request,
                    "timed out waiting for child session to spawn",
                )
                .await
            }
        }
    }

    /// Send a failure `Response` to an adapter reverse request with a custom
    /// message.
    async fn fail_reverse_request(
        &mut self,
        channel: &mut DuplexChannel,
        request: &dap::Request,
        message: &str,
    ) -> anyhow::Result<()> {
        self.send_reverse_response(channel, request, false, Some(message.to_string()))
            .await
    }

    /// Decline an unsupported adapter reverse request with a generic failure
    /// `Response` so the adapter never hangs.
    async fn decline_reverse_request(
        &mut self,
        channel: &mut DuplexChannel,
        request: &dap::Request,
    ) -> anyhow::Result<()> {
        let command = request.command_name();
        warn!("Declining unsupported reverse request '{command}' in headless mode");
        let message =
            format!("dapper headless session does not support the '{command}' reverse request");
        self.send_reverse_response(channel, request, false, Some(message))
            .await
    }

    /// Build and send a `Response` for an adapter-originated reverse request,
    /// echoing the inbound `request_seq` so the adapter can correlate it. The
    /// body matches the request command where a dedicated `ResponseBody` variant
    /// exists, falling back to a protocol-`Unknown` body keyed by the command
    /// name (keeping the match total).
    async fn send_reverse_response(
        &mut self,
        channel: &mut DuplexChannel,
        request: &dap::Request,
        success: bool,
        message: Option<String>,
    ) -> anyhow::Result<()> {
        let body = match &request.command {
            RequestCommand::StartDebugging(_) => ResponseBody::StartDebugging,
            RequestCommand::RunInTerminal(_) => {
                ResponseBody::RunInTerminal(RunInTerminalResponseBody::default())
            }
            _ => ResponseBody::Unknown(UnknownResponseBody {
                command: request.command_name().to_string(),
                body: None,
                extra: Default::default(),
            }),
        };

        let seq = self.next_seq;
        self.next_seq = self.next_seq.next();
        let response = dap::Response {
            seq,
            request_seq: request.seq,
            success,
            message,
            body,
        };
        channel
            .send(response.into())
            .await
            .context("Failed to send reverse-request response")?;
        Ok(())
    }

    fn handle_dapper_event(&mut self, event: &UnknownEvent) -> anyhow::Result<()> {
        let body = event.body.as_ref().context("dapper event missing body")?;

        let dapper_event: DapperEvent =
            serde_json::from_value(body.clone()).context("failed to parse dapper event")?;

        match dapper_event {
            DapperEvent::ControlPlaneStatus(status) => self.handle_control_plane_status(status),
            DapperEvent::Unknown => {
                debug!("Ignoring unknown dapper event");
                Ok(())
            }
        }
    }

    fn handle_control_plane_status(&mut self, status: ControlPlaneStatus) -> anyhow::Result<()> {
        if !status.success {
            let msg = status.message.as_deref().unwrap_or("unknown error");
            anyhow::bail!("control plane status failed: {}", msg);
        }

        let port = status.port.context("control plane status missing port")?;
        self.control_plane_port = Some(port);
        self.event_writer.emit(&ProgressEvent::DapperReady {
            session_id: status.session_id,
            control_port: port,
            elapsed: self.elapsed(),
        });
        Ok(())
    }

    fn handle_program_stopped_event(&mut self, event: &dap::Event) {
        match &event.event {
            EventKind::Stopped(stopped) => {
                let reason = stopped.reason.to_string();
                self.event_writer.emit(&ProgressEvent::ProgramStopped {
                    reason,
                    exit_code: None,
                    elapsed: self.elapsed(),
                });
            }
            EventKind::Exited(exited) => {
                self.event_writer.emit(&ProgressEvent::ProgramStopped {
                    reason: "exited".to_owned(),
                    exit_code: Some(exited.exit_code),
                    elapsed: self.elapsed(),
                });
            }
            EventKind::Terminated(_) => {
                self.event_writer.emit(&ProgressEvent::ProgramStopped {
                    reason: "terminated".to_owned(),
                    exit_code: None,
                    elapsed: self.elapsed(),
                });
            }
            _ => {}
        }
    }

    /// Run the full initialization sequence, then receive messages until the channel closes.
    ///
    /// Per DAP spec, the sequence is:
    /// 1. Send initialize request, wait for response
    /// 2. Send launch/attach request (don't block for response)
    /// 3. Wait for initialized event
    /// 4. Send configuration (breakpoints, etc.)
    /// 5. Send configurationDone, wait for response
    /// 6. Wait for launch/attach response (may come after configurationDone)
    pub async fn run(mut self, mut channel: DuplexChannel) -> anyhow::Result<()> {
        self.start_time = Some(Instant::now());
        self.event_writer.emit(&ProgressEvent::SessionInit {
            status: Status::Started,
            message: "Starting DAP initialization".into(),
            elapsed: Duration::ZERO,
        });
        info!("Starting DAP initialization sequence");

        self.initialize(&mut channel).await?;
        self.send_debug_request(&mut channel).await?;
        self.wait_for_initialized_event(&mut channel).await?;
        // Build the breakpoint groups once and thread them into both
        // install steps; otherwise each step re-iterates the config and
        // they could drift if grouping ever gains validation/dedup logic.
        let groups = BreakpointGroups::from_breakpoints(&self.config.breakpoints);
        self.set_breakpoints(&mut channel, &groups).await?;
        self.set_exception_breakpoints(&mut channel, &groups)
            .await?;
        self.configuration_done(&mut channel).await?;
        self.wait_for_debug_response(&mut channel).await?;

        self.event_writer.emit(&ProgressEvent::SessionInit {
            status: Status::Completed,
            message: "DAP initialization complete".into(),
            elapsed: self.elapsed(),
        });
        self.event_writer.emit(&ProgressEvent::SessionReady {
            elapsed: self.elapsed(),
        });
        info!("DAP initialization complete, continuing to receive messages");

        self.receive_messages_until_closed(&mut channel).await;

        Ok(())
    }

    async fn initialize(&mut self, channel: &mut DuplexChannel) -> anyhow::Result<()> {
        debug!("Sending initialize request");
        let request = requests::initialize(
            self.config.initialize_args.as_ref(),
            self.supports_start_debugging(),
        )?;
        let response = self.request(channel, request).await?;
        response.check_success()?;
        // Capture capabilities only on success — DAP doesn't define the
        // body shape for failed initialize responses, so storing whatever
        // happens to be there could mislead later steps that consult
        // `adapter_capabilities` (e.g. `set_exception_breakpoints`).
        if let ResponseBody::Initialize(Some(caps)) = &response.body {
            self.adapter_capabilities = Some(caps.clone());
        }
        Ok(())
    }

    /// Send the launch/attach request but don't wait for its response.
    /// Per DAP spec, the response may not come until after configurationDone.
    async fn send_debug_request(&mut self, channel: &mut DuplexChannel) -> anyhow::Result<()> {
        let debug_request = self
            .config
            .debug_request
            .as_ref()
            .context("No debug_request in config")?;

        debug!("Sending debug request (launch/attach)");
        let request = requests::debug_request(debug_request);
        let seq = self.send(channel, request).await?;
        self.debug_request_seq = Some(seq);

        debug!(
            "Debug request sent (seq={}), will wait for response later",
            seq
        );
        Ok(())
    }

    /// Wait for the initialized event from the debug adapter.
    /// Per DAP spec, this event signals that the DA is ready to receive configuration.
    async fn wait_for_initialized_event_no_timeout(
        &mut self,
        channel: &mut DuplexChannel,
    ) -> anyhow::Result<()> {
        debug!("Waiting for initialized event");
        loop {
            let Some(msg) = self.recv_message(channel).await? else {
                anyhow::bail!("Channel closed while waiting for initialized event");
            };

            if let dap::Message::Event(ref event) = msg
                && matches!(&event.event, EventKind::Initialized(_))
            {
                debug!("Received initialized event");
                return Ok(());
            }
            trace!(
                "Received message while waiting for initialized: {:?}",
                msg.message_type()
            );
        }
    }

    async fn wait_for_initialized_event(
        &mut self,
        channel: &mut DuplexChannel,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(
            self.timeout,
            self.wait_for_initialized_event_no_timeout(channel),
        )
        .await
        .context("Timed out waiting for initialized event")?
    }

    /// Wait for the launch/attach response.
    /// It may have already arrived (stored in pending_debug_response) or may come now.
    async fn wait_for_debug_response(&mut self, channel: &mut DuplexChannel) -> anyhow::Result<()> {
        if let Some(response) = self.pending_debug_response.take() {
            debug!("Using previously received debug response");
            return response.check_success();
        }

        let debug_seq = self
            .debug_request_seq
            .context("No debug request was sent")?;

        debug!("Waiting for debug response (seq={})", debug_seq);
        let response = self.wait_for_response(channel, debug_seq).await?;
        response.check_success()
    }

    async fn set_breakpoints(
        &mut self,
        channel: &mut DuplexChannel,
        groups: &BreakpointGroups,
    ) -> anyhow::Result<()> {
        // `groups` carries source, function, and exception buckets; this
        // step issues only the source and function setBreakpoints requests.
        // Exception filters are sent separately by
        // `set_exception_breakpoints` after this returns.
        let has_source_or_function =
            groups.source_breakpoints().count() > 0 || !groups.function_breakpoints().is_empty();
        if !has_source_or_function {
            debug!("No source or function breakpoints to set");
            return Ok(());
        }

        for (path, lines) in groups.source_breakpoints() {
            debug!("Setting breakpoints in {} at lines {:?}", path, lines);
            let request = requests::set_breakpoints(path, lines);
            let response = self.request(channel, request).await?;

            if !response.success {
                warn!(
                    "setBreakpoints for {} failed: {}",
                    path,
                    response.message.as_deref().unwrap_or_default()
                );
            }
        }

        let function_bps = groups.function_breakpoints();
        if !function_bps.is_empty() {
            debug!("Setting function breakpoints: {:?}", function_bps);
            let request = requests::set_function_breakpoints(function_bps);
            let response = self.request(channel, request).await?;

            if !response.success {
                warn!(
                    "setFunctionBreakpoints failed: {}",
                    response.message.as_deref().unwrap_or_default()
                );
            }
        }

        Ok(())
    }

    /// Send the `setExceptionBreakpoints` request between `setBreakpoints`
    /// and `configurationDone`, per the DAP spec ordering.
    ///
    /// Validation (per design):
    /// 1. If the adapter advertises no exception filters at all, no-op
    ///    with a debug log when no explicit `Exception` entries are in
    ///    config (avoids spamming startup), or a warn log listing the
    ///    skipped filter ids when there are explicit entries (so users
    ///    don't lose their config silently). Init succeeds either way.
    /// 2. If the adapter advertises filters and the config references an
    ///    unknown filter id, bail with the valid ids — the config is wrong
    ///    and silent skipping would mask a typo.
    /// 3. Adapter-rejected response is logged at warn (not bailed) so the
    ///    rest of init can complete; matches the function-bp pattern.
    async fn set_exception_breakpoints(
        &mut self,
        channel: &mut DuplexChannel,
        groups: &BreakpointGroups,
    ) -> anyhow::Result<()> {
        let explicit = groups.exception_filters();

        let advertised: &[ExceptionBreakpointsFilter] = self
            .adapter_capabilities
            .as_ref()
            .and_then(|c| c.exception_breakpoint_filters.as_deref())
            .unwrap_or(&[]);

        if advertised.is_empty() {
            if explicit.is_empty() {
                debug!("Adapter advertises no exception breakpoint filters; nothing to install");
            } else {
                let skipped: Vec<&str> = explicit.iter().map(|e| e.filter.as_str()).collect();
                warn!(
                    skipped = ?skipped,
                    "adapter advertises no exception breakpoint filters; skipping {} config-requested filter(s)",
                    skipped.len(),
                );
            }
            return Ok(());
        }

        // Validate explicit filter ids against the advertised set — this is
        // initialization-fatal because a typo in the config would
        // otherwise silently fall through. Collect all unknown ids before
        // bailing so a user with multiple typos sees them all at once
        // instead of fixing-and-rerunning.
        let unknown: Vec<&str> = explicit
            .iter()
            .filter(|e| !advertised.iter().any(|f| f.filter == e.filter))
            .map(|e| e.filter.as_str())
            .collect();
        if !unknown.is_empty() {
            let valid: Vec<&str> = advertised.iter().map(|f| f.filter.as_str()).collect();
            anyhow::bail!(
                "config references unknown exception breakpoint filter(s) {:?}; valid ids: {:?}",
                unknown,
                valid,
            );
        }

        // Merge: defaults (if opted in) first, then explicit list overrides
        // by filter id. Use BTreeMap so iteration is deterministic by id.
        let mut merged: BTreeMap<String, ExceptionFilterEntry> = BTreeMap::new();
        if self.config.install_default_exception_breakpoints {
            for f in advertised.iter().filter(|f| f.default == Some(true)) {
                merged.insert(
                    f.filter.clone(),
                    ExceptionFilterEntry {
                        filter: f.filter.clone(),
                        condition: None,
                    },
                );
            }
        }
        for entry in explicit {
            merged.insert(entry.filter.clone(), entry.clone());
        }

        if merged.is_empty() {
            debug!("No exception breakpoints to install");
            return Ok(());
        }

        // `merged.into_values()` already iterates in BTreeMap key order, so
        // `merged_vec` is sorted by filter id. The builder also sorts
        // defensively, which is redundant on this path but keeps the
        // builder safe for any other caller.
        let merged_vec: Vec<ExceptionFilterEntry> = merged.into_values().collect();
        let (request, effective) = build_set_exception_breakpoints_request(
            &merged_vec,
            self.adapter_capabilities.as_ref(),
        );

        // The builder logs each condition drop at warn level individually,
        // so we don't need an aggregate count here — log just the sent count.
        debug!(
            count = effective.len(),
            "Sending setExceptionBreakpoints request"
        );
        let response = self.request(channel, request).await?;
        if !response.success {
            warn!(
                "setExceptionBreakpoints failed: {}",
                response.message.as_deref().unwrap_or_default()
            );
        }
        Ok(())
    }

    async fn configuration_done(&mut self, channel: &mut DuplexChannel) -> anyhow::Result<()> {
        let supports = self
            .adapter_capabilities
            .as_ref()
            .and_then(|c| c.supports_configuration_done_request)
            .unwrap_or(false);
        if !supports {
            debug!(
                "Skipping configurationDone request: adapter does not advertise supportsConfigurationDoneRequest"
            );
            return Ok(());
        }
        debug!("Sending configurationDone request");
        let request = requests::configuration_done();
        let response = self.request(channel, request).await?;
        response.check_success()
    }

    async fn send(
        &mut self,
        channel: &mut DuplexChannel,
        mut request: dap::Request,
    ) -> anyhow::Result<Seq> {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.next();
        request.seq = seq;

        channel
            .send(request.into())
            .await
            .context("Failed to send request")?;

        Ok(seq)
    }

    /// Send a request and wait for its response (with timeout).
    async fn request(
        &mut self,
        channel: &mut DuplexChannel,
        request: dap::Request,
    ) -> anyhow::Result<dap::Response> {
        let seq = self.send(channel, request).await?;
        self.wait_for_response(channel, seq).await
    }

    async fn wait_for_response_no_timeout(
        &mut self,
        channel: &mut DuplexChannel,
        seq: Seq,
    ) -> anyhow::Result<dap::Response> {
        // Check if already stashed
        if let Some(response) = &self.pending_debug_response
            && response.request_seq == seq
        {
            debug!("Using stashed response (seq={})", seq);
            return Ok(self.pending_debug_response.take().unwrap());
        }

        loop {
            let Some(msg) = self.recv_message(channel).await? else {
                anyhow::bail!("Channel closed while waiting for response");
            };

            match msg {
                dap::Message::Response(response) if response.request_seq == seq => {
                    return Ok(response);
                }
                other => {
                    trace!(
                        "Received message while waiting for response: {:?}",
                        other.message_type()
                    );
                }
            }
        }
    }

    async fn wait_for_response(
        &mut self,
        channel: &mut DuplexChannel,
        seq: Seq,
    ) -> anyhow::Result<dap::Response> {
        tokio::time::timeout(
            self.timeout,
            self.wait_for_response_no_timeout(channel, seq),
        )
        .await
        .context("Timed out waiting for response")?
    }

    async fn receive_messages_until_closed(&mut self, channel: &mut DuplexChannel) {
        loop {
            match self.recv_message(channel).await {
                Ok(Some(msg)) => {
                    // In headless mode, exit once the debuggee terminates.
                    // The `exited` and `terminated` events signal the debug
                    // session is over; without this, the proxy hangs.
                    if let dap::Message::Event(event) = &msg
                        && matches!(
                            &event.event,
                            EventKind::Exited(_) | EventKind::Terminated(_)
                        )
                    {
                        debug!("Received {:?} event, init client exiting", event.event);
                        break;
                    }
                    trace!("Received message: {:?}", msg.message_type());
                }
                Ok(None) => {
                    debug!("Channel closed, init client exiting");
                    break;
                }
                Err(e) => {
                    warn!("Error receiving message: {}", e);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use dapper_dap_protocol::capabilities::Capabilities;
    use dapper_dap_protocol::events::EventKind;
    use dapper_dap_protocol::requests::RequestCommand;
    use dapper_dap_protocol::requests::SetExceptionBreakpointsArguments;
    use dapper_dap_protocol::responses::ResponseBody;
    use dapper_session::config::BreakpointSpec;
    use dapper_session::config::DebugRequest;
    use dapper_session::config::SpawnConfig;
    use dapper_session::config::StdioSpawnConfig;

    use super::*;

    /// Protocol state for the mock backend
    #[derive(Debug, Clone, Copy, PartialEq)]
    #[expect(
        clippy::enum_variant_names,
        reason = "test mock states read clearly with the shared protocol-phase prefix"
    )]
    enum MockState {
        /// Waiting for initialize request
        WaitingForInitialize,
        /// Waiting for launch/attach request
        WaitingForDebugRequest,
        /// Waiting for configurationDone (initialized event sent)
        WaitingForConfigurationDone,
    }

    /// DAP spec-compliant mock backend with strict protocol enforcement.
    /// Returns error when messages arrive out of order.
    ///
    /// Expected sequence:
    /// 1. initialize request → initialize response
    /// 2. launch/attach request → initialized EVENT (response deferred)
    /// 3. configurationDone request → configurationDone response + deferred launch/attach response
    async fn mock_backend(mut channel: DuplexChannel) -> anyhow::Result<()> {
        let mut state = MockState::WaitingForInitialize;
        let mut pending_debug_request: Option<dap::Request> = None;
        let mut seq: Seq = 1.into();

        while let Ok(Some(msg)) = channel.recv().await {
            if let dap::Message::Request(req) = msg {
                match (&state, &req.command) {
                    (MockState::WaitingForInitialize, RequestCommand::Initialize(_)) => {
                        let response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::Initialize(Some(Capabilities {
                                supports_configuration_done_request: Some(true),
                                ..Default::default()
                            })),
                        };
                        seq = seq.next();
                        channel
                            .send(response.into())
                            .await
                            .context("Failed to send initialize response")?;
                        state = MockState::WaitingForDebugRequest;
                    }

                    (
                        MockState::WaitingForDebugRequest,
                        RequestCommand::Launch(_) | RequestCommand::Attach(_),
                    ) => {
                        pending_debug_request = Some(req);
                        let initialized_event = dap::Event {
                            seq,
                            event: EventKind::Initialized(Default::default()),
                        };
                        seq = seq.next();
                        channel
                            .send(initialized_event.into())
                            .await
                            .context("Failed to send initialized event")?;
                        state = MockState::WaitingForConfigurationDone;
                    }

                    (
                        MockState::WaitingForConfigurationDone,
                        RequestCommand::ConfigurationDone(_),
                    ) => {
                        let config_response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::ConfigurationDone,
                        };
                        seq = seq.next();
                        channel
                            .send(config_response.into())
                            .await
                            .context("Failed to send configurationDone response")?;

                        // Send deferred launch/attach response
                        if let Some(debug_req) = pending_debug_request.take() {
                            let debug_response_body = match &debug_req.command {
                                RequestCommand::Launch(_) => ResponseBody::Launch,
                                RequestCommand::Attach(_) => ResponseBody::Attach,
                                _ => unreachable!(),
                            };
                            let debug_response = dap::Response {
                                seq,
                                request_seq: debug_req.seq,
                                success: true,
                                message: None,
                                body: debug_response_body,
                            };
                            channel
                                .send(debug_response.into())
                                .await
                                .context("Failed to send debug response")?;
                        }
                        return Ok(());
                    }

                    // Invalid: out-of-order request
                    (current_state, cmd) => {
                        anyhow::bail!(
                            "Protocol violation: received '{}' in state {:?}",
                            cmd.command_name(),
                            current_state
                        );
                    }
                }
            }
        }

        anyhow::bail!("Channel closed before completion, state: {:?}", state)
    }

    #[tokio::test]
    async fn test_initialization_sequence() {
        let config = DebugSessionConfig {
            spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
                cmd: "test".to_owned(),
                args: vec![],
                new_session: false,
            }),
            debug_request: Some(DebugRequest::Launch(
                serde_json::from_value(serde_json::json!({
                    "program": "/path/to/program"
                }))
                .unwrap(),
            )),
            breakpoints: vec![],
            metadata: Default::default(),
            initialize_args: None,
            init_timeout_secs: None,
            install_default_exception_breakpoints: false,
            child_sessions: None,
        };

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend(server));

        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(1));
        let result = initializer.run(client).await;

        // Check that both the client and mock succeeded
        let backend_result = backend_handle.await.expect("backend task panicked");
        assert!(
            backend_result.is_ok(),
            "Mock backend failed: {:?}",
            backend_result
        );
        assert!(result.is_ok(), "SessionInitializer failed: {:?}", result);
    }

    #[tokio::test]
    async fn test_initialization_fails_without_debug_request() {
        let config = DebugSessionConfig {
            spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
                cmd: "test".to_owned(),
                args: vec![],
                new_session: false,
            }),
            debug_request: None,
            breakpoints: vec![],
            metadata: Default::default(),
            initialize_args: None,
            init_timeout_secs: None,
            install_default_exception_breakpoints: false,
            child_sessions: None,
        };

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend(server));

        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(1));
        let result = initializer.run(client).await;

        backend_handle.abort();
        assert!(result.is_err());
    }

    /// Mock backend that responds to initialize but never sends the initialized event.
    /// Used to test init timeout behavior.
    async fn mock_backend_hangs_after_initialize(mut channel: DuplexChannel) -> anyhow::Result<()> {
        let mut seq: Seq = 1.into();

        // Wait for initialize request
        loop {
            let Some(msg) = channel.recv().await? else {
                anyhow::bail!("Channel closed");
            };
            if let dap::Message::Request(req) = msg
                && matches!(req.command, RequestCommand::Initialize(_))
            {
                let response = dap::Response {
                    seq,
                    request_seq: req.seq,
                    success: true,
                    message: None,
                    body: ResponseBody::Initialize(Some(Capabilities::default())),
                };
                seq = Seq(seq.0 + 1);
                channel.send(response.into()).await?;
                break;
            }
        }

        // Read launch request but never send initialized event — just hang
        let _ = channel.recv().await;
        let _ = seq;
        std::future::pending::<()>().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_init_timeout_from_config() {
        // Verify that init_timeout_secs from config is applied correctly.
        // Use a very short timeout (1 second) with a backend that never sends
        // the initialized event, so the timeout triggers quickly.
        let config = DebugSessionConfig {
            spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
                cmd: "test".to_owned(),
                args: vec![],
                new_session: false,
            }),
            debug_request: Some(DebugRequest::Launch(
                serde_json::from_value(serde_json::json!({
                    "program": "/path/to/program"
                }))
                .unwrap(),
            )),
            breakpoints: vec![],
            metadata: Default::default(),
            initialize_args: None,
            init_timeout_secs: Some(1),
            install_default_exception_breakpoints: false,
            child_sessions: None,
        };

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend_hangs_after_initialize(server));

        let initializer = SessionInitializer::new(config.clone())
            .with_timeout(Duration::from_secs(config.init_timeout_secs.unwrap()));
        let start = std::time::Instant::now();
        let result = initializer.run(client).await;
        let elapsed = start.elapsed();

        backend_handle.abort();
        assert!(result.is_err(), "Expected timeout error");
        assert!(
            format!("{:?}", result.unwrap_err()).contains("Timed out"),
            "Expected timeout error message"
        );
        // Should have timed out in ~1 second, not 5 minutes
        assert!(
            elapsed < Duration::from_secs(5),
            "Timeout took too long: {:?}",
            elapsed
        );
    }

    /// Mock backend variant that captures any setExceptionBreakpoints
    /// requests received between `initialized` and `configurationDone`.
    /// `caps` is returned in the initialize response.
    async fn mock_backend_with_caps(
        mut channel: DuplexChannel,
        caps: Capabilities,
        captured: Arc<Mutex<Vec<SetExceptionBreakpointsArguments>>>,
    ) -> anyhow::Result<()> {
        let mut state = MockState::WaitingForInitialize;
        let mut pending_debug_request: Option<dap::Request> = None;
        let mut seq: Seq = 1.into();

        while let Ok(Some(msg)) = channel.recv().await {
            if let dap::Message::Request(req) = msg {
                match (&state, &req.command) {
                    (MockState::WaitingForInitialize, RequestCommand::Initialize(_)) => {
                        let response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::Initialize(Some(caps.clone())),
                        };
                        seq = seq.next();
                        channel.send(response.into()).await?;
                        state = MockState::WaitingForDebugRequest;
                    }
                    (
                        MockState::WaitingForDebugRequest,
                        RequestCommand::Launch(_) | RequestCommand::Attach(_),
                    ) => {
                        pending_debug_request = Some(req);
                        let initialized_event = dap::Event {
                            seq,
                            event: EventKind::Initialized(Default::default()),
                        };
                        seq = seq.next();
                        channel.send(initialized_event.into()).await?;
                        state = MockState::WaitingForConfigurationDone;
                    }
                    // Allow setExceptionBreakpoints between initialized and
                    // configurationDone; capture for assertions.
                    (
                        MockState::WaitingForConfigurationDone,
                        RequestCommand::SetExceptionBreakpoints(args),
                    ) => {
                        captured.lock().unwrap().push(args.clone());
                        let response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::SetExceptionBreakpoints(None),
                        };
                        seq = seq.next();
                        channel.send(response.into()).await?;
                    }
                    (
                        MockState::WaitingForConfigurationDone,
                        RequestCommand::ConfigurationDone(_),
                    ) => {
                        let config_response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::ConfigurationDone,
                        };
                        seq = seq.next();
                        channel.send(config_response.into()).await?;

                        if let Some(debug_req) = pending_debug_request.take() {
                            let body = match &debug_req.command {
                                RequestCommand::Launch(_) => ResponseBody::Launch,
                                RequestCommand::Attach(_) => ResponseBody::Attach,
                                _ => unreachable!(),
                            };
                            let debug_response = dap::Response {
                                seq,
                                request_seq: debug_req.seq,
                                success: true,
                                message: None,
                                body,
                            };
                            channel.send(debug_response.into()).await?;
                        }
                        return Ok(());
                    }
                    (current_state, cmd) => {
                        anyhow::bail!(
                            "Protocol violation: received '{}' in state {:?}",
                            cmd.command_name(),
                            current_state
                        );
                    }
                }
            }
        }
        anyhow::bail!("Channel closed before completion, state: {:?}", state)
    }

    fn launch_config(
        breakpoints: Vec<BreakpointSpec>,
        install_defaults: bool,
    ) -> DebugSessionConfig {
        DebugSessionConfig {
            spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
                cmd: "test".to_owned(),
                args: vec![],
                new_session: false,
            }),
            debug_request: Some(DebugRequest::Launch(
                serde_json::from_value(serde_json::json!({
                    "program": "/path/to/program"
                }))
                .unwrap(),
            )),
            breakpoints,
            metadata: Default::default(),
            initialize_args: None,
            init_timeout_secs: None,
            install_default_exception_breakpoints: install_defaults,
            child_sessions: None,
        }
    }

    #[tokio::test]
    async fn test_no_filters_advertised_no_explicit_no_request_sent() {
        // Adapter advertises nothing; config has no Exception entries; even
        // with install_defaults=true the path should no-op silently
        // (debug-level log only).
        let config = launch_config(vec![], true);
        let captured = Arc::new(Mutex::new(Vec::new()));

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend_with_caps(
            server,
            Capabilities {
                supports_configuration_done_request: Some(true),
                ..Default::default()
            },
            Arc::clone(&captured),
        ));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.await.expect("backend panicked").unwrap();
        assert!(result.is_ok(), "init failed: {:?}", result);
        assert!(
            captured.lock().unwrap().is_empty(),
            "expected no setExceptionBreakpoints request",
        );
    }

    #[tokio::test]
    async fn test_no_filters_advertised_with_explicit_warns_no_request() {
        // Adapter advertises nothing but config has an explicit Exception
        // entry. Init succeeds (warn-only); no DAP request is sent.
        let config = launch_config(vec![BreakpointSpec::exception("uncaught", None)], false);
        let captured = Arc::new(Mutex::new(Vec::new()));

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend_with_caps(
            server,
            Capabilities {
                supports_configuration_done_request: Some(true),
                ..Default::default()
            },
            Arc::clone(&captured),
        ));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.await.expect("backend panicked").unwrap();
        assert!(result.is_ok(), "init failed: {:?}", result);
        assert!(captured.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_default_filters_expanded_from_capabilities() {
        // Adapter advertises a default-true filter; config opts in to defaults;
        // request should include that filter.
        let caps = Capabilities {
            supports_configuration_done_request: Some(true),
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "uncaught".to_string(),
                label: "Uncaught Exceptions".to_string(),
                default: Some(true),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let config = launch_config(vec![], true);
        let captured = Arc::new(Mutex::new(Vec::new()));

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle =
            tokio::spawn(mock_backend_with_caps(server, caps, Arc::clone(&captured)));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.await.expect("backend panicked").unwrap();
        assert!(result.is_ok(), "init failed: {:?}", result);
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].filters, vec!["uncaught"]);
    }

    #[tokio::test]
    async fn test_unknown_filter_id_with_advertised_filters_fails_init() {
        // Adapter advertises only "raised"; config asks for "bogus" → init
        // should bail with an error listing the valid ids.
        let caps = Capabilities {
            supports_configuration_done_request: Some(true),
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "raised".to_string(),
                label: "Raised".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let config = launch_config(vec![BreakpointSpec::exception("bogus", None)], false);
        let captured = Arc::new(Mutex::new(Vec::new()));

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle =
            tokio::spawn(mock_backend_with_caps(server, caps, Arc::clone(&captured)));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.abort();
        let err = result.expect_err("expected init to fail on unknown filter id");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("bogus"),
            "expected error to mention the bad id; got: {msg}"
        );
        assert!(
            msg.contains("raised"),
            "expected error to list valid ids; got: {msg}"
        );
        assert!(captured.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_explicit_overrides_default_filter() {
        // Adapter advertises both `uncaught` (default=true) and `raised`
        // (default=false). Config opts in to defaults AND lists an explicit
        // `uncaught` with a condition. The explicit entry must win — the
        // request should carry the explicit condition, not just the bare
        // default-expansion entry.
        let caps = Capabilities {
            supports_configuration_done_request: Some(true),
            supports_exception_filter_options: Some(true),
            exception_breakpoint_filters: Some(vec![
                ExceptionBreakpointsFilter {
                    filter: "uncaught".to_string(),
                    label: "Uncaught".to_string(),
                    default: Some(true),
                    supports_condition: Some(true),
                    ..Default::default()
                },
                ExceptionBreakpointsFilter {
                    filter: "raised".to_string(),
                    label: "Raised".to_string(),
                    default: Some(false),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let config = launch_config(
            vec![BreakpointSpec::exception(
                "uncaught",
                Some("err.code == 42".to_string()),
            )],
            true,
        );
        let captured = Arc::new(Mutex::new(Vec::new()));

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle =
            tokio::spawn(mock_backend_with_caps(server, caps, Arc::clone(&captured)));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.await.expect("backend panicked").unwrap();
        assert!(result.is_ok(), "init failed: {:?}", result);
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        // Explicit entry wins: uncaught carries the condition via filter_options;
        // no plain `filters` entry.
        assert!(
            captured[0].filters.is_empty(),
            "expected explicit entry to take filterOptions slot; got filters={:?}",
            captured[0].filters
        );
        let opts = captured[0]
            .filter_options
            .as_ref()
            .expect("expected filter_options to be populated");
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].filter_id, "uncaught");
        assert_eq!(opts[0].condition.as_deref(), Some("err.code == 42"));
    }

    #[tokio::test]
    async fn test_no_request_when_neither_defaults_nor_explicit_contribute() {
        // Adapter advertises filters but none are default-true; config has
        // no explicit entries and `install_defaults=false`. The merge step
        // produces an empty set and no setExceptionBreakpoints request
        // should be sent.
        let caps = Capabilities {
            supports_configuration_done_request: Some(true),
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "raised".to_string(),
                label: "Raised".to_string(),
                default: Some(false),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let config = launch_config(vec![], false);
        let captured = Arc::new(Mutex::new(Vec::new()));

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle =
            tokio::spawn(mock_backend_with_caps(server, caps, Arc::clone(&captured)));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.await.expect("backend panicked").unwrap();
        assert!(result.is_ok(), "init failed: {:?}", result);
        assert!(
            captured.lock().unwrap().is_empty(),
            "expected no setExceptionBreakpoints request when merged set is empty"
        );
    }

    #[tokio::test]
    async fn test_explicit_condition_dropped_when_caps_lack_support() {
        // Adapter advertises "raised" but has neither
        // `supports_exception_filter_options` nor per-filter
        // `supports_condition`. The condition on the explicit Exception
        // entry must be dropped silently in `filter_options` and the
        // request must carry the bare filter id.
        let caps = Capabilities {
            supports_configuration_done_request: Some(true),
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "raised".to_string(),
                label: "Raised".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let config = launch_config(
            vec![BreakpointSpec::exception("raised", Some("x>5".to_string()))],
            false,
        );
        let captured = Arc::new(Mutex::new(Vec::new()));

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle =
            tokio::spawn(mock_backend_with_caps(server, caps, Arc::clone(&captured)));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.await.expect("backend panicked").unwrap();
        assert!(result.is_ok(), "init failed: {:?}", result);
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].filters, vec!["raised"]);
        assert!(
            captured[0].filter_options.is_none(),
            "expected no filterOptions because caps don't support conditions; got {:?}",
            captured[0].filter_options
        );
    }

    /// Variant of `mock_backend_with_caps` that responds to
    /// `setExceptionBreakpoints` with `success: false` so we can verify
    /// the warn-on-failure path doesn't bail init.
    async fn mock_backend_rejecting_set_exception_breakpoints(
        mut channel: DuplexChannel,
        caps: Capabilities,
    ) -> anyhow::Result<()> {
        let mut state = MockState::WaitingForInitialize;
        let mut pending_debug_request: Option<dap::Request> = None;
        let mut seq: Seq = 1.into();

        while let Ok(Some(msg)) = channel.recv().await {
            if let dap::Message::Request(req) = msg {
                match (&state, &req.command) {
                    (MockState::WaitingForInitialize, RequestCommand::Initialize(_)) => {
                        let response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::Initialize(Some(caps.clone())),
                        };
                        seq = seq.next();
                        channel.send(response.into()).await?;
                        state = MockState::WaitingForDebugRequest;
                    }
                    (
                        MockState::WaitingForDebugRequest,
                        RequestCommand::Launch(_) | RequestCommand::Attach(_),
                    ) => {
                        pending_debug_request = Some(req);
                        let initialized_event = dap::Event {
                            seq,
                            event: EventKind::Initialized(Default::default()),
                        };
                        seq = seq.next();
                        channel.send(initialized_event.into()).await?;
                        state = MockState::WaitingForConfigurationDone;
                    }
                    (
                        MockState::WaitingForConfigurationDone,
                        RequestCommand::SetExceptionBreakpoints(_),
                    ) => {
                        // Respond with success=false to exercise the warn path.
                        let response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: false,
                            message: Some("simulated adapter rejection".to_string()),
                            body: ResponseBody::SetExceptionBreakpoints(None),
                        };
                        seq = seq.next();
                        channel.send(response.into()).await?;
                    }
                    (
                        MockState::WaitingForConfigurationDone,
                        RequestCommand::ConfigurationDone(_),
                    ) => {
                        let config_response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::ConfigurationDone,
                        };
                        seq = seq.next();
                        channel.send(config_response.into()).await?;
                        if let Some(debug_req) = pending_debug_request.take() {
                            let body = match &debug_req.command {
                                RequestCommand::Launch(_) => ResponseBody::Launch,
                                RequestCommand::Attach(_) => ResponseBody::Attach,
                                _ => unreachable!(),
                            };
                            let debug_response = dap::Response {
                                seq,
                                request_seq: debug_req.seq,
                                success: true,
                                message: None,
                                body,
                            };
                            channel.send(debug_response.into()).await?;
                        }
                        return Ok(());
                    }
                    (current_state, cmd) => {
                        anyhow::bail!(
                            "Protocol violation: received '{}' in state {:?}",
                            cmd.command_name(),
                            current_state
                        );
                    }
                }
            }
        }
        anyhow::bail!("Channel closed before completion, state: {:?}", state)
    }

    #[tokio::test]
    async fn test_set_exception_breakpoints_rejection_does_not_bail_init() {
        // Adapter advertises a default-true filter so the install path
        // sends a request, but rejects it. Init should still complete
        // (warn-only, matching the function-bp pattern).
        let caps = Capabilities {
            supports_configuration_done_request: Some(true),
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "uncaught".to_string(),
                label: "Uncaught".to_string(),
                default: Some(true),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let config = launch_config(vec![], true);

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend_rejecting_set_exception_breakpoints(
            server, caps,
        ));
        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(2));
        let result = initializer.run(client).await;

        backend_handle.await.expect("backend panicked").unwrap();
        assert!(
            result.is_ok(),
            "init should succeed despite adapter rejection; got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_initialization_with_custom_initialize_args() {
        let config = DebugSessionConfig {
            spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
                cmd: "test".to_owned(),
                args: vec![],
                new_session: false,
            }),
            debug_request: Some(DebugRequest::Launch(
                serde_json::from_value(serde_json::json!({
                    "program": "/path/to/program"
                }))
                .unwrap(),
            )),
            breakpoints: vec![],
            metadata: Default::default(),
            initialize_args: Some(serde_json::json!({
                "adapterID": "cppdbg",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path"
            })),
            init_timeout_secs: None,
            install_default_exception_breakpoints: false,
            child_sessions: None,
        };

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend(server));

        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(1));
        let result = initializer.run(client).await;

        let backend_result = backend_handle.await.expect("backend task panicked");
        assert!(
            backend_result.is_ok(),
            "Mock backend failed: {:?}",
            backend_result
        );
        assert!(result.is_ok(), "SessionInitializer failed: {:?}", result);
    }

    /// Mock backend that does not advertise supportsConfigurationDoneRequest,
    /// and expects no configurationDone request (per DAP spec). Sends debug
    /// response after initialized event without waiting for configurationDone.
    async fn mock_backend_no_config_done(mut channel: DuplexChannel) -> anyhow::Result<()> {
        let mut seq: Seq = 1.into();

        while let Ok(Some(msg)) = channel.recv().await {
            if let dap::Message::Request(req) = msg {
                match req.command {
                    RequestCommand::Initialize(_) => {
                        let response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: ResponseBody::Initialize(Some(Capabilities::default())),
                        };
                        seq = seq.next();
                        channel
                            .send(response.into())
                            .await
                            .context("Failed to send initialize response")?;
                    }

                    RequestCommand::Launch(_) | RequestCommand::Attach(_) => {
                        let initialized_event = dap::Event {
                            seq,
                            event: EventKind::Initialized(Default::default()),
                        };
                        seq = seq.next();
                        channel
                            .send(initialized_event.into())
                            .await
                            .context("Failed to send initialized event")?;
                        // Adapter does not support configurationDone, so send debug response now
                        let debug_response_body = match &req.command {
                            RequestCommand::Launch(_) => ResponseBody::Launch,
                            RequestCommand::Attach(_) => ResponseBody::Attach,
                            _ => unreachable!(),
                        };
                        let debug_response = dap::Response {
                            seq,
                            request_seq: req.seq,
                            success: true,
                            message: None,
                            body: debug_response_body,
                        };
                        channel
                            .send(debug_response.into())
                            .await
                            .context("Failed to send debug response")?;
                        return Ok(());
                    }

                    // Invalid: out-of-order request, including unexpected ConfigurationDone
                    cmd => {
                        anyhow::bail!(
                            "Protocol violation: received '{}' (expected no configurationDone)",
                            cmd.command_name()
                        );
                    }
                }
            }
        }

        anyhow::bail!("Channel closed before completion")
    }

    #[tokio::test]
    async fn test_configuration_done_skipped_when_not_supported() {
        // Adapter does NOT advertise supportsConfigurationDoneRequest.
        // SessionInitializer should skip sending configurationDone and still succeed.
        let config = launch_config(vec![], false);

        let (server, client) = DuplexChannel::in_memory(1024);
        let backend_handle = tokio::spawn(mock_backend_no_config_done(server));

        let initializer = SessionInitializer::new(config).with_timeout(Duration::from_secs(1));
        let result = initializer.run(client).await;

        let backend_result = backend_handle.await.expect("backend task panicked");
        assert!(
            backend_result.is_ok(),
            "Mock backend failed (should not have received configurationDone): {:?}",
            backend_result
        );
        assert!(result.is_ok(), "SessionInitializer failed: {:?}", result);
    }

    /// Drive `recv_message` once with `request` (a serialized DAP reverse
    /// request from the backend) and return the failure `Response` the
    /// initializer sends back. Asserts the reply echoes the inbound
    /// `request_seq`, is a failure, and arrives without hanging.
    async fn decline_reverse_request_response(request: serde_json::Value) -> dap::Response {
        let config = launch_config(vec![], false);
        let mut initializer = SessionInitializer::new(config);
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        let req: dap::Request =
            serde_json::from_value(request).expect("valid reverse request JSON");
        let req_seq = req.seq;
        let req_command = req.command_name().to_string();
        server.send(req.into()).await.expect("send reverse request");

        // recv_message processes the request (sending a decline) and returns it.
        tokio::time::timeout(
            Duration::from_secs(5),
            initializer.recv_message(&mut client),
        )
        .await
        .expect("recv_message should not hang")
        .expect("recv_message ok")
        .expect("a message was received");

        let resp_msg = tokio::time::timeout(Duration::from_secs(5), server.recv())
            .await
            .expect("decline response should be sent without hanging")
            .expect("recv ok")
            .expect("a response was received");

        match resp_msg {
            dap::Message::Response(resp) => {
                assert_eq!(
                    resp.request_seq, req_seq,
                    "decline must echo the inbound request_seq"
                );
                assert!(!resp.success, "decline must be a failure response");
                assert!(
                    resp.message
                        .as_deref()
                        .is_some_and(|m| m.contains(&req_command)),
                    "decline must carry a human-readable reason naming the command, got: {:?}",
                    resp.message
                );
                resp
            }
            other => panic!("expected a Response, got {:?}", other.message_type()),
        }
    }

    #[tokio::test]
    async fn test_decline_start_debugging_reverse_request() {
        let resp = decline_reverse_request_response(serde_json::json!({
            "seq": 42,
            "command": "startDebugging",
            "arguments": { "request": "launch", "configuration": {} }
        }))
        .await;
        assert!(
            matches!(resp.body, ResponseBody::StartDebugging),
            "startDebugging decline should use the StartDebugging response body"
        );
    }

    #[tokio::test]
    async fn test_decline_run_in_terminal_reverse_request() {
        let resp = decline_reverse_request_response(serde_json::json!({
            "seq": 7,
            "command": "runInTerminal",
            "arguments": { "cwd": "/tmp", "args": ["echo", "hi"] }
        }))
        .await;
        assert!(
            matches!(resp.body, ResponseBody::RunInTerminal(_)),
            "runInTerminal decline should use the RunInTerminal response body"
        );
    }

    #[tokio::test]
    async fn test_decline_unknown_reverse_request() {
        let resp = decline_reverse_request_response(serde_json::json!({
            "seq": 99,
            "command": "someUnknownReverseRequest",
            "arguments": {}
        }))
        .await;
        match resp.body {
            ResponseBody::Unknown(u) => assert_eq!(u.command, "someUnknownReverseRequest"),
            other => panic!("expected an Unknown response body, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_decline_unexpected_known_request() {
        // A backend should never send `threads` as a reverse request, but the
        // handler is total and must still decline it via the catch-all arm,
        // keyed by the command name.
        let resp = decline_reverse_request_response(serde_json::json!({
            "seq": 5,
            "command": "threads"
        }))
        .await;
        match resp.body {
            ResponseBody::Unknown(u) => assert_eq!(u.command, "threads"),
            other => panic!(
                "expected an Unknown catch-all response body, got {:?}",
                other
            ),
        }
    }

    // ----- startDebugging spawn-handler tests (mpsc-channel seam) -----

    /// A debugpy-style headless config with autoSpawn enabled and a tcp
    /// connect-back rule keyed on `request == "attach"` and
    /// `configuration.connect.{host,port}`.
    fn child_spawn_config() -> DebugSessionConfig {
        serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "python", "args": ["-m", "debugpy.adapter"] },
                "childSessions": {
                    "autoSpawn": true,
                    "maxDepth": 2,
                    "profile": {
                        "rules": [{
                            "when": {
                                "request": "attach",
                                "exists": ["configuration.connect.host", "configuration.connect.port"]
                            },
                            "childBackend": {
                                "type": "tcp",
                                "host": "${configuration.connect.host}",
                                "port": "${configuration.connect.port}"
                            },
                            "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                        }]
                    }
                }
            }"#,
        )
        .unwrap()
    }

    /// A `startDebugging` reverse request that resolves under `child_spawn_config`
    /// (attach to `configuration.connect`).
    fn attach_start_debugging(seq: i64) -> dap::Request {
        serde_json::from_value(serde_json::json!({
            "seq": seq,
            "command": "startDebugging",
            "arguments": {
                "request": "attach",
                "configuration": { "connect": { "host": "127.0.0.1", "port": 5679 } }
            }
        }))
        .expect("valid startDebugging request")
    }

    /// Drive `recv_message` once (bounded) so the handler processes a request.
    async fn drive_recv(initializer: &mut SessionInitializer, client: &mut DuplexChannel) {
        tokio::time::timeout(Duration::from_secs(5), initializer.recv_message(client))
            .await
            .expect("recv_message should not hang")
            .expect("recv_message ok");
    }

    async fn read_response(server: &mut DuplexChannel) -> dap::Response {
        match tokio::time::timeout(Duration::from_secs(5), server.recv())
            .await
            .expect("a response should be sent without hanging")
            .expect("recv ok")
            .expect("a response message")
        {
            dap::Message::Response(resp) => resp,
            other => panic!("expected a Response, got {:?}", other.message_type()),
        }
    }

    #[tokio::test]
    async fn test_start_debugging_spawns_child_and_acks() {
        let (spawn_tx, mut spawn_rx) = mpsc::channel(1);
        let mut initializer =
            SessionInitializer::new(child_spawn_config()).with_child_spawn_tx(spawn_tx);
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        // Fake supervisor: assert the resolved child config, then confirm spawn.
        let supervisor = tokio::spawn(async move {
            let req = spawn_rx.recv().await.expect("a spawn request");
            match &req.config.spawn_config {
                SpawnConfig::Tcp(tcp) => {
                    assert_eq!(
                        tcp.addr,
                        "127.0.0.1:5679".parse().unwrap(),
                        "child should connect to configuration.connect"
                    );
                }
                other => panic!("expected tcp child backend, got {other:?}"),
            }
            req.reply.send(Ok(())).expect("reply delivered");
        });

        server
            .send(attach_start_debugging(50).into())
            .await
            .unwrap();
        drive_recv(&mut initializer, &mut client).await;

        let resp = read_response(&mut server).await;
        assert_eq!(resp.request_seq, Seq(50), "ack must echo request_seq");
        assert!(resp.success, "a confirmed spawn must ack success");
        assert!(matches!(resp.body, ResponseBody::StartDebugging));

        supervisor.await.unwrap();
    }

    #[tokio::test]
    async fn test_start_debugging_spawn_failure_fails_closed() {
        let (spawn_tx, mut spawn_rx) = mpsc::channel(1);
        let mut initializer =
            SessionInitializer::new(child_spawn_config()).with_child_spawn_tx(spawn_tx);
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        let supervisor = tokio::spawn(async move {
            let req = spawn_rx.recv().await.expect("a spawn request");
            req.reply
                .send(Err(anyhow::anyhow!("simulated spawn failure")))
                .expect("reply delivered");
        });

        server
            .send(attach_start_debugging(51).into())
            .await
            .unwrap();
        drive_recv(&mut initializer, &mut client).await;

        let resp = read_response(&mut server).await;
        assert_eq!(resp.request_seq, Seq(51));
        assert!(!resp.success, "a spawn failure must fail closed");
        assert!(matches!(resp.body, ResponseBody::StartDebugging));

        supervisor.await.unwrap();
    }

    #[tokio::test]
    async fn test_start_debugging_resolver_error_fails_closed_without_spawning() {
        let (spawn_tx, mut spawn_rx) = mpsc::channel(1);
        let mut initializer =
            SessionInitializer::new(child_spawn_config()).with_child_spawn_tx(spawn_tx);
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        // request kind "launch" doesn't match the rule (which requires "attach").
        let req: dap::Request = serde_json::from_value(serde_json::json!({
            "seq": 52,
            "command": "startDebugging",
            "arguments": { "request": "launch", "configuration": {} }
        }))
        .unwrap();
        server.send(req.into()).await.unwrap();
        drive_recv(&mut initializer, &mut client).await;

        let resp = read_response(&mut server).await;
        assert!(!resp.success, "an unresolvable request must fail closed");
        assert!(matches!(resp.body, ResponseBody::StartDebugging));
        assert!(
            spawn_rx.try_recv().is_err(),
            "a resolver error must not reach the supervisor"
        );
    }

    #[tokio::test]
    async fn test_start_debugging_max_depth_zero_fails_closed_without_spawning() {
        let config: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "python" },
                "childSessions": { "autoSpawn": true, "maxDepth": 0, "profile": { "rules": [] } }
            }"#,
        )
        .unwrap();
        let (spawn_tx, mut spawn_rx) = mpsc::channel(1);
        let mut initializer = SessionInitializer::new(config).with_child_spawn_tx(spawn_tx);
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        server
            .send(attach_start_debugging(53).into())
            .await
            .unwrap();
        drive_recv(&mut initializer, &mut client).await;

        let resp = read_response(&mut server).await;
        assert!(!resp.success, "max_depth==0 must fail closed");
        assert!(
            spawn_rx.try_recv().is_err(),
            "max_depth==0 must not reach the supervisor"
        );
    }

    #[tokio::test]
    async fn test_start_debugging_without_supervisor_channel_fails_closed() {
        // autoSpawn is on but no spawn channel was installed (a wiring error):
        // fail closed rather than panic, drop, or hang.
        let mut initializer = SessionInitializer::new(child_spawn_config());
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        server
            .send(attach_start_debugging(54).into())
            .await
            .unwrap();
        drive_recv(&mut initializer, &mut client).await;

        let resp = read_response(&mut server).await;
        assert!(
            !resp.success,
            "autoSpawn with no supervisor channel must fail closed"
        );
        assert!(matches!(resp.body, ResponseBody::StartDebugging));
    }

    // ----- supportsStartDebuggingRequest capability gate -----

    #[test]
    fn test_supports_start_debugging_gate() {
        // The capability is advertised only when this process holds a supervisor
        // channel AND the config/profile gates pass; install a dummy channel so
        // each case below exercises its specific config gate, not the
        // missing-channel gate (covered separately at the end).
        fn with_channel(config: DebugSessionConfig) -> SessionInitializer {
            SessionInitializer::new(config).with_child_spawn_tx(mpsc::channel(1).0)
        }

        // debugpy-style config: autoSpawn + a tcp connect-back rule (a literal
        // backend, compatible with any parent) -> advertised on Unix.
        assert_eq!(
            with_channel(child_spawn_config()).supports_start_debugging(),
            cfg!(unix),
            "debugpy-style config should advertise on Unix"
        );

        // No child_sessions -> never advertised.
        assert!(
            !with_channel(launch_config(vec![], false)).supports_start_debugging(),
            "config without child_sessions must not advertise"
        );

        // autoSpawn off -> not advertised.
        let off: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "python" },
                "childSessions": {
                    "autoSpawn": false,
                    "profile": { "rules": [{
                        "when": {},
                        "childBackend": { "type": "inheritParentStdio" },
                        "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                    }] }
                }
            }"#,
        )
        .unwrap();
        assert!(!with_channel(off).supports_start_debugging());

        // maxDepth 0 -> not advertised.
        let depth0: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "python" },
                "childSessions": {
                    "autoSpawn": true,
                    "maxDepth": 0,
                    "profile": { "rules": [{
                        "when": {},
                        "childBackend": { "type": "inheritParentStdio" },
                        "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                    }] }
                }
            }"#,
        )
        .unwrap();
        assert!(!with_channel(depth0).supports_start_debugging());

        // autoSpawn on but no rule whose action fits the parent backend
        // (parentBackend rule + stdio parent) -> not advertised.
        let incompatible: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
                "childSessions": {
                    "autoSpawn": true,
                    "profile": { "rules": [{
                        "when": { "parentBackend": ["tcp", "uds"] },
                        "childBackend": { "type": "parentBackend" },
                        "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                    }] }
                }
            }"#,
        )
        .unwrap();
        assert!(
            !with_channel(incompatible).supports_start_debugging(),
            "a stdio parent with only a parentBackend rule must not advertise"
        );

        // Valid config but NO supervisor channel installed (e.g.
        // `setup_child_supervisor` returned None because `current_exe()` failed)
        // -> not advertised, even though the config/profile gates would pass.
        assert!(
            !SessionInitializer::new(child_spawn_config()).supports_start_debugging(),
            "valid config without a supervisor channel must not advertise"
        );
    }

    #[test]
    fn test_initialize_advertises_capability_only_when_enabled() {
        let enabled = super::requests::initialize(None, true).unwrap();
        match enabled.command {
            RequestCommand::Initialize(args) => {
                assert_eq!(args.supports_start_debugging_request, Some(true));
            }
            other => panic!("expected Initialize, got {other:?}"),
        }

        let disabled = super::requests::initialize(None, false).unwrap();
        match disabled.command {
            RequestCommand::Initialize(args) => {
                assert_eq!(args.supports_start_debugging_request, None);
            }
            other => panic!("expected Initialize, got {other:?}"),
        }

        // Applied even on the full-override path, so an override can't drop it.
        let overridden =
            super::requests::initialize(Some(&serde_json::json!({ "adapterID": "x" })), true)
                .unwrap();
        match overridden.command {
            RequestCommand::Initialize(args) => {
                assert_eq!(args.supports_start_debugging_request, Some(true));
            }
            other => panic!("expected Initialize, got {other:?}"),
        }
    }
}
