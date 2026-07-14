// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;

use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::Scope;
use dapper_dap_protocol::data_types::StackFrame;
use dapper_dap_protocol::data_types::Thread;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::Variable;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_dap_protocol::events::EventKind;
use dapper_dap_protocol::events::ExitedEventBody;
use dapper_dap_protocol::events::StoppedEventBody;
use dapper_dap_protocol::responses::ResponseBody;
use dapper_dap_protocol::responses::SetVariableResponseBody;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::BreakpointInfo;
use crate::ExceptionFilterEntry;
use crate::NavigationType;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawDapResult {
    /// Intentionally not `#[serde(default)]`: a result without a body is invalid.
    pub body: ResponseBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<WaitedEvent>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing `Received` would require broader public API updates"
)]
pub enum WaitedEvent {
    Received(EventKind),
    TimedOut {
        timeout_seconds: u64,
    },
    #[serde(untagged)]
    Unknown(Value),
}

impl RawDapResult {
    fn body_value(&self) -> Option<serde_json::Value> {
        extract_response_body(&self.body)
    }

    fn to_json_value(&self) -> serde_json::Value {
        let body_value = self.body_value();

        match &self.event {
            Some(WaitedEvent::Received(event_kind)) => {
                serde_json::json!({
                    "response": body_value,
                    "event": {
                        "type": event_kind.event_name(),
                        "body": extract_event_body(event_kind)
                    }
                })
            }
            Some(WaitedEvent::TimedOut { timeout_seconds }) => {
                serde_json::json!({
                    "response": body_value,
                    "note": format!("Timed out waiting for event after {}s", timeout_seconds)
                })
            }
            Some(WaitedEvent::Unknown(v)) => {
                serde_json::json!({
                    "response": body_value,
                    "event": v
                })
            }
            None => match body_value {
                Some(body) => body,
                None => serde_json::json!({}),
            },
        }
    }

    pub fn render_json(&self) -> String {
        self.to_json_value().to_string()
    }
}

fn extract_response_body(body: &ResponseBody) -> Option<serde_json::Value> {
    match body {
        ResponseBody::Unknown(u) => u.body.clone(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.get("body").cloned()),
    }
}

fn extract_event_body(event: &EventKind) -> Option<serde_json::Value> {
    match event {
        EventKind::Unknown(u) => u.body.clone(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.get("body").cloned()),
    }
}

impl fmt::Display for RawDapResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `Value`'s `Display` is infallible; `{:#}` is its pretty format.
        write!(f, "{:#}", self.to_json_value())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesResult {
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default)]
    pub variables_reference: VariablesReference,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for VariablesResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Variables for reference {}:", self.variables_reference)?;

        if self.variables.is_empty() {
            writeln!(f, "  No variables found")?;
        } else {
            for variable in &self.variables {
                writeln!(f, "  {}", variable)?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesResult {
    #[serde(default)]
    pub scopes: Vec<Scope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locals: Option<Vec<Variable>>,
    #[serde(default)]
    pub frame_id: FrameId,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for ScopesResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Scopes for frame {}:", self.frame_id)?;

        if self.scopes.is_empty() {
            writeln!(f, "  No scopes found")?;
            return Ok(());
        }

        for scope in &self.scopes {
            writeln!(f, "  {}", scope)?;

            if !scope.is_locals() {
                continue;
            }

            match &self.locals {
                Some(locals) if locals.is_empty() => {
                    writeln!(f, "    (no variables)")?;
                }
                Some(locals) => {
                    for variable in locals {
                        writeln!(f, "    {}", variable)?;
                    }
                }
                None => {}
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceResult {
    #[serde(default)]
    pub stack_frames: Vec<StackFrame>,
    #[serde(default)]
    pub start_frame: i64,
    #[serde(default)]
    pub has_more_frames: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<ScopesResult>,
    #[serde(default)]
    pub thread_id: ThreadId,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for StackTraceResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let start = self.start_frame as usize;
        let end = start + self.stack_frames.len().saturating_sub(1);

        writeln!(
            f,
            "Stack trace (frames {} - {}) for thread {}:",
            start, end, self.thread_id
        )?;

        if self.stack_frames.is_empty() {
            writeln!(f, "  No stack frames found")?;
            return Ok(());
        }

        if self.has_more_frames {
            writeln!(
                f,
                "(frames omitted after #{}, use 'debug_stack_trace_command' to request more)",
                end,
            )?;
        }

        for (index, frame) in self.stack_frames.iter().enumerate() {
            writeln!(f, "  {}", frame.format_with_index(start + index))?;
        }

        if let Some(scopes_result) = &self.scopes {
            write!(f, "\n{}", scopes_result)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsResult {
    #[serde(default, serialize_with = "serialize_sessions_without_debugger_args")]
    pub sessions: Vec<dapper_session::SessionInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<dapper_session::ScopeId>,
}

/// Serialize sessions as camelCase, excluding `debugger_args`.
fn serialize_sessions_without_debugger_args<S: serde::Serializer>(
    sessions: &[dapper_session::SessionInfo],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(serde::Serialize)]
    #[serde(remote = "dapper_session::SessionInfo", rename_all = "camelCase")]
    struct Def {
        session_id: dapper_session::SessionId,
        pid: u32,
        control_plane_port: Option<dapper_session::Port>,
        started_at: i64,
        command_line_args: Vec<String>,
        current_working_directory: Option<std::path::PathBuf>,
        scope_id: Option<dapper_session::ScopeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_type: Option<dapper_session::RequestType>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        debuggee_process_id: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_session_id: Option<dapper_session::SessionId>,
        #[serde(default, skip_serializing)]
        debugger_args: Option<serde_json::Value>,
    }

    struct Wrap<'a>(&'a dapper_session::SessionInfo);
    impl serde::Serialize for Wrap<'_> {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            Def::serialize(self.0, s)
        }
    }
    serializer.collect_seq(sessions.iter().map(Wrap))
}

impl fmt::Display for SessionsResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.sessions.is_empty() {
            write!(
                f,
                "No active sessions found{}.",
                self.scope_id
                    .as_ref()
                    .map_or(String::new(), |s| format!(" in scope '{}'", s))
            )?;
            return Ok(());
        }

        writeln!(
            f,
            "Found {} active session(s){}:\n",
            self.sessions.len(),
            self.scope_id
                .as_ref()
                .map_or(String::new(), |s| format!(" in scope '{}'", s))
        )?;

        for (i, session) in self.sessions.iter().enumerate() {
            write!(f, "{}", session)?;
            if i < self.sessions.len() - 1 {
                writeln!(f)?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsResult {
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<StackTraceResult>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for ThreadsResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Threads:")?;

        if self.threads.is_empty() {
            writeln!(f, "  No threads found")?;
            return Ok(());
        }

        for thread in &self.threads {
            writeln!(f, "  {}", thread)?;
        }

        if let Some(stack_trace_result) = &self.stack_trace {
            write!(f, "\n{}", stack_trace_result)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetVariableResult {
    #[serde(default)]
    pub body: SetVariableResponseBody,
    #[serde(default)]
    pub name: String,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for SetVariableResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let type_info = self
            .body
            .type_
            .as_deref()
            .map(|t| format!(" ({})", t))
            .unwrap_or_default();

        let variables_reference = self.body.variables_reference.unwrap_or_default();
        let child_ref = if variables_reference.has_children() {
            format!(" [ref: {}]", variables_reference)
        } else {
            String::new()
        };

        write!(
            f,
            "Variable '{}' set successfully\n  {}: {}{}{}\n",
            self.name, self.name, self.body.value, type_info, child_ref
        )
    }
}

// Adjacently tagged (`{"type": "stopped", "data": {...}}`) because the variants
// carry heterogeneous payloads that need a stable discriminator for deserialization.
// WaitedEvent above uses externally tagged because its variant names are already
// unambiguous wrappers (Received/TimedOut).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum NavigateResult {
    CommandExecuted,
    Stopped(StoppedEventBody),
    Exited(ExitedEventBody),
    Terminated,
    TimedOut {
        timeout_seconds: Option<u64>,
    },
    #[serde(untagged)]
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigationResult {
    pub result: NavigateResult,
    pub navigation_type: NavigationType,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for NavigationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.result {
            NavigateResult::CommandExecuted => {
                write!(f, "{}", self.navigation_type.command_success_description())
            }
            NavigateResult::Stopped(stopped) => {
                write!(f, "Execution stopped: {}", stopped.reason)
            }
            NavigateResult::Exited(exited) => {
                write!(f, "Program exited with code: {}", exited.exit_code)
            }
            NavigateResult::Terminated => {
                write!(f, "Program terminated")
            }
            NavigateResult::TimedOut { timeout_seconds } => {
                // Match exhaustively so adding a new NavigationType is a
                // compile error here rather than a silent mislabel.
                let command_name = match self.navigation_type {
                    NavigationType::Continue => "Continue",
                    NavigationType::Pause => "Pause",
                    NavigationType::ReverseContinue => "Reverse continue",
                    // Step variants (forward and back) skip the wait branch
                    // in ProxyClient::navigate today, so they should not
                    // reach TimedOut. Display impls must stay total — we
                    // render a sentence-case fallback rather than panicking,
                    // so a programmatic NavigateResult constructor (or a
                    // future refactor) cannot crash a render path. Listed
                    // explicitly so adding a new NavigationType forces an
                    // intentional decision here.
                    NavigationType::StepIn => "Step in",
                    NavigationType::StepOver => "Step over",
                    NavigationType::StepOut => "Step out",
                    NavigationType::StepBack => "Step back",
                };
                match timeout_seconds {
                    Some(secs) => write!(
                        f,
                        "{} command executed, program still running after {} seconds",
                        command_name, secs
                    ),
                    None => {
                        write!(
                            f,
                            "{} command executed, program still running",
                            command_name
                        )
                    }
                }
            }
            NavigateResult::Unknown(v) => write!(f, "Unknown navigation result: {}", v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResult {
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for StatusResult {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsResult {
    #[serde(default)]
    pub breakpoints: Vec<BreakpointInfo>,
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub new_count: usize,
    #[serde(default)]
    pub existing_count: usize,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for SetBreakpointsResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.existing_count > 0 {
            writeln!(
                f,
                "Appended {} new breakpoints to existing {} breakpoints in {}:",
                self.new_count, self.existing_count, self.source_path
            )?;
        } else {
            writeln!(
                f,
                "Set {} breakpoints in {}:",
                self.new_count, self.source_path
            )?;
        }

        for bp in &self.breakpoints {
            let verified_str = if bp.verified {
                "Verified:"
            } else {
                "Not Verified:"
            };
            writeln!(f, "  {} Line {}", verified_str, bp.line)?;
        }

        Ok(())
    }
}

/// Result of a control-plane `set_exception_breakpoints` call. `installed`
/// reflects the post-sanitization effective set actually carried in the DAP
/// request (with conditions dropped where the adapter didn't support them);
/// callers should use this rather than the input filter list when reasoning
/// about post-call state.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetExceptionBreakpointsResult {
    #[serde(default)]
    pub installed: Vec<ExceptionFilterEntry>,
    /// Number of *unique* explicit filters in the request that weren't
    /// already in the installed set (so they were genuinely new).
    /// Intra-request duplicates are counted once. Note also that with
    /// `clear_existing=true` every explicit filter counts as new (no
    /// continuity with the prior installed set).
    #[serde(default)]
    pub new_count: usize,
    /// Number of *unique* explicit filters in the request that were
    /// already in the installed set (so the request was a no-op for
    /// them). Always 0 when `clear_existing=true`. Intra-request
    /// duplicates are counted once.
    #[serde(default)]
    pub existing_count: usize,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl fmt::Display for SetExceptionBreakpointsResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.installed.len();
        if self.existing_count > 0 {
            writeln!(
                f,
                "Added {} new exception breakpoint filter(s) to existing {} ({} total):",
                self.new_count, self.existing_count, total
            )?;
        } else {
            writeln!(f, "Set {} exception breakpoint filter(s):", total)?;
        }

        for entry in &self.installed {
            match &entry.condition {
                Some(cond) => writeln!(f, "  {} (condition: {})", entry.filter, cond)?,
                None => writeln!(f, "  {}", entry.filter)?,
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::data_types::FrameId;
    use dapper_dap_protocol::data_types::Scope;
    use dapper_dap_protocol::data_types::StackFrame;
    use dapper_dap_protocol::data_types::Thread;
    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::data_types::VariablesReference;
    use dapper_dap_protocol::enums::ScopePresentationHint;
    use dapper_dap_protocol::events::ExitedEventBody;
    use dapper_dap_protocol::events::StoppedEventBody;
    use dapper_dap_protocol::events::UnknownEvent;
    use dapper_dap_protocol::responses::ThreadsResponseBody;
    use dapper_dap_protocol::responses::UnknownResponseBody;

    use super::*;

    #[test]
    fn display_unknown_response_body_only() {
        let result = RawDapResult {
            body: ResponseBody::Unknown(UnknownResponseBody {
                command: "threads".to_string(),
                body: Some(serde_json::json!({"threads": [{"id": 1, "name": "main"}]})),
                extra: Default::default(),
            }),
            event: None,
            extra: Default::default(),
        };

        let output = result.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(parsed.get("threads").and_then(|t| t.as_array()).is_some());
    }

    #[test]
    fn display_known_response_body_only() {
        let result = RawDapResult {
            body: ResponseBody::Threads(ThreadsResponseBody {
                threads: vec![Thread {
                    id: ThreadId(1),
                    name: "main".to_string(),
                }],
                ..Default::default()
            }),
            event: None,
            extra: Default::default(),
        };

        let output = result.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        let threads = parsed
            .get("threads")
            .and_then(|t| t.as_array())
            .expect("threads array");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0]["id"], 1);
    }

    #[test]
    fn display_bodyless_response() {
        let result = RawDapResult {
            body: ResponseBody::Pause,
            event: None,
            extra: Default::default(),
        };

        assert_eq!(result.to_string(), "{}");
    }

    #[test]
    fn render_json_is_compact_and_parseable() {
        let result = RawDapResult {
            body: ResponseBody::Threads(ThreadsResponseBody {
                threads: vec![Thread {
                    id: ThreadId(1),
                    name: "main".to_string(),
                }],
                ..Default::default()
            }),
            event: None,
            extra: Default::default(),
        };

        let output = result.render_json();
        assert!(
            !output.contains('\n'),
            "compact rendering should be a single line, got: {output}"
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["threads"][0]["id"], 1);
    }

    #[test]
    fn display_response_with_received_event() {
        let result = RawDapResult {
            body: ResponseBody::Unknown(UnknownResponseBody {
                command: "continue".to_string(),
                body: Some(serde_json::json!({"allThreadsContinued": true})),
                extra: Default::default(),
            }),
            event: Some(WaitedEvent::Received(EventKind::Stopped(
                StoppedEventBody {
                    reason: dapper_dap_protocol::enums::StoppedReason::Breakpoint,
                    thread_id: Some(ThreadId(1)),
                    ..Default::default()
                },
            ))),
            extra: Default::default(),
        };

        let output = result.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(parsed.get("response").is_some());
        assert!(parsed.get("event").is_some());
        assert_eq!(parsed["event"]["type"], "stopped");
        assert!(parsed["event"]["body"].get("reason").is_some());
    }

    #[test]
    fn display_response_with_exited_event() {
        let result = RawDapResult {
            body: ResponseBody::Unknown(UnknownResponseBody {
                command: "continue".to_string(),
                body: None,
                extra: Default::default(),
            }),
            event: Some(WaitedEvent::Received(EventKind::Exited(ExitedEventBody {
                exit_code: 0,
                ..Default::default()
            }))),
            extra: Default::default(),
        };

        let output = result.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["event"]["type"], "exited");
        assert_eq!(parsed["event"]["body"]["exitCode"], 0);
    }

    #[test]
    fn display_response_with_unknown_event() {
        let result = RawDapResult {
            body: ResponseBody::Pause,
            event: Some(WaitedEvent::Received(EventKind::Unknown(UnknownEvent {
                event: "customEvent".to_string(),
                body: Some(serde_json::json!({"data": 42})),
                extra: Default::default(),
            }))),
            extra: Default::default(),
        };

        let output = result.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["event"]["type"], "customEvent");
        assert_eq!(parsed["event"]["body"]["data"], 42);
    }

    #[test]
    fn display_response_with_timeout() {
        let result = RawDapResult {
            body: ResponseBody::Unknown(UnknownResponseBody {
                command: "continue".to_string(),
                body: Some(serde_json::json!({"allThreadsContinued": true})),
                extra: Default::default(),
            }),
            event: Some(WaitedEvent::TimedOut {
                timeout_seconds: 30,
            }),
            extra: Default::default(),
        };

        let output = result.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(parsed.get("response").is_some());
        assert_eq!(parsed["note"], "Timed out waiting for event after 30s");
    }

    #[test]
    fn serde_round_trip() {
        let original = RawDapResult {
            body: ResponseBody::Threads(ThreadsResponseBody {
                threads: vec![Thread {
                    id: ThreadId(1),
                    name: "main".to_string(),
                }],
                ..Default::default()
            }),
            event: Some(WaitedEvent::Received(EventKind::Stopped(
                StoppedEventBody {
                    reason: dapper_dap_protocol::enums::StoppedReason::Breakpoint,
                    thread_id: Some(ThreadId(1)),
                    ..Default::default()
                },
            ))),
            extra: Default::default(),
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: RawDapResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn serde_round_trip_with_timeout() {
        let original = RawDapResult {
            body: ResponseBody::Pause,
            event: Some(WaitedEvent::TimedOut {
                timeout_seconds: 60,
            }),
            extra: Default::default(),
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: RawDapResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn format_variables_empty() {
        let result = VariablesResult {
            variables: vec![],
            variables_reference: VariablesReference(42),
            ..Default::default()
        };
        assert_eq!(
            result.to_string(),
            "Variables for reference 42:\n  No variables found\n"
        );
    }

    #[test]
    fn format_variables_single() {
        let result = VariablesResult {
            variables: vec![Variable {
                name: "x".to_string(),
                value: "10".to_string(),
                var_type: Some("int".to_string()),
                ..Default::default()
            }],
            variables_reference: VariablesReference(1),
            ..Default::default()
        };
        assert_eq!(
            result.to_string(),
            "Variables for reference 1:\n  x: 10 (int)\n"
        );
    }

    #[test]
    fn format_variables_multiple_with_child_ref() {
        let result = VariablesResult {
            variables: vec![
                Variable {
                    name: "a".to_string(),
                    value: "hello".to_string(),
                    ..Default::default()
                },
                Variable {
                    name: "b".to_string(),
                    value: "{}".to_string(),
                    var_type: Some("MyStruct".to_string()),
                    variables_reference: VariablesReference(99),
                    ..Default::default()
                },
            ],
            variables_reference: VariablesReference(5),
            ..Default::default()
        };
        assert_eq!(
            result.to_string(),
            "Variables for reference 5:\n  a: hello\n  b: {} (MyStruct) [ref: 99]\n"
        );
    }

    #[test]
    fn format_scopes_empty() {
        let result = ScopesResult {
            scopes: vec![],
            locals: None,
            frame_id: FrameId(0),
            ..Default::default()
        };
        assert_eq!(
            result.to_string(),
            "Scopes for frame 0:\n  No scopes found\n"
        );
    }

    #[test]
    fn format_scopes_without_locals_expansion() {
        let result = ScopesResult {
            scopes: vec![
                Scope {
                    name: "Locals".to_string(),
                    presentation_hint: Some(ScopePresentationHint::Locals),
                    variables_reference: VariablesReference(10),
                    ..Default::default()
                },
                Scope {
                    name: "Arguments".to_string(),
                    presentation_hint: Some(ScopePresentationHint::Arguments),
                    variables_reference: VariablesReference(11),
                    ..Default::default()
                },
            ],
            locals: None,
            frame_id: FrameId(1),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Scopes for frame 1:"));
        assert!(output.contains("Scope: Locals"));
        assert!(output.contains("Scope: Arguments"));
        assert!(!output.contains("(no variables)"));
    }

    #[test]
    fn format_scopes_with_expanded_locals() {
        let result = ScopesResult {
            scopes: vec![Scope {
                name: "Locals".to_string(),
                presentation_hint: Some(ScopePresentationHint::Locals),
                variables_reference: VariablesReference(10),
                ..Default::default()
            }],
            locals: Some(vec![
                Variable {
                    name: "x".to_string(),
                    value: "42".to_string(),
                    var_type: Some("int".to_string()),
                    ..Default::default()
                },
                Variable {
                    name: "y".to_string(),
                    value: "hello".to_string(),
                    ..Default::default()
                },
            ]),
            frame_id: FrameId(1),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Scope: Locals"));
        assert!(output.contains("    x: 42 (int)\n"));
        assert!(output.contains("    y: hello\n"));
    }

    #[test]
    fn format_scopes_with_empty_locals() {
        let result = ScopesResult {
            scopes: vec![Scope {
                name: "Locals".to_string(),
                presentation_hint: Some(ScopePresentationHint::Locals),
                variables_reference: VariablesReference(10),
                ..Default::default()
            }],
            locals: Some(vec![]),
            frame_id: FrameId(2),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Scope: Locals"));
        assert!(output.contains("(no variables)"));
    }

    fn make_frame(id: i64, name: &str) -> StackFrame {
        StackFrame {
            id: FrameId(id),
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn format_stack_trace_empty() {
        let result = StackTraceResult {
            stack_frames: vec![],
            start_frame: 0,
            has_more_frames: false,
            scopes: None,
            thread_id: ThreadId(1),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Stack trace (frames 0 - 0) for thread 1:"));
        assert!(output.contains("No stack frames found"));
    }

    #[test]
    fn format_stack_trace_basic() {
        let result = StackTraceResult {
            stack_frames: vec![make_frame(10, "main"), make_frame(11, "foo")],
            start_frame: 0,
            has_more_frames: false,
            scopes: None,
            thread_id: ThreadId(1),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Stack trace (frames 0 - 1) for thread 1:"));
        assert!(output.contains("#0: main"));
        assert!(output.contains("#1: foo"));
        assert!(!output.contains("frames omitted"));
    }

    #[test]
    fn format_stack_trace_with_offset() {
        let result = StackTraceResult {
            stack_frames: vec![make_frame(12, "bar")],
            start_frame: 5,
            has_more_frames: false,
            scopes: None,
            thread_id: ThreadId(2),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Stack trace (frames 5 - 5) for thread 2:"));
        assert!(output.contains("#5: bar"));
    }

    #[test]
    fn format_stack_trace_with_more_frames() {
        let result = StackTraceResult {
            stack_frames: vec![make_frame(10, "main"), make_frame(11, "foo")],
            start_frame: 0,
            has_more_frames: true,
            scopes: None,
            thread_id: ThreadId(1),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("frames omitted after #1"));
    }

    #[test]
    fn format_stack_trace_with_scopes() {
        let result = StackTraceResult {
            stack_frames: vec![make_frame(10, "main")],
            start_frame: 0,
            has_more_frames: false,
            scopes: Some(ScopesResult {
                scopes: vec![Scope {
                    name: "Locals".to_string(),
                    presentation_hint: Some(ScopePresentationHint::Locals),
                    variables_reference: VariablesReference(5),
                    ..Default::default()
                }],
                locals: Some(vec![Variable {
                    name: "x".to_string(),
                    value: "42".to_string(),
                    ..Default::default()
                }]),
                frame_id: FrameId(10),
                ..Default::default()
            }),
            thread_id: ThreadId(1),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("#0: main"));
        assert!(output.contains("Scopes for frame 10:"));
        assert!(output.contains("x: 42"));
    }

    fn make_thread(id: i64, name: &str) -> Thread {
        Thread {
            id: ThreadId(id),
            name: name.to_string(),
        }
    }

    #[test]
    fn format_threads_empty() {
        let result = ThreadsResult {
            threads: vec![],
            stack_trace: None,
            ..Default::default()
        };
        assert_eq!(result.to_string(), "Threads:\n  No threads found\n");
    }

    #[test]
    fn format_threads_basic() {
        let result = ThreadsResult {
            threads: vec![make_thread(1, "main"), make_thread(2, "worker")],
            stack_trace: None,
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Threads:"));
        assert!(output.contains("main"));
        assert!(output.contains("worker"));
        assert!(!output.contains("Stack trace"));
    }

    #[test]
    fn format_threads_with_stack_trace() {
        let result = ThreadsResult {
            threads: vec![make_thread(1, "main")],
            stack_trace: Some(StackTraceResult {
                stack_frames: vec![make_frame(10, "entry")],
                start_frame: 0,
                has_more_frames: false,
                scopes: None,
                thread_id: ThreadId(1),
                ..Default::default()
            }),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("main"));
        assert!(output.contains("Stack trace (frames 0 - 0) for thread 1:"));
        assert!(output.contains("#0: entry"));
    }

    #[test]
    fn format_set_variable_basic() {
        let result = SetVariableResult {
            body: SetVariableResponseBody {
                value: "42".to_string(),
                type_: Some("int".to_string()),
                ..Default::default()
            },
            name: "x".to_string(),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Variable 'x' set successfully"));
        assert!(output.contains("x: 42 (int)"));
    }

    #[test]
    fn format_set_variable_with_child_ref() {
        let result = SetVariableResult {
            body: SetVariableResponseBody {
                value: "{}".to_string(),
                type_: Some("MyStruct".to_string()),
                variables_reference: Some(VariablesReference(99)),
                ..Default::default()
            },
            name: "obj".to_string(),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("obj: {} (MyStruct) [ref: 99]"));
    }

    #[test]
    fn format_set_variable_no_type() {
        let result = SetVariableResult {
            body: SetVariableResponseBody {
                value: "hello".to_string(),
                ..Default::default()
            },
            name: "s".to_string(),
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Variable 's' set successfully"));
        assert!(output.contains("  s: hello\n"));
    }

    #[test]
    fn format_navigate_step_command() {
        let output = NavigationResult {
            result: NavigateResult::CommandExecuted,
            navigation_type: NavigationType::StepOver,
            extra: Default::default(),
        };
        assert_eq!(output.to_string(), "Next command executed successfully");
    }

    #[test]
    fn format_navigate_stopped() {
        let stopped = StoppedEventBody {
            reason: dapper_dap_protocol::enums::StoppedReason::Breakpoint,
            ..Default::default()
        };
        let output = NavigationResult {
            result: NavigateResult::Stopped(stopped),
            navigation_type: NavigationType::Continue,
            extra: Default::default(),
        };
        assert_eq!(output.to_string(), "Execution stopped: breakpoint");
    }

    #[test]
    fn format_navigate_exited() {
        let exited = ExitedEventBody {
            exit_code: 0,
            ..Default::default()
        };
        let output = NavigationResult {
            result: NavigateResult::Exited(exited),
            navigation_type: NavigationType::Continue,
            extra: Default::default(),
        };
        assert_eq!(output.to_string(), "Program exited with code: 0");
    }

    #[test]
    fn format_navigate_timeout_with_seconds() {
        let output = NavigationResult {
            result: NavigateResult::TimedOut {
                timeout_seconds: Some(30),
            },
            navigation_type: NavigationType::Continue,
            extra: Default::default(),
        };
        assert_eq!(
            output.to_string(),
            "Continue command executed, program still running after 30 seconds"
        );
    }

    #[test]
    fn format_navigate_timeout_no_seconds() {
        let output = NavigationResult {
            result: NavigateResult::TimedOut {
                timeout_seconds: None,
            },
            navigation_type: NavigationType::Pause,
            extra: Default::default(),
        };
        assert_eq!(
            output.to_string(),
            "Pause command executed, program still running"
        );
    }

    #[test]
    fn format_navigate_timeout_reverse_continue() {
        let output = NavigationResult {
            result: NavigateResult::TimedOut {
                timeout_seconds: Some(30),
            },
            navigation_type: NavigationType::ReverseContinue,
            extra: Default::default(),
        };
        assert_eq!(
            output.to_string(),
            "Reverse continue command executed, program still running after 30 seconds"
        );
    }

    fn make_bp(line: i64, verified: bool) -> BreakpointInfo {
        BreakpointInfo {
            line,
            verified,
            ..Default::default()
        }
    }

    #[test]
    fn format_set_breakpoints_set() {
        let result = SetBreakpointsResult {
            breakpoints: vec![make_bp(10, true), make_bp(20, false)],
            source_path: "/path/to/file.py".to_string(),
            new_count: 2,
            existing_count: 0,
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Set 2 breakpoints in /path/to/file.py:"));
        assert!(output.contains("  Verified: Line 10"));
        assert!(output.contains("  Not Verified: Line 20"));
    }

    #[test]
    fn format_set_breakpoints_appended() {
        let result = SetBreakpointsResult {
            breakpoints: vec![make_bp(10, true), make_bp(20, true), make_bp(30, true)],
            source_path: "/path/to/file.py".to_string(),
            new_count: 1,
            existing_count: 2,
            ..Default::default()
        };
        let output = result.to_string();
        assert!(
            output.contains(
                "Appended 1 new breakpoints to existing 2 breakpoints in /path/to/file.py:"
            )
        );
    }

    #[test]
    fn format_set_breakpoints_empty() {
        let result = SetBreakpointsResult {
            breakpoints: vec![],
            source_path: "/path/to/file.py".to_string(),
            new_count: 0,
            existing_count: 0,
            ..Default::default()
        };
        let output = result.to_string();
        assert!(output.contains("Set 0 breakpoints in /path/to/file.py:"));
    }

    #[test]
    fn serde_scopes_result_round_trip() {
        let original = ScopesResult {
            scopes: vec![Scope {
                name: "Locals".to_string(),
                presentation_hint: Some(ScopePresentationHint::Locals),
                variables_reference: VariablesReference(10),
                ..Default::default()
            }],
            locals: Some(vec![Variable {
                name: "x".to_string(),
                value: "42".to_string(),
                var_type: Some("int".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: ScopesResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn serde_stack_trace_result_round_trip() {
        let original = StackTraceResult {
            stack_frames: vec![make_frame(10, "main"), make_frame(11, "foo")],
            start_frame: 0,
            has_more_frames: true,
            scopes: Some(ScopesResult {
                scopes: vec![Scope {
                    name: "Locals".to_string(),
                    variables_reference: VariablesReference(5),
                    ..Default::default()
                }],
                locals: None,
                ..Default::default()
            }),
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: StackTraceResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn serde_threads_result_round_trip() {
        let original = ThreadsResult {
            threads: vec![make_thread(1, "main"), make_thread(2, "worker")],
            stack_trace: Some(StackTraceResult {
                stack_frames: vec![make_frame(10, "entry")],
                start_frame: 0,
                has_more_frames: false,
                scopes: None,
                ..Default::default()
            }),
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: ThreadsResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn serde_set_breakpoints_result_round_trip() {
        let original = SetBreakpointsResult {
            breakpoints: vec![make_bp(10, true), make_bp(20, false)],
            source_path: "/path/to/file.py".to_string(),
            new_count: 2,
            existing_count: 0,
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: SetBreakpointsResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn serde_set_exception_breakpoints_result_round_trip() {
        let original = SetExceptionBreakpointsResult {
            installed: vec![
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: Some("x>5".to_string()),
                },
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
            ],
            new_count: 2,
            existing_count: 0,
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: SetExceptionBreakpointsResult =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn display_set_exception_breakpoints_result_set_wording() {
        // existing_count == 0 → "Set N exception breakpoint filter(s)" wording.
        let result = SetExceptionBreakpointsResult {
            installed: vec![ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            }],
            new_count: 1,
            existing_count: 0,
            ..Default::default()
        };
        let rendered = result.to_string();
        assert!(
            rendered.starts_with("Set 1 exception breakpoint filter(s):"),
            "expected 'Set' wording: {rendered}"
        );
        assert!(
            rendered.contains("  uncaught\n"),
            "expected bare filter line: {rendered}"
        );
    }

    #[test]
    fn display_set_exception_breakpoints_result_added_wording() {
        // existing_count > 0 → "Added N to existing M (T total)" wording,
        // with conditions rendered alongside the filter id.
        let result = SetExceptionBreakpointsResult {
            installed: vec![
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: Some("x>5".to_string()),
                },
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
            ],
            new_count: 1,
            existing_count: 1,
            ..Default::default()
        };
        let rendered = result.to_string();
        assert!(
            rendered
                .starts_with("Added 1 new exception breakpoint filter(s) to existing 1 (2 total):"),
            "expected 'Added' wording: {rendered}"
        );
        assert!(
            rendered.contains("  raised (condition: x>5)\n"),
            "expected condition rendering: {rendered}"
        );
        assert!(
            rendered.contains("  uncaught\n"),
            "expected bare filter rendering: {rendered}"
        );
    }

    #[test]
    fn serde_navigate_result_round_trip_all_variants() {
        let variants: Vec<NavigateResult> = vec![
            NavigateResult::CommandExecuted,
            NavigateResult::Stopped(StoppedEventBody {
                reason: dapper_dap_protocol::enums::StoppedReason::Breakpoint,
                thread_id: Some(ThreadId(1)),
                ..Default::default()
            }),
            NavigateResult::Exited(ExitedEventBody {
                exit_code: 0,
                ..Default::default()
            }),
            NavigateResult::TimedOut {
                timeout_seconds: Some(30),
            },
        ];

        for original in variants {
            let json = serde_json::to_string(&original).expect("serialize");
            let deserialized: NavigateResult = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(original, deserialized);
        }
    }

    #[test]
    fn serde_forward_compat_unknown_fields_captured_in_extra() {
        let json = r#"{
            "scopes": [],
            "locals": null,
            "futureField": "hello",
            "anotherNewField": 42
        }"#;

        let result: ScopesResult = serde_json::from_str(json).expect("deserialize");
        assert!(result.scopes.is_empty());
        assert!(result.locals.is_none());
        assert_eq!(
            result.extra.get("futureField"),
            Some(&serde_json::json!("hello"))
        );
        assert_eq!(
            result.extra.get("anotherNewField"),
            Some(&serde_json::json!(42))
        );
    }

    #[test]
    fn serde_forward_compat_extra_survives_round_trip() {
        let json = r#"{
            "threads": [],
            "newServerField": {"nested": true}
        }"#;

        let result: ThreadsResult = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            result.extra.get("newServerField"),
            Some(&serde_json::json!({"nested": true}))
        );

        let re_serialized = serde_json::to_string(&result).expect("re-serialize");
        let re_deserialized: ThreadsResult =
            serde_json::from_str(&re_serialized).expect("re-deserialize");
        assert_eq!(result, re_deserialized);
    }

    #[test]
    fn serde_missing_fields_use_defaults() {
        let json = "{}";

        let scopes: ScopesResult = serde_json::from_str(json).expect("deserialize ScopesResult");
        assert!(scopes.scopes.is_empty());
        assert!(scopes.locals.is_none());

        let st: StackTraceResult =
            serde_json::from_str(json).expect("deserialize StackTraceResult");
        assert!(st.stack_frames.is_empty());
        assert_eq!(st.start_frame, 0);
        assert!(!st.has_more_frames);
        assert!(st.scopes.is_none());

        let threads: ThreadsResult = serde_json::from_str(json).expect("deserialize ThreadsResult");
        assert!(threads.threads.is_empty());
        assert!(threads.stack_trace.is_none());

        let bp: SetBreakpointsResult =
            serde_json::from_str(json).expect("deserialize SetBreakpointsResult");
        assert!(bp.breakpoints.is_empty());
        assert!(bp.source_path.is_empty());
        assert_eq!(bp.new_count, 0);
        assert_eq!(bp.existing_count, 0);
    }

    #[test]
    fn serde_navigate_result_unknown_variant() {
        let json = r#"{"type": "futureVariant", "data": {"info": 123}}"#;
        let result: NavigateResult = serde_json::from_str(json).expect("deserialize");
        assert!(matches!(result, NavigateResult::Unknown(_)));
    }

    #[test]
    fn serde_waited_event_unknown_variant() {
        let json = r#"{"FutureEventType": {"some_data": true}}"#;
        let result: WaitedEvent = serde_json::from_str(json).expect("deserialize");
        assert!(matches!(result, WaitedEvent::Unknown(_)));
    }

    #[test]
    fn serde_waited_event_known_variants_preserved() {
        let received = WaitedEvent::Received(EventKind::Stopped(StoppedEventBody {
            reason: dapper_dap_protocol::enums::StoppedReason::Breakpoint,
            ..Default::default()
        }));
        let json = serde_json::to_string(&received).expect("serialize");
        let deserialized: WaitedEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(deserialized, WaitedEvent::Received(_)));

        let timed_out = WaitedEvent::TimedOut {
            timeout_seconds: 30,
        };
        let json = serde_json::to_string(&timed_out).expect("serialize");
        let deserialized: WaitedEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            deserialized,
            WaitedEvent::TimedOut {
                timeout_seconds: 30
            }
        ));
    }

    fn make_session_info(id: &str) -> dapper_session::SessionInfo {
        dapper_session::SessionInfo {
            session_id: id.into(),
            pid: 1234,
            control_plane_port: None,
            started_at: 0,
            command_line_args: vec![],
            current_working_directory: None,
            scope_id: None,
            request_type: None,
            session_type: None,
            program_path: None,
            debuggee_process_id: None,
            debugger_args: None,
            parent_session_id: None,
        }
    }

    #[test]
    fn sessions_empty_no_scope() {
        let result = SessionsResult {
            sessions: vec![],
            scope_id: None,
        };
        assert_eq!(result.to_string(), "No active sessions found.");

        let json: serde_json::Value = serde_json::to_value(&result).expect("serialize to JSON");
        assert_eq!(json, serde_json::json!({"sessions": []}));
    }

    #[test]
    fn sessions_empty_with_scope() {
        let result = SessionsResult {
            sessions: vec![],
            scope_id: Some(dapper_session::ScopeId::new("my-scope")),
        };
        assert_eq!(
            result.to_string(),
            "No active sessions found in scope 'my-scope'."
        );

        let json: serde_json::Value = serde_json::to_value(&result).expect("serialize to JSON");
        assert_eq!(
            json,
            serde_json::json!({"sessions": [], "scopeId": "my-scope"})
        );
    }

    #[test]
    fn sessions_with_entries() {
        let result = SessionsResult {
            sessions: vec![make_session_info("s1"), make_session_info("s2")],
            scope_id: None,
        };
        let output = result.to_string();
        assert!(output.contains("Found 2 active session(s)"));
        assert!(output.contains("Session s1:"));
        assert!(output.contains("Session s2:"));
    }

    #[test]
    fn sessions_expose_parent_session_id() {
        let root = make_session_info("root");
        let child = make_session_info("child").with_parent_session_id(Some("root".into()));
        let result = SessionsResult {
            sessions: vec![root, child],
            scope_id: None,
        };

        // JSON: the child carries `parentSessionId` (camelCase); the root omits it.
        let json: serde_json::Value = serde_json::to_value(&result).expect("serialize to JSON");
        let sessions = json["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions[0]["sessionId"], "root");
        assert!(
            sessions[0].get("parentSessionId").is_none(),
            "root session must omit parentSessionId, got: {}",
            sessions[0]
        );
        assert_eq!(
            sessions[1]["parentSessionId"], "root",
            "child session must report its parent, got: {}",
            sessions[1]
        );

        // Plaintext Display shows the parent linkage for the child only.
        let output = result.to_string();
        assert!(
            output.contains("Parent Session: root"),
            "Display should show the child's parent, got:\n{output}"
        );
    }

    #[test]
    fn sessions_with_entries_and_scope() {
        let result = SessionsResult {
            sessions: vec![make_session_info("s1")],
            scope_id: Some(dapper_session::ScopeId::new("dev")),
        };
        let output = result.to_string();
        assert!(output.contains("Found 1 active session(s) in scope 'dev'"));
    }

    #[test]
    fn sessions_separator_between_entries() {
        let result = SessionsResult {
            sessions: vec![
                make_session_info("s1"),
                make_session_info("s2"),
                make_session_info("s3"),
            ],
            scope_id: None,
        };

        let output = result.to_string();

        // The previous CLI behavior printed each session via SessionInfo::Display
        // with a blank line between sessions (but not after the last one).
        assert!(output.starts_with("Found 3 active session(s):\n"));
        assert!(output.contains("Session s1:"));
        assert!(output.contains("Session s2:"));
        assert!(output.contains("Session s3:"));

        // Verify blank-line separators between sessions (but not trailing)
        let lines: Vec<&str> = output.lines().collect();
        let session_header_indices: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("Session s"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(session_header_indices.len(), 3);

        // Between sessions there should be a blank line
        for pair in session_header_indices.windows(2) {
            let prev_session_last_line = pair[1] - 1;
            assert!(
                lines[prev_session_last_line].is_empty(),
                "expected blank line separator before session header at line {}",
                pair[1]
            );
        }

        // No trailing blank line after last session
        assert!(
            !lines.last().unwrap().is_empty(),
            "should not have trailing blank line after last session"
        );
    }

    #[test]
    fn sessions_json_excludes_debugger_args() {
        let mut session = make_session_info("s1");
        session.debugger_args = Some(serde_json::json!({"type": "python", "secret": "value"}));

        let result = SessionsResult {
            sessions: vec![session],
            scope_id: None,
        };

        let json = serde_json::to_value(&result).expect("serialize");
        let sessions_arr = json["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions_arr.len(), 1);

        let session_obj = sessions_arr[0].as_object().expect("session object");
        let mut actual_keys: Vec<&str> = session_obj.keys().map(|k| k.as_str()).collect();
        actual_keys.sort();
        let expected_keys = vec![
            "commandLineArgs",
            "controlPlanePort",
            "currentWorkingDirectory",
            "pid",
            "scopeId",
            "sessionId",
            "startedAt",
        ];
        assert_eq!(
            actual_keys, expected_keys,
            "serialized session should contain exactly these keys (no debugger_args)"
        );
        assert_eq!(session_obj["sessionId"], "s1");
    }

    #[test]
    fn serde_sessions_result_deserialize_empty() {
        let json = r#"{"sessions": [], "scopeId": null}"#;
        let result: SessionsResult = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            result,
            SessionsResult {
                sessions: vec![],
                scope_id: None,
            }
        );
    }

    #[test]
    fn serde_sessions_result_deserialize_defaults() {
        let json = "{}";
        let result: SessionsResult = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            result,
            SessionsResult {
                sessions: vec![],
                scope_id: None,
            }
        );
    }

    #[test]
    fn serde_breakpoint_info_round_trip() {
        let original = BreakpointInfo {
            line: 42,
            verified: true,
            id: Some(dapper_dap_protocol::data_types::BreakpointId(5)),
            condition: Some("x > 0".to_string()),
            log_message: Some("hit bp".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: BreakpointInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn serde_breakpoint_info_forward_compat() {
        let json = r#"{"line": 10, "verified": true, "newField": "value"}"#;
        let result: BreakpointInfo = serde_json::from_str(json).expect("deserialize");
        assert_eq!(result.line, 10);
        assert!(result.verified);
        assert_eq!(
            result.extra.get("newField"),
            Some(&serde_json::json!("value"))
        );
    }
}
