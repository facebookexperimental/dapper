// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;

use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::enums::StoppedReason;
use dapper_dap_protocol::events::EventKind;
use dapper_dap_protocol::events::StoppedEventBody;
use dapper_dap_protocol::protocol::Message;
use dapper_dap_protocol::requests::ContinueArguments;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::requests::ReverseContinueArguments;
use dapper_dap_protocol::responses::ResponseBody;

use super::tracker_inner::DebugSessionTrackerInner;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingExecutionRequest {
    pub thread_id: ThreadId,
    pub single_thread: Option<bool>,
}

impl From<&ContinueArguments> for PendingExecutionRequest {
    fn from(args: &ContinueArguments) -> Self {
        Self {
            thread_id: args.thread_id,
            single_thread: args.single_thread,
        }
    }
}

impl From<&ReverseContinueArguments> for PendingExecutionRequest {
    fn from(args: &ReverseContinueArguments) -> Self {
        Self {
            thread_id: args.thread_id,
            single_thread: args.single_thread,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemainderStatus {
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StopInfo {
    pub thread_id: Option<ThreadId>,
    pub reason: StoppedReason,
    pub description: Option<String>,
    pub additional_information: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionStatus {
    Unknown,
    Live {
        running_threads: HashSet<ThreadId>,
        stopped_threads: HashSet<ThreadId>,
        remainder_status: RemainderStatus,
        stop_info: Option<StopInfo>,
    },
    Exited,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionState {
    pub current: ExecutionStatus,
    pub supports_single_thread_execution: bool,
    pub pending_execution_requests: HashMap<Seq, PendingExecutionRequest>,
    pending_restart_seq: Option<Seq>,
    stopped_during_restart: bool,
    version: u64,
}

impl Default for ExecutionState {
    fn default() -> Self {
        Self {
            current: ExecutionStatus::Unknown,
            supports_single_thread_execution: false,
            pending_execution_requests: HashMap::new(),
            pending_restart_seq: None,
            stopped_during_restart: false,
            version: 1,
        }
    }
}

impl ExecutionState {
    pub fn is_all_running(&self) -> bool {
        match &self.current {
            ExecutionStatus::Live {
                stopped_threads,
                remainder_status,
                ..
            } => stopped_threads.is_empty() && *remainder_status == RemainderStatus::Running,
            _ => false,
        }
    }

    pub fn is_all_stopped(&self) -> bool {
        match &self.current {
            ExecutionStatus::Live {
                running_threads,
                remainder_status,
                ..
            } => running_threads.is_empty() && *remainder_status == RemainderStatus::Stopped,
            _ => false,
        }
    }

    pub fn any_thread_stopped(&self) -> bool {
        match &self.current {
            ExecutionStatus::Live {
                stopped_threads,
                remainder_status,
                ..
            } => !stopped_threads.is_empty() || *remainder_status == RemainderStatus::Stopped,
            _ => false,
        }
    }

    pub fn is_thread_stopped(&self, thread_id: ThreadId) -> bool {
        match &self.current {
            ExecutionStatus::Live {
                running_threads,
                stopped_threads,
                remainder_status,
                ..
            } => {
                if stopped_threads.contains(&thread_id) {
                    return true;
                }
                if running_threads.contains(&thread_id) {
                    return false;
                }
                *remainder_status == RemainderStatus::Stopped
            }
            _ => false,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn summary(&self) -> dapper_session::VersionedExecutionStateSummary {
        let status = match &self.current {
            ExecutionStatus::Unknown => dapper_session::ExecutionStatus::Unknown,
            ExecutionStatus::Exited => dapper_session::ExecutionStatus::Exited,
            ExecutionStatus::Live { .. } => {
                if self.is_all_running() {
                    dapper_session::ExecutionStatus::Running
                } else {
                    dapper_session::ExecutionStatus::Stopped
                }
            }
        };
        let (thread_id, stop_reason, description, additional_information) = match &self.current {
            ExecutionStatus::Live {
                stop_info: Some(info),
                ..
            } => (
                info.thread_id,
                Some(info.reason.clone()),
                info.description.clone(),
                info.additional_information.clone(),
            ),
            _ => (None, None, None, None),
        };
        dapper_session::VersionedExecutionStateSummary {
            version: self.version,
            state: dapper_session::ExecutionStateSummary {
                status,
                thread_id,
                stop_reason,
                description,
                additional_information,
            },
        }
    }

    pub fn set_all_running(&mut self) {
        match &mut self.current {
            ExecutionStatus::Live {
                running_threads,
                stopped_threads,
                remainder_status,
                stop_info,
            } => {
                running_threads.clear();
                stopped_threads.clear();
                *remainder_status = RemainderStatus::Running;
                *stop_info = None;
            }
            _ => {
                self.current = ExecutionStatus::Live {
                    running_threads: HashSet::new(),
                    stopped_threads: HashSet::new(),
                    remainder_status: RemainderStatus::Running,
                    stop_info: None,
                };
            }
        }
        self.version += 1;
    }

    pub fn set_all_stopped(&mut self, stopped: &StoppedEventBody) {
        match &mut self.current {
            ExecutionStatus::Live {
                running_threads,
                stopped_threads,
                remainder_status,
                stop_info,
            } => {
                running_threads.clear();
                stopped_threads.clear();
                if let Some(tid) = stopped.thread_id {
                    stopped_threads.insert(tid);
                }
                *remainder_status = RemainderStatus::Stopped;
                Self::update_stop_info(stop_info, stopped);
            }
            _ => {
                let mut new_stop_info = None;
                Self::update_stop_info(&mut new_stop_info, stopped);
                let mut stopped_threads = HashSet::new();
                if let Some(tid) = stopped.thread_id {
                    stopped_threads.insert(tid);
                }
                self.current = ExecutionStatus::Live {
                    running_threads: HashSet::new(),
                    stopped_threads,
                    remainder_status: RemainderStatus::Stopped,
                    stop_info: new_stop_info,
                };
            }
        }
        self.version += 1;
    }

    pub(super) fn track_message_from_client(
        inner: &Mutex<DebugSessionTrackerInner>,
        message: &Message,
    ) {
        if let Message::Request(request) = message {
            match &request.command {
                RequestCommand::Launch(_) | RequestCommand::Attach(_) => {
                    Self::with_execution_state(inner, |this| {
                        this.set_all_running();
                    });
                }
                RequestCommand::Restart(_) => {
                    Self::with_execution_state(inner, |this| {
                        this.pending_restart_seq = Some(request.seq);
                        this.stopped_during_restart = false;
                    });
                }
                RequestCommand::Continue(args) => {
                    Self::with_execution_state(inner, |this| {
                        this.pending_execution_requests
                            .insert(request.seq, PendingExecutionRequest::from(args));
                    });
                }
                RequestCommand::ReverseContinue(args) => {
                    Self::with_execution_state(inner, |this| {
                        this.pending_execution_requests
                            .insert(request.seq, PendingExecutionRequest::from(args));
                    });
                }
                _ => {}
            }
        }
    }

    pub(super) fn track_message_to_client(
        inner: &Mutex<DebugSessionTrackerInner>,
        message: &Message,
    ) {
        match message {
            Message::Response(response) => match &response.body {
                ResponseBody::Initialize(caps) => {
                    Self::with_execution_state(inner, |this| {
                        if response.success {
                            this.supports_single_thread_execution = caps
                                .as_ref()
                                .and_then(|c| c.supports_single_thread_execution_requests)
                                .unwrap_or(false);

                            tracing::debug!(
                                supports_single_thread_execution =
                                    this.supports_single_thread_execution,
                                has_capabilities = caps.is_some(),
                                "Captured adapter capabilities from initialize response"
                            );
                        }
                    });
                }
                ResponseBody::Continue(body) => {
                    Self::with_execution_state(inner, |this| {
                        let pending = this
                            .pending_execution_requests
                            .remove(&response.request_seq);
                        if response.success
                            && let Some(pending) = pending
                        {
                            let all_threads_continued = body.all_threads_continued.unwrap_or(true);
                            if all_threads_continued {
                                this.set_all_running();
                            } else {
                                this.mark_thread_running(pending.thread_id);
                            }
                            tracing::debug!(
                                command = %response.command_name(),
                                thread_id = pending.thread_id.as_i64(),
                                state = ?this.current,
                                "Execution state changed from response"
                            );
                        }
                    });
                }
                ResponseBody::ReverseContinue => {
                    Self::with_execution_state(inner, |this| {
                        let pending = this
                            .pending_execution_requests
                            .remove(&response.request_seq);
                        if response.success
                            && let Some(pending) = pending
                        {
                            // Infer allThreadsContinued from capabilities and request args
                            // Per DAP spec: if adapter doesn't support single-thread execution,
                            // or if singleThread wasn't requested, all threads continue
                            let all_threads_continued = !this.supports_single_thread_execution
                                || !pending.single_thread.unwrap_or(false);

                            if all_threads_continued {
                                this.set_all_running();
                            } else {
                                this.mark_thread_running(pending.thread_id);
                            }

                            tracing::debug!(
                                command = "reverseContinue",
                                thread_id = pending.thread_id.as_i64(),
                                single_thread = ?pending.single_thread,
                                supports_single_thread = this.supports_single_thread_execution,
                                all_threads_continued = all_threads_continued,
                                state = ?this.current,
                                "Execution state changed from reverseContinue response"
                            );
                        }
                    });
                }
                ResponseBody::Restart => {
                    Self::with_execution_state(inner, |this| {
                        if response.success {
                            if !this.stopped_during_restart {
                                this.set_all_running();
                            }
                            this.pending_execution_requests.clear();
                        }
                        tracing::debug!(
                            command = "restart",
                            success = response.success,
                            stopped_during_restart = this.stopped_during_restart,
                            state = ?this.current,
                            "Execution state after restart response"
                        );
                        this.pending_restart_seq = None;
                        this.stopped_during_restart = false;
                    });
                }
                _ => {}
            },
            Message::Event(event) => match &event.event {
                EventKind::Stopped(stopped) => {
                    Self::with_execution_state(inner, |this| {
                        if this.pending_restart_seq.is_some() {
                            this.stopped_during_restart = true;
                        }

                        let all_stopped = stopped.all_threads_stopped.unwrap_or(false); // the default is false, according to the DAP spec.

                        if all_stopped {
                            this.set_all_stopped(stopped);
                        } else {
                            this.mark_thread_stopped(stopped);
                        }
                    });
                }
                EventKind::Continued(continued) => {
                    Self::with_execution_state(inner, |this| {
                        let thread_id = continued.thread_id;
                        let all_continued = continued.all_threads_continued.unwrap_or(true); // the default is true, according to the DAP spec.

                        if all_continued {
                            this.set_all_running();
                        } else {
                            this.mark_thread_running(thread_id);
                        }
                    });
                }
                EventKind::Exited(_) | EventKind::Terminated(_) => {
                    Self::with_execution_state(inner, |this| {
                        this.current = ExecutionStatus::Exited;
                        this.version += 1;
                    });
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn is_valid_stopped_event(stopped: &StoppedEventBody) -> bool {
        if stopped.reason != StoppedReason::Exception {
            return true;
        }
        let description = match stopped.description.as_ref() {
            Some(desc) => desc.to_lowercase(),
            None => return false,
        };
        // Known issues in lldb-dap:
        // Attach to core files could result in spurious stopped events with reason "exception" and description "signal 0" from non crashing threads
        // Attach to running processes with "stopOnEntry": true, could result in spurious stopped events with reason "exception" and description "signal SIGSTOP" for all threads
        !description.contains("signal 0") && !description.contains("signal sigstop")
    }

    fn update_stop_info(stop_info: &mut Option<StopInfo>, stopped: &StoppedEventBody) {
        if Self::is_valid_stopped_event(stopped) {
            *stop_info = Some(StopInfo {
                thread_id: stopped.thread_id,
                reason: stopped.reason.clone(),
                description: stopped.description.clone(),
                additional_information: stopped.text.clone(),
            });
        }
    }

    fn mark_thread_stopped(&mut self, stopped: &StoppedEventBody) {
        match &mut self.current {
            ExecutionStatus::Live {
                running_threads,
                stopped_threads,
                remainder_status,
                stop_info,
            } => {
                if let Some(tid) = stopped.thread_id {
                    running_threads.remove(&tid);
                    if *remainder_status == RemainderStatus::Running {
                        stopped_threads.insert(tid);
                    }
                } else {
                    tracing::warn!(
                        "Received stopped event without allThreadsStopped=true and without threadId"
                    );
                }
                Self::update_stop_info(stop_info, stopped);
            }
            _ => {
                // unreachable, but we ignore the error instead of calling
                // unreachable!(), just in case someone do crazy things
                // outside of spec. the state remains in the Unknown or Exited
                // state so it's okay
            }
        }
        self.version += 1;
    }

    fn mark_thread_running(&mut self, thread_id: ThreadId) {
        match &mut self.current {
            ExecutionStatus::Live {
                running_threads,
                stopped_threads,
                remainder_status,
                ..
            } => {
                stopped_threads.remove(&thread_id);
                if *remainder_status == RemainderStatus::Stopped {
                    running_threads.insert(thread_id);
                }
                self.version += 1;
            }
            _ => {
                // unreachable, but we ignore the error instead of calling
                // unreachable!(), just in case someone do crazy things
                // outside of spec. the state remains in the Unknown or Exited
                // state so it's okay
            }
        }
    }

    fn with_execution_state(
        inner: &Mutex<DebugSessionTrackerInner>,
        f: impl FnOnce(&mut ExecutionState),
    ) {
        match inner.lock() {
            Ok(mut guard) => {
                f(&mut guard.execution_state);
                tracing::trace!(state = ?guard.execution_state, "Execution state changed");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to acquire lock for inner to update execution state");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;

    use dapper_dap_protocol::capabilities::Capabilities;
    use dapper_dap_protocol::data_types::Seq;
    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::enums::StoppedReason;
    use dapper_dap_protocol::events::ContinuedEventBody;
    use dapper_dap_protocol::events::EventKind;
    use dapper_dap_protocol::events::ExitedEventBody;
    use dapper_dap_protocol::events::StoppedEventBody;
    use dapper_dap_protocol::protocol::Event;
    use dapper_dap_protocol::protocol::Message;
    use dapper_dap_protocol::protocol::Request;
    use dapper_dap_protocol::protocol::Response;
    use dapper_dap_protocol::requests::ContinueArguments;
    use dapper_dap_protocol::requests::LaunchRequestArguments;
    use dapper_dap_protocol::requests::RequestCommand;
    use dapper_dap_protocol::requests::RestartArguments;
    use dapper_dap_protocol::requests::ReverseContinueArguments;
    use dapper_dap_protocol::responses::ContinueResponseBody;
    use dapper_dap_protocol::responses::ResponseBody;

    use super::*;

    fn make_test_inner() -> Mutex<DebugSessionTrackerInner> {
        let session_id: dapper_session::SessionId = "test".into();
        Mutex::new(DebugSessionTrackerInner::new(&session_id, 0))
    }

    fn es(inner: &Mutex<DebugSessionTrackerInner>) -> ExecutionState {
        inner.lock().unwrap().execution_state.clone()
    }

    fn track_from(inner: &Mutex<DebugSessionTrackerInner>, message: &Message) {
        ExecutionState::track_message_from_client(inner, message);
    }

    fn track_to(inner: &Mutex<DebugSessionTrackerInner>, message: &Message) {
        ExecutionState::track_message_to_client(inner, message);
    }

    fn stop_info(inner: &Mutex<DebugSessionTrackerInner>) -> Option<StopInfo> {
        match &es(inner).current {
            ExecutionStatus::Live { stop_info, .. } => stop_info.clone(),
            _ => None,
        }
    }

    fn breakpoint_stop_info_for(thread_id: Option<ThreadId>) -> Option<StopInfo> {
        Some(StopInfo {
            thread_id,
            reason: StoppedReason::Breakpoint,
            description: None,
            additional_information: None,
        })
    }

    #[test]
    fn test_execution_state_initial() {
        let state = ExecutionState::default();
        assert_eq!(state.current, ExecutionStatus::Unknown);
        assert!(!state.is_all_stopped());
    }

    #[test]
    fn test_execution_state_after_launch() {
        let inner = make_test_inner();

        let request = Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };

        track_from(&inner, &Message::Request(request));
        assert!(es(&inner).is_all_running());
        assert!(!es(&inner).is_all_stopped());
    }

    #[test]
    fn test_execution_state_stopped_event() {
        let inner = make_test_inner();

        let event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };

        track_to(&inner, &Message::Event(event));
        assert!(es(&inner).is_all_stopped());
    }

    #[test]
    fn test_execution_state_continued_event() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let continued_event = Event {
            seq: 101.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: 1.into(),
                all_threads_continued: None,
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(continued_event));
        assert!(es(&inner).is_all_running());
        assert!(!es(&inner).is_all_stopped());
    }

    #[test]
    fn test_execution_state_exited_event() {
        let inner = make_test_inner();

        let event = Event {
            seq: 100.into(),
            event: EventKind::Exited(ExitedEventBody {
                exit_code: 0,
                ..Default::default()
            }),
        };

        track_to(&inner, &Message::Event(event));
        assert_eq!(es(&inner).current, ExecutionStatus::Exited);
        assert!(!es(&inner).is_all_stopped());
    }

    #[test]
    fn test_execution_state_terminated_event() {
        let inner = make_test_inner();

        let event = Event {
            seq: 100.into(),
            event: EventKind::Terminated(None),
        };

        track_to(&inner, &Message::Event(event));
        assert_eq!(es(&inner).current, ExecutionStatus::Exited);
    }

    #[test]
    fn test_partial_stop() {
        let inner = make_test_inner();

        let launch_request = Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(launch_request));

        let event1 = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(event1));
        assert_eq!(
            es(&inner).current,
            ExecutionStatus::Live {
                running_threads: HashSet::new(),
                stopped_threads: HashSet::from([1.into()]),
                remainder_status: RemainderStatus::Running,
                stop_info: breakpoint_stop_info_for(Some(1.into())),
            }
        );

        let event2 = Event {
            seq: 101.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(3.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(event2));
        assert_eq!(
            es(&inner).current,
            ExecutionStatus::Live {
                running_threads: HashSet::new(),
                stopped_threads: HashSet::from([1.into(), 3.into()]),
                remainder_status: RemainderStatus::Running,
                stop_info: breakpoint_stop_info_for(Some(3.into())),
            }
        );
    }

    #[test]
    fn test_partial_continue() {
        let inner = make_test_inner();

        let launch_request = Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(launch_request));

        let stop1 = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stop1));

        let stop2 = Event {
            seq: 101.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(3.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stop2));
        assert_eq!(
            es(&inner).current,
            ExecutionStatus::Live {
                running_threads: HashSet::new(),
                stopped_threads: HashSet::from([1.into(), 3.into()]),
                remainder_status: RemainderStatus::Running,
                stop_info: breakpoint_stop_info_for(Some(3.into())),
            }
        );

        let cont = Event {
            seq: 102.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: 1.into(),
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(cont));
        assert_eq!(
            es(&inner).current,
            ExecutionStatus::Live {
                running_threads: HashSet::new(),
                stopped_threads: HashSet::from([3.into()]),
                remainder_status: RemainderStatus::Running,
                stop_info: breakpoint_stop_info_for(Some(3.into())),
            }
        );
    }

    #[test]
    fn test_all_continued_clears_stopped_set() {
        let inner = make_test_inner();

        let launch_request = Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(launch_request));

        let stop1 = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stop1));

        let stop2 = Event {
            seq: 101.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(3.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stop2));
        assert_eq!(
            es(&inner).current,
            ExecutionStatus::Live {
                running_threads: HashSet::new(),
                stopped_threads: HashSet::from([1.into(), 3.into()]),
                remainder_status: RemainderStatus::Running,
                stop_info: breakpoint_stop_info_for(Some(3.into())),
            }
        );

        let cont = Event {
            seq: 102.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: 1.into(),
                all_threads_continued: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(cont));
        assert!(es(&inner).is_all_running());
    }

    #[test]
    fn test_all_stopped_then_continue_single_thread_keeps_others_stopped() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let continued_event = Event {
            seq: 101.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: 1.into(),
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(continued_event));

        let state = es(&inner);
        assert_eq!(
            state.current,
            ExecutionStatus::Live {
                running_threads: HashSet::from([1.into()]),
                stopped_threads: HashSet::new(),
                remainder_status: RemainderStatus::Stopped,
                stop_info: breakpoint_stop_info_for(Some(1.into())),
            }
        );
        assert!(!state.is_thread_stopped(1.into()));
        assert!(state.is_thread_stopped(2.into()));
        assert!(state.is_thread_stopped(99.into()));
        assert!(!state.is_all_stopped());
        assert!(state.any_thread_stopped());
    }

    #[test]
    fn test_all_stopped_then_continue_multiple_threads_sequentially() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));

        let cont1 = Event {
            seq: 101.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: 1.into(),
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(cont1));

        let cont2 = Event {
            seq: 102.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: 2.into(),
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(cont2));

        let state = es(&inner);
        assert_eq!(
            state.current,
            ExecutionStatus::Live {
                running_threads: HashSet::from([1.into(), 2.into()]),
                stopped_threads: HashSet::new(),
                remainder_status: RemainderStatus::Stopped,
                stop_info: breakpoint_stop_info_for(Some(1.into())),
            }
        );
        assert!(!state.is_thread_stopped(1.into()));
        assert!(!state.is_thread_stopped(2.into()));
        assert!(state.is_thread_stopped(3.into()));
    }

    #[test]
    fn test_partial_stop_then_all_stop_then_partial_continue() {
        let inner = make_test_inner();

        let launch_request = Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(launch_request));

        let partial_stop = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(partial_stop));
        assert_eq!(
            es(&inner).current,
            ExecutionStatus::Live {
                running_threads: HashSet::new(),
                stopped_threads: HashSet::from([1.into()]),
                remainder_status: RemainderStatus::Running,
                stop_info: breakpoint_stop_info_for(Some(1.into())),
            }
        );

        let all_stop = Event {
            seq: 101.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(2.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(all_stop));
        assert!(es(&inner).is_all_stopped());

        let partial_cont = Event {
            seq: 102.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: 5.into(),
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(partial_cont));

        let state = es(&inner);
        assert!(!state.is_thread_stopped(5.into()));
        assert!(state.is_thread_stopped(1.into()));
        assert!(state.is_thread_stopped(2.into()));
        assert!(state.any_thread_stopped());
        assert!(!state.is_all_stopped());
    }

    #[test]
    fn test_continue_request_response_transitions_to_running() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let continue_request = Request {
            seq: 10.into(),
            command: RequestCommand::Continue(ContinueArguments {
                thread_id: 1.into(),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(continue_request));

        let continue_response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: true,
            message: None,
            body: ResponseBody::Continue(ContinueResponseBody {
                all_threads_continued: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Response(continue_response));

        assert!(es(&inner).is_all_running());
        assert!(!es(&inner).is_all_stopped());
    }

    #[test]
    fn test_failed_continue_response_does_not_transition_state() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let continue_request = Request {
            seq: 10.into(),
            command: RequestCommand::Continue(ContinueArguments {
                thread_id: 1.into(),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(continue_request));

        let failed_response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: false,
            message: Some("Thread not found".to_string()),
            body: ResponseBody::Continue(ContinueResponseBody {
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Response(failed_response));

        assert!(es(&inner).is_all_stopped());
    }

    #[test]
    fn test_continue_response_partial_threads_continued() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));

        let continue_request = Request {
            seq: 10.into(),
            command: RequestCommand::Continue(ContinueArguments {
                thread_id: 1.into(),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(continue_request));

        let continue_response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: true,
            message: None,
            body: ResponseBody::Continue(ContinueResponseBody {
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Response(continue_response));

        let state = es(&inner);
        assert!(!state.is_thread_stopped(1.into()));
        assert!(state.is_thread_stopped(2.into()));
        assert!(state.any_thread_stopped());
        assert!(!state.is_all_stopped());
    }

    #[test]
    fn test_captures_single_thread_execution_capability() {
        let inner = make_test_inner();
        assert!(!es(&inner).supports_single_thread_execution);

        let init_response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Initialize(Some(Capabilities {
                supports_single_thread_execution_requests: Some(true),
                ..Default::default()
            })),
        };
        track_to(&inner, &Message::Response(init_response));

        assert!(es(&inner).supports_single_thread_execution);
    }

    #[test]
    fn test_capability_defaults_to_false_when_missing() {
        let inner = make_test_inner();

        let init_response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Initialize(Some(Capabilities {
                ..Default::default()
            })),
        };
        track_to(&inner, &Message::Response(init_response));

        assert!(!es(&inner).supports_single_thread_execution);
    }

    #[test]
    fn test_capability_defaults_to_false_when_no_body() {
        let inner = make_test_inner();

        let init_response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Initialize(None),
        };
        track_to(&inner, &Message::Response(init_response));

        assert!(!es(&inner).supports_single_thread_execution);
    }

    #[test]
    fn test_reverse_continue_request_tracked() {
        let inner = make_test_inner();

        let request = Request {
            seq: 10.into(),
            command: RequestCommand::ReverseContinue(ReverseContinueArguments {
                thread_id: 1.into(),
                single_thread: Some(true),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(request));

        let state = es(&inner);
        assert!(state.pending_execution_requests.contains_key(&Seq(10)));
        let pending = state.pending_execution_requests.get(&Seq(10)).unwrap();
        assert_eq!(pending.thread_id, ThreadId(1));
        assert_eq!(pending.single_thread, Some(true));
    }

    #[test]
    fn test_reverse_continue_without_single_thread_support_continues_all() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let request = Request {
            seq: 10.into(),
            command: RequestCommand::ReverseContinue(ReverseContinueArguments {
                thread_id: 1.into(),
                single_thread: Some(true),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(request));

        let response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: true,
            message: None,
            body: ResponseBody::ReverseContinue,
        };
        track_to(&inner, &Message::Response(response));

        assert!(es(&inner).is_all_running());
    }

    #[test]
    fn test_reverse_continue_with_single_thread_support_but_single_thread_false() {
        let inner = make_test_inner();

        let init_response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Initialize(Some(Capabilities {
                supports_single_thread_execution_requests: Some(true),
                ..Default::default()
            })),
        };
        track_to(&inner, &Message::Response(init_response));

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));

        let request = Request {
            seq: 10.into(),
            command: RequestCommand::ReverseContinue(ReverseContinueArguments {
                thread_id: 1.into(),
                single_thread: Some(false),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(request));

        let response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: true,
            message: None,
            body: ResponseBody::ReverseContinue,
        };
        track_to(&inner, &Message::Response(response));

        assert!(es(&inner).is_all_running());
    }

    #[test]
    fn test_reverse_continue_with_single_thread_support_and_single_thread_true() {
        let inner = make_test_inner();

        let init_response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Initialize(Some(Capabilities {
                supports_single_thread_execution_requests: Some(true),
                ..Default::default()
            })),
        };
        track_to(&inner, &Message::Response(init_response));

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));

        let request = Request {
            seq: 10.into(),
            command: RequestCommand::ReverseContinue(ReverseContinueArguments {
                thread_id: 1.into(),
                single_thread: Some(true),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(request));

        let response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: true,
            message: None,
            body: ResponseBody::ReverseContinue,
        };
        track_to(&inner, &Message::Response(response));

        let state = es(&inner);
        assert!(!state.is_thread_stopped(1.into()));
        assert!(state.is_thread_stopped(2.into()));
        assert!(state.any_thread_stopped());
    }

    #[test]
    fn test_reverse_continue_failed_response_does_not_change_state() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let request = Request {
            seq: 10.into(),
            command: RequestCommand::ReverseContinue(ReverseContinueArguments {
                thread_id: 1.into(),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(request));

        let response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: false,
            message: Some("Reverse debugging not supported".to_string()),
            body: ResponseBody::ReverseContinue,
        };
        track_to(&inner, &Message::Response(response));

        assert!(es(&inner).is_all_stopped());
    }

    #[test]
    fn test_restart_response_resets_to_running_and_clears_pending() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let continue_request = Request {
            seq: 10.into(),
            command: RequestCommand::Continue(ContinueArguments {
                thread_id: 1.into(),
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(continue_request));
        assert!(!es(&inner).pending_execution_requests.is_empty());

        let restart_request = Request {
            seq: 20.into(),
            command: RequestCommand::Restart(Some(RestartArguments {
                ..Default::default()
            })),
        };
        track_from(&inner, &Message::Request(restart_request));

        let restart_response = Response {
            seq: 200.into(),
            request_seq: 20.into(),
            success: true,
            message: None,
            body: ResponseBody::Restart,
        };
        track_to(&inner, &Message::Response(restart_response));

        assert!(es(&inner).is_all_running());
        assert!(es(&inner).pending_execution_requests.is_empty());
    }

    #[test]
    fn test_failed_restart_response_does_not_change_state() {
        let inner = make_test_inner();

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let restart_response = Response {
            seq: 200.into(),
            request_seq: 20.into(),
            success: false,
            message: Some("Restart not supported".to_string()),
            body: ResponseBody::Restart,
        };
        track_to(&inner, &Message::Response(restart_response));

        assert!(es(&inner).is_all_stopped());
    }

    #[test]
    fn test_restart_with_stopped_event_before_response_preserves_stopped_state() {
        let inner = make_test_inner();

        let launch_request = Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(launch_request));
        assert!(es(&inner).is_all_running());

        let restart_request = Request {
            seq: 10.into(),
            command: RequestCommand::Restart(Some(RestartArguments {
                ..Default::default()
            })),
        };
        track_from(&inner, &Message::Request(restart_request));

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Entry,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));
        assert!(es(&inner).is_all_stopped());

        let restart_response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: true,
            message: None,
            body: ResponseBody::Restart,
        };
        track_to(&inner, &Message::Response(restart_response));

        assert!(es(&inner).is_all_stopped());
        assert!(!es(&inner).is_all_running());
    }

    #[test]
    fn test_restart_with_partial_stopped_event_before_response() {
        let inner = make_test_inner();

        let launch_request = Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        };
        track_from(&inner, &Message::Request(launch_request));

        let restart_request = Request {
            seq: 10.into(),
            command: RequestCommand::Restart(Some(RestartArguments {
                ..Default::default()
            })),
        };
        track_from(&inner, &Message::Request(restart_request));

        let stopped_event = Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(1.into()),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        };
        track_to(&inner, &Message::Event(stopped_event));

        let restart_response = Response {
            seq: 200.into(),
            request_seq: 10.into(),
            success: true,
            message: None,
            body: ResponseBody::Restart,
        };
        track_to(&inner, &Message::Response(restart_response));

        assert_eq!(
            es(&inner).current,
            ExecutionStatus::Live {
                running_threads: HashSet::new(),
                stopped_threads: HashSet::from([1.into()]),
                remainder_status: RemainderStatus::Running,
                stop_info: breakpoint_stop_info_for(Some(1.into())),
            }
        );
        assert!(!es(&inner).is_all_running());
    }

    #[test]
    fn test_is_valid_stopped_event() {
        let stopped = StoppedEventBody {
            reason: StoppedReason::Exception,
            description: Some("signal 0".to_string()),
            ..Default::default()
        };
        assert!(!ExecutionState::is_valid_stopped_event(&stopped));

        let stopped = StoppedEventBody {
            reason: StoppedReason::Exception,
            description: Some("signal SIGSEGV".to_string()),
            ..Default::default()
        };
        assert!(ExecutionState::is_valid_stopped_event(&stopped));

        let stopped = StoppedEventBody {
            reason: StoppedReason::Exception,
            description: Some("signal SIGSTOP".to_string()),
            ..Default::default()
        };
        assert!(!ExecutionState::is_valid_stopped_event(&stopped));

        let stopped = StoppedEventBody {
            reason: StoppedReason::Breakpoint,
            description: Some("breakpoint 2.1".to_string()),
            ..Default::default()
        };
        assert!(ExecutionState::is_valid_stopped_event(&stopped));

        let stopped = StoppedEventBody {
            reason: StoppedReason::Step,
            description: Some("step over".to_string()),
            ..Default::default()
        };
        assert!(ExecutionState::is_valid_stopped_event(&stopped));
    }

    #[test]
    fn test_stop_info_set_on_valid_stopped_event() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let event = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(5)),
                description: Some("hit breakpoint".to_string()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &event);

        let info = stop_info(&inner).expect("should have stop_info");
        assert_eq!(info.thread_id, Some(ThreadId(5)));
        assert_eq!(info.reason, StoppedReason::Breakpoint);
        assert_eq!(info.description, Some("hit breakpoint".to_string()));
    }

    #[test]
    fn test_stop_info_not_set_on_spurious_event() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let valid_event = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Exception,
                thread_id: Some(ThreadId(1)),
                description: Some("signal SIGSEGV".to_string()),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &valid_event);
        assert!(stop_info(&inner).is_some());

        let spurious_event = Message::Event(Event {
            seq: 101.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Exception,
                thread_id: Some(ThreadId(2)),
                description: Some("signal 0".to_string()),
                ..Default::default()
            }),
        });
        track_to(&inner, &spurious_event);

        let info = stop_info(&inner).expect("should still have original");
        assert_eq!(info.description, Some("signal SIGSEGV".to_string()));
    }

    #[test]
    fn test_stop_info_cleared_on_set_all_running() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let event = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &event);
        assert!(stop_info(&inner).is_some());

        inner.lock().unwrap().execution_state.set_all_running();
        assert!(stop_info(&inner).is_none());
    }

    #[test]
    fn test_stop_info_cleared_on_continue() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let stopped = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped);
        assert!(stop_info(&inner).is_some());

        let continue_req = Message::Request(Request {
            seq: Seq(10),
            command: RequestCommand::Continue(ContinueArguments {
                thread_id: ThreadId(1),
                ..Default::default()
            }),
        });
        track_from(&inner, &continue_req);

        let continue_resp = Message::Response(Response {
            seq: 200.into(),
            request_seq: Seq(10),
            success: true,
            message: None,
            body: ResponseBody::Continue(ContinueResponseBody {
                all_threads_continued: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &continue_resp);

        assert!(
            stop_info(&inner).is_none(),
            "stop_info should be cleared after continue"
        );
        assert!(es(&inner).is_all_running());
    }

    #[test]
    fn test_stop_info_cleared_on_exited() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let stopped = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped);
        assert!(stop_info(&inner).is_some());

        let exited = Message::Event(Event {
            seq: 101.into(),
            event: EventKind::Exited(ExitedEventBody {
                exit_code: 0,
                ..Default::default()
            }),
        });
        track_to(&inner, &exited);

        assert!(
            stop_info(&inner).is_none(),
            "stop_info should be cleared on exit"
        );
        assert_eq!(es(&inner).current, ExecutionStatus::Exited);
    }

    #[test]
    fn test_stop_info_preserved_on_single_thread_resume() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let stopped = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped);
        assert!(stop_info(&inner).is_some());

        let continued = Message::Event(Event {
            seq: 101.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: ThreadId(2),
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        });
        track_to(&inner, &continued);

        assert!(
            stop_info(&inner).is_some(),
            "stop_info should be preserved when only a single thread resumes"
        );
    }

    fn version(inner: &Mutex<DebugSessionTrackerInner>) -> u64 {
        es(inner).version()
    }

    #[test]
    fn test_version_starts_at_one() {
        let inner = make_test_inner();
        assert_eq!(version(&inner), 1);
    }

    #[test]
    fn test_version_increments_on_stopped() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);
        let v_after_launch = version(&inner);

        let stopped = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped);
        assert!(
            version(&inner) > v_after_launch,
            "version should increment after stopped event"
        );
    }

    #[test]
    fn test_version_increments_on_continued() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let stopped = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped);
        let v_after_stopped = version(&inner);

        let continued = Message::Event(Event {
            seq: 101.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: ThreadId(1),
                all_threads_continued: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &continued);
        assert!(
            version(&inner) > v_after_stopped,
            "version should increment after continued event"
        );
    }

    #[test]
    fn test_version_increments_on_exited() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);
        let v_after_launch = version(&inner);

        let exited = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Exited(ExitedEventBody {
                exit_code: 0,
                ..Default::default()
            }),
        });
        track_to(&inner, &exited);
        assert!(
            version(&inner) > v_after_launch,
            "version should increment after exited event"
        );
    }

    #[test]
    fn test_version_increments_monotonically() {
        let inner = make_test_inner();
        let mut prev = version(&inner);

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let stopped = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped);
        let v = version(&inner);
        assert!(v > prev, "version should increase after stopped");
        prev = v;

        let stopped2 = Message::Event(Event {
            seq: 101.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(2)),
                all_threads_stopped: Some(false),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped2);
        let v = version(&inner);
        assert!(v > prev, "version should increase after second stopped");
        prev = v;

        let continued = Message::Event(Event {
            seq: 102.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: ThreadId(1),
                all_threads_continued: Some(false),
                ..Default::default()
            }),
        });
        track_to(&inner, &continued);
        let v = version(&inner);
        assert!(v > prev, "version should increase after partial continue");
        prev = v;

        let continued_all = Message::Event(Event {
            seq: 103.into(),
            event: EventKind::Continued(ContinuedEventBody {
                thread_id: ThreadId(2),
                all_threads_continued: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &continued_all);
        let v = version(&inner);
        assert!(v > prev, "version should increase after all continued");
        prev = v;

        let exited = Message::Event(Event {
            seq: 104.into(),
            event: EventKind::Exited(ExitedEventBody {
                exit_code: 0,
                ..Default::default()
            }),
        });
        track_to(&inner, &exited);
        let v = version(&inner);
        assert!(v > prev, "version should increase after exited");
    }

    #[test]
    fn test_version_unchanged_on_unrelated_events() {
        let inner = make_test_inner();

        let launch = Message::Request(Request {
            seq: 1.into(),
            command: RequestCommand::Launch(LaunchRequestArguments {
                ..Default::default()
            }),
        });
        track_from(&inner, &launch);

        let stopped = Message::Event(Event {
            seq: 100.into(),
            event: EventKind::Stopped(StoppedEventBody {
                reason: StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                all_threads_stopped: Some(true),
                ..Default::default()
            }),
        });
        track_to(&inner, &stopped);
        let v = version(&inner);

        let unrelated = Message::Event(Event {
            seq: 101.into(),
            event: EventKind::Output(dapper_dap_protocol::events::OutputEventBody {
                output: "hello".to_string(),
                ..Default::default()
            }),
        });
        track_to(&inner, &unrelated);

        assert_eq!(
            version(&inner),
            v,
            "version should not change on output events"
        );
    }
}
