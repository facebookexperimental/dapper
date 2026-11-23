// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::convert::identity;
use std::sync::Arc;

use dapper_access::NotPermitted;
use dapper_access::Operation;
use dapper_access::Policy;
use dapper_access::operation_names;
use dapper_access::operations_needed_by;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_dap_protocol::events::EventKind;
use dapper_dap_protocol::events::ExitedEventBody;
use dapper_dap_protocol::events::StoppedEventBody;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_session::BufferedOutput;
use dapper_session::CapabilitiesResult;
use dapper_session::ExecutionStateSummary;
use dapper_session::Expansion;
use dapper_session::NavigateResult;
use dapper_session::NavigationResult;
use dapper_session::NavigationType;
use dapper_session::RawDapResult;
use dapper_session::ResponseContext;
use dapper_session::ScopesResult;
use dapper_session::SetBreakpointsResult;
use dapper_session::SetExceptionBreakpointsResult;
use dapper_session::SetVariableResult;
use dapper_session::StackTraceResult;
use dapper_session::StatusResult;
use dapper_session::ThreadsResult;
use dapper_session::VariablesResult;
use dapper_session::VersionedExecutionStateSummary;
use dapper_session::WaitedEvent;
use dapper_tracing::events::TelemetryEvent;
use enumset::EnumSet;
use serde_json::Value;

use crate::ControlPlaneResult;
use crate::DapperControlPlane;

/// A control plane that refuses the calls `policy` does not allow before they
/// reach `inner`, and keeps the program's values out of what it answers when
/// `policy` does not allow `read_values`.
pub(crate) struct Enforced<T> {
    pub(crate) inner: Arc<T>,
    pub(crate) policy: Policy,
    /// The call carried the session's override token: `policy` is reported
    /// but not enforced.
    pub(crate) overridden: bool,
}

impl<T> Enforced<T> {
    fn check(&self, call: &str, needed: impl Into<EnumSet<Operation>>) -> Result<(), NotPermitted> {
        let checked = self.policy.check(call, needed.into());
        if self.overridden {
            let missing = checked
                .err()
                .map_or(EnumSet::empty(), |refusal| refusal.missing);
            tracing::info!(
                dapper_event = %TelemetryEvent::ControlPlaneCallOverridden,
                call,
                missing = %operation_names(&missing),
                "`{call}` carried the override token, so the access policy does not apply to it"
            );
            return Ok(());
        }
        checked.inspect_err(|refusal| {
            tracing::warn!(
                dapper_event = %TelemetryEvent::ControlPlaneCallRefused,
                call,
                missing = %operation_names(&refusal.missing),
                "{refusal}"
            );
        })
    }

    /// `value`, or what `strip` leaves of it when the policy withholds the
    /// program's values.
    fn shown<V>(&self, value: V, strip: impl FnOnce(V) -> V) -> V {
        if self.overridden || self.policy.allowed().contains(Operation::ReadValues) {
            value
        } else {
            strip(value)
        }
    }
}

fn without_locals(expansion: Expansion) -> Expansion {
    Expansion {
        locals: false,
        ..expansion
    }
}

/// `result` without the program's values: `strip` takes them out of the
/// result itself, [`context_without_values`] out of its context.
fn result_without_values<R>(
    ControlPlaneResult { result, context }: ControlPlaneResult<R>,
    strip: impl FnOnce(R) -> R,
) -> ControlPlaneResult<R> {
    ControlPlaneResult {
        result: strip(result),
        context: context.map(context_without_values),
    }
}

/// `context` without what the program printed, what the adapter said about
/// why it stopped, which may quote its data, and fields this dapper does not
/// know, which could hold anything.
fn context_without_values(context: ResponseContext) -> ResponseContext {
    let ResponseContext {
        session,
        other_sessions,
        execution_state,
        breakpoints,
        installed_exception_filters,
        output: _,
        output_history_file: _,
        extra: _,
    } = context;
    ResponseContext {
        session,
        other_sessions,
        execution_state: execution_state.map(
            |VersionedExecutionStateSummary { version, state }| VersionedExecutionStateSummary {
                version,
                state: state_without_values(state),
            },
        ),
        breakpoints,
        installed_exception_filters,
        output: BufferedOutput::default(),
        output_history_file: None,
        extra: Default::default(),
    }
}

fn state_without_values(state: ExecutionStateSummary) -> ExecutionStateSummary {
    let ExecutionStateSummary {
        status,
        thread_id,
        stop_reason,
        description: _,
        additional_information: _,
    } = state;
    ExecutionStateSummary {
        status,
        thread_id,
        stop_reason,
        description: None,
        additional_information: None,
    }
}

/// `stop` without the adapter's description of it, which may quote the
/// program's data (an exception's message, say).
fn stop_without_values(stop: StoppedEventBody) -> StoppedEventBody {
    let StoppedEventBody {
        reason,
        description: _,
        thread_id,
        preserve_focus_hint,
        text: _,
        all_threads_stopped,
        hit_breakpoint_ids,
        extra: _,
    } = stop;
    StoppedEventBody {
        reason,
        description: None,
        thread_id,
        preserve_focus_hint,
        text: None,
        all_threads_stopped,
        hit_breakpoint_ids,
        extra: Default::default(),
    }
}

/// `exited` with nothing but its exit code.
fn exit_without_values(
    ExitedEventBody {
        exit_code,
        extra: _,
    }: ExitedEventBody,
) -> ExitedEventBody {
    ExitedEventBody {
        exit_code,
        extra: Default::default(),
    }
}

fn navigation_without_values(navigation: NavigationResult) -> NavigationResult {
    let NavigationResult {
        result,
        navigation_type,
        extra: _,
    } = navigation;
    let result = match result {
        NavigateResult::Stopped(stop) => NavigateResult::Stopped(stop_without_values(stop)),
        NavigateResult::Exited(exited) => NavigateResult::Exited(exit_without_values(exited)),
        NavigateResult::Unknown(_) => NavigateResult::Unknown(Value::Null),
        result @ (NavigateResult::CommandExecuted
        | NavigateResult::Terminated
        | NavigateResult::TimedOut { .. }) => result,
    };
    NavigationResult {
        result,
        navigation_type,
        extra: Default::default(),
    }
}

/// `raw` without the program's data in the event it waited for. The response
/// body is left as it is: a request whose response holds values needs
/// `read_values`.
fn raw_without_values(raw: RawDapResult) -> RawDapResult {
    let RawDapResult {
        body,
        event,
        extra: _,
    } = raw;
    RawDapResult {
        body,
        event: event.map(waited_event_without_values),
        extra: Default::default(),
    }
}

/// `event` without the program's data: a stop without its description, an
/// exit with only its code, a termination without the adapter's restart data.
/// A raw request waits only for those, so any other event is dropped rather
/// than vetted.
fn waited_event_without_values(event: WaitedEvent) -> WaitedEvent {
    match event {
        WaitedEvent::Received(EventKind::Stopped(stop)) => {
            WaitedEvent::Received(EventKind::Stopped(stop_without_values(stop)))
        }
        WaitedEvent::Received(EventKind::Exited(exited)) => {
            WaitedEvent::Received(EventKind::Exited(exit_without_values(exited)))
        }
        WaitedEvent::Received(EventKind::Terminated(_)) => {
            WaitedEvent::Received(EventKind::Terminated(None))
        }
        timed_out @ WaitedEvent::TimedOut { .. } => timed_out,
        WaitedEvent::Received(_) | WaitedEvent::Unknown(_) => WaitedEvent::Unknown(Value::Null),
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
        let result = self
            .inner
            .threads(self.shown(expansion, without_locals))
            .await?;
        Ok(self.shown(result, |result| result_without_values(result, identity)))
    }

    async fn stack_trace(
        &self,
        thread_id: ThreadId,
        start_frame: Option<i64>,
        levels: Option<i64>,
        expansion: Expansion,
    ) -> anyhow::Result<ControlPlaneResult<StackTraceResult>> {
        self.check("stack_trace", Operation::Inspect)?;
        let result = self
            .inner
            .stack_trace(
                thread_id,
                start_frame,
                levels,
                self.shown(expansion, without_locals),
            )
            .await?;
        Ok(self.shown(result, |result| result_without_values(result, identity)))
    }

    async fn scopes(
        &self,
        frame_id: FrameId,
        expansion: Expansion,
    ) -> anyhow::Result<ControlPlaneResult<ScopesResult>> {
        self.check("scopes", Operation::Inspect)?;
        let result = self
            .inner
            .scopes(frame_id, self.shown(expansion, without_locals))
            .await?;
        Ok(self.shown(result, |result| result_without_values(result, identity)))
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
        let result = self
            .inner
            .navigate(navigation_type, thread_id, single_thread)
            .await?;
        Ok(self.shown(result, |result| {
            result_without_values(result, navigation_without_values)
        }))
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
        let result = self
            .inner
            .set_breakpoints(source_path, clear_existing, breakpoint_specs)
            .await?;
        Ok(self.shown(result, |result| result_without_values(result, identity)))
    }

    async fn set_exception_breakpoints(
        &self,
        filters: &[String],
        clear_existing: bool,
    ) -> anyhow::Result<ControlPlaneResult<SetExceptionBreakpointsResult>> {
        self.check("set_exception_breakpoints", Operation::Breakpoints)?;
        let result = self
            .inner
            .set_exception_breakpoints(filters, clear_existing)
            .await?;
        Ok(self.shown(result, |result| result_without_values(result, identity)))
    }

    async fn send_dap_request(
        &self,
        command: RequestCommand,
        wait_for_event: bool,
        timeout_seconds: u64,
    ) -> anyhow::Result<RawDapResult> {
        self.check(command.command_name(), operations_needed_by(&command))?;
        let result = self
            .inner
            .send_dap_request(command, wait_for_event, timeout_seconds)
            .await?;
        Ok(self.shown(result, raw_without_values))
    }

    async fn capabilities(&self) -> anyhow::Result<ControlPlaneResult<CapabilitiesResult>> {
        self.check("capabilities", EnumSet::empty())?;
        let result = self.inner.capabilities().await?;
        Ok(self.shown(result, |result| result_without_values(result, identity)))
    }

    async fn status(&self) -> anyhow::Result<ControlPlaneResult<StatusResult>> {
        self.check("status", EnumSet::empty())?;
        let ControlPlaneResult { result, context } = self.inner.status().await?;
        let result = StatusResult {
            access: Some(self.policy.into()),
            ..result
        };
        Ok(
            self.shown(ControlPlaneResult { result, context }, |result| {
                result_without_values(result, identity)
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use dapper_dap_protocol::data_types::BreakpointId;
    use dapper_dap_protocol::enums::StoppedReason;
    use dapper_dap_protocol::events::OutputEventBody;
    use dapper_dap_protocol::events::TerminatedEventBody;
    use dapper_dap_protocol::responses::ResponseBody;
    use dapper_session::BreakpointInfo;
    use dapper_session::ExecutionStatus;
    use dapper_session::OutputEvent;
    use indexmap::IndexMap;
    use serde_json::json;

    use super::*;
    use crate::test_log::logged_by;

    fn stop_with_a_message() -> StoppedEventBody {
        StoppedEventBody {
            reason: StoppedReason::Exception,
            description: Some("ValueError: card 4111 rejected".to_owned()),
            thread_id: Some(ThreadId(3)),
            text: Some("card 4111 rejected".to_owned()),
            hit_breakpoint_ids: Some(vec![BreakpointId(9)]),
            extra: [("vendorDetail".to_owned(), json!("4111"))]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_context_without_values_keeps_the_program_structure() {
        let breakpoints = HashMap::from([(
            "/src/pay.py".to_owned(),
            vec![BreakpointInfo {
                line: 12,
                verified: true,
                ..Default::default()
            }],
        )]);
        let context = ResponseContext {
            execution_state: Some(VersionedExecutionStateSummary {
                version: 4,
                state: ExecutionStateSummary {
                    status: ExecutionStatus::Stopped,
                    thread_id: Some(ThreadId(3)),
                    stop_reason: Some(StoppedReason::Exception),
                    description: Some("ValueError: card 4111 rejected".to_owned()),
                    additional_information: Some("card 4111".to_owned()),
                },
            }),
            breakpoints: breakpoints.clone(),
            output: BufferedOutput {
                head: vec![OutputEvent {
                    output: "charging 4111\n".to_owned(),
                    ..Default::default()
                }],
                total_count: 1,
                ..Default::default()
            },
            output_history_file: Some("/tmp/dapper-output.log".into()),
            extra: [("lastExceptionMessage".to_owned(), json!("card 4111"))]
                .into_iter()
                .collect(),
            ..Default::default()
        };

        assert_eq!(
            context_without_values(context),
            ResponseContext {
                execution_state: Some(VersionedExecutionStateSummary {
                    version: 4,
                    state: ExecutionStateSummary {
                        status: ExecutionStatus::Stopped,
                        thread_id: Some(ThreadId(3)),
                        stop_reason: Some(StoppedReason::Exception),
                        description: None,
                        additional_information: None,
                    },
                }),
                breakpoints,
                ..Default::default()
            },
            "the program's output, the stop's description and unknown fields go; where it stopped and why stays"
        );
    }

    fn unknown_fields() -> IndexMap<String, Value> {
        [("vendorDetail".to_owned(), json!("card 4111"))]
            .into_iter()
            .collect()
    }

    #[test]
    fn a_navigation_without_values_keeps_where_it_stopped() {
        let navigation = NavigationResult {
            result: NavigateResult::Stopped(stop_with_a_message()),
            navigation_type: NavigationType::Continue,
            extra: unknown_fields(),
        };

        let stripped = navigation_without_values(navigation);

        assert!(
            stripped.extra.is_empty(),
            "unknown fields could hold anything"
        );
        let NavigateResult::Stopped(stop) = stripped.result else {
            panic!("a stop should stay a stop");
        };
        assert_eq!(
            stop,
            StoppedEventBody {
                reason: StoppedReason::Exception,
                thread_id: Some(ThreadId(3)),
                hit_breakpoint_ids: Some(vec![BreakpointId(9)]),
                ..Default::default()
            }
        );
        assert_eq!(
            navigation_without_values(NavigationResult {
                result: NavigateResult::Exited(ExitedEventBody {
                    exit_code: 3,
                    extra: unknown_fields(),
                }),
                navigation_type: NavigationType::Continue,
                extra: IndexMap::new(),
            })
            .result,
            NavigateResult::Exited(ExitedEventBody {
                exit_code: 3,
                extra: IndexMap::new(),
            }),
            "an exit keeps only its code"
        );
    }

    #[test]
    fn a_raw_result_without_values_keeps_its_response() {
        let raw = RawDapResult {
            body: ResponseBody::Pause,
            event: Some(WaitedEvent::Received(EventKind::Stopped(
                stop_with_a_message(),
            ))),
            extra: unknown_fields(),
        };

        let stripped = raw_without_values(raw.clone());

        assert_eq!(stripped.body, raw.body);
        assert!(
            stripped.extra.is_empty(),
            "unknown fields could hold anything"
        );
        let Some(WaitedEvent::Received(EventKind::Stopped(stop))) = stripped.event else {
            panic!("the waited-for stop should stay");
        };
        assert_eq!((stop.description, stop.text), (None, None));
    }

    #[test]
    fn a_raw_result_without_values_keeps_only_what_it_waits_for() {
        let printed = OutputEventBody {
            output: "charging card 4111\n".to_owned(),
            ..Default::default()
        };
        for (event, expected, why) in [
            (
                WaitedEvent::Received(EventKind::Exited(ExitedEventBody {
                    exit_code: 3,
                    extra: unknown_fields(),
                })),
                WaitedEvent::Received(EventKind::Exited(ExitedEventBody {
                    exit_code: 3,
                    extra: IndexMap::new(),
                })),
                "an exit keeps only its code",
            ),
            (
                WaitedEvent::Received(EventKind::Terminated(Some(TerminatedEventBody {
                    restart: Some(json!({"card": "4111"})),
                    extra: IndexMap::new(),
                }))),
                WaitedEvent::Received(EventKind::Terminated(None)),
                "a termination loses the adapter's restart data",
            ),
            (
                WaitedEvent::Received(EventKind::Output(printed)),
                WaitedEvent::Unknown(Value::Null),
                "a raw request never waits for output, so it is dropped",
            ),
            (
                WaitedEvent::Unknown(json!({"text": "card 4111"})),
                WaitedEvent::Unknown(Value::Null),
                "an event dapper cannot read could hold anything",
            ),
            (
                WaitedEvent::TimedOut {
                    timeout_seconds: 60,
                },
                WaitedEvent::TimedOut {
                    timeout_seconds: 60,
                },
                "a timeout holds no data",
            ),
        ] {
            assert_eq!(waited_event_without_values(event), expected, "{why}");
        }
    }

    #[test]
    fn a_refusal_is_logged_as_a_telemetry_event() {
        let enforced = Enforced {
            inner: Arc::new(()),
            policy: Policy::try_new(EnumSet::only(Operation::Inspect))
                .expect("inspect alone is a whole policy"),
            overridden: false,
        };

        let logged = logged_by(|| {
            enforced
                .check("threads", Operation::Inspect)
                .expect("inspect is allowed");
            enforced
                .check("writeMemory", Operation::WriteMemory)
                .expect_err("write_memory is not allowed");
        });

        assert_eq!(
            logged.lines().count(),
            1,
            "only the refusal is logged: {logged}"
        );
        for field in [
            "dapper_event=control_plane_call_refused",
            "call=\"writeMemory\"",
            "missing=write_memory",
        ] {
            assert!(logged.contains(field), "`{field}` should be in: {logged}");
        }
    }

    #[test]
    fn every_call_made_with_the_override_token_is_logged() {
        let overridden = Enforced {
            inner: Arc::new(()),
            policy: Policy::try_new(EnumSet::only(Operation::Inspect))
                .expect("inspect alone is a whole policy"),
            overridden: true,
        };

        let logged = logged_by(|| {
            for (call, needed) in [
                ("status", EnumSet::empty()),
                ("threads", EnumSet::only(Operation::Inspect)),
                ("stop", EnumSet::only(Operation::Stop)),
            ] {
                overridden
                    .check(call, needed)
                    .expect("the override token lets every call through");
            }
        });

        let lines: Vec<&str> = logged.lines().collect();
        assert_eq!(lines.len(), 3, "each call should be logged: {logged}");
        for (line, call, missing) in [
            (lines[0], "status", "nothing"),
            (lines[1], "threads", "nothing"),
            (lines[2], "stop", "stop"),
        ] {
            for field in [
                "dapper_event=control_plane_call_overridden".to_owned(),
                format!("call=\"{call}\""),
                format!("missing={missing}"),
            ] {
                assert!(line.contains(&field), "`{field}` should be in: {line}");
            }
        }
    }
}
