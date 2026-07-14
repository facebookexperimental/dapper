// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Structured, serializable representation of a [`ResponseContext`] for JSON output.
//! Mirrors the plaintext formatting in `envelope.rs`, but produces typed structs
//! instead of formatted strings.

use dapper_config::ContextConfig;
use dapper_dap_protocol::data_types::Seq;
use dapper_session::BreakpointInfo;
use dapper_session::ExceptionFilterEntry;
use dapper_session::ExecutionStateSummary;
use dapper_session::OutputEvent;
use dapper_session::RequestType;
use dapper_session::ResponseContext;
use dapper_session::SessionId;
use dapper_session::SessionInfo;
use serde::Serialize;

/// Structured JSON representation of a [`ResponseContext`], combining session info
/// and context sections (execution state, breakpoints, output, other sessions).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseContextOutput<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfoOutput<'a>>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_sessions: Vec<&'a SessionId>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_state: Option<&'a ExecutionStateSummary>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<BreakpointsOutput<'a>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_exception_filters: Option<ExceptionFiltersOutput<'a>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputSummary<'a>>,
}

impl<'a> ResponseContextOutput<'a> {
    pub fn from_response_context(context: &'a ResponseContext, config: &ContextConfig) -> Self {
        let session = if config.show_session {
            context
                .session
                .as_ref()
                .map(SessionInfoOutput::from_session_info)
        } else {
            None
        };

        let other_sessions = if config.show_sessions {
            context
                .other_sessions
                .iter()
                .map(|s| &s.session_id)
                .collect()
        } else {
            Vec::new()
        };

        let execution_state = if config.show_execution_state {
            context
                .execution_state
                .as_ref()
                .map(|versioned| &versioned.state)
        } else {
            None
        };

        let breakpoints = if config.show_breakpoints {
            BreakpointsOutput::from_response_context(context, config)
        } else {
            None
        };

        let installed_exception_filters = if config.show_exception_breakpoints {
            ExceptionFiltersOutput::from_response_context(context)
        } else {
            None
        };

        let output = if config.max_output_lines > 0 {
            OutputSummary::from_response_context(context)
        } else {
            None
        };

        Self {
            session,
            other_sessions,
            execution_state,
            breakpoints,
            installed_exception_filters,
            output,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.session.is_none()
            && self.other_sessions.is_empty()
            && self.execution_state.is_none()
            && self.breakpoints.is_none()
            && self.installed_exception_filters.is_none()
            && self.output.is_none()
    }
}

/// Session info for the JSON context header (mirrors `format_context_header` in `envelope.rs`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoOutput<'a> {
    pub session_id: &'a SessionId,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub debugger: Option<&'a str>,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub request_type: Option<RequestType>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
}

impl<'a> SessionInfoOutput<'a> {
    pub fn from_session_info(info: &'a SessionInfo) -> Self {
        let (program, pid) = match info.request_type {
            Some(RequestType::Launch) => (info.program_path.as_deref(), None),
            Some(RequestType::Attach) => (None, info.debuggee_process_id),
            None => (None, None),
        };

        Self {
            session_id: &info.session_id,
            debugger: info.session_type.as_deref(),
            request_type: info.request_type,
            program,
            pid,
        }
    }
}

/// A single source file with its breakpoint lines.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointFileEntry<'a> {
    pub path: &'a str,
    pub lines: Vec<i64>,
}

impl<'a> BreakpointFileEntry<'a> {
    pub fn from_breakpoint_info(path: &'a str, breakpoints: &[BreakpointInfo]) -> Self {
        let mut lines: Vec<i64> = breakpoints.iter().map(|bp| bp.line).collect();
        lines.sort();
        Self { path, lines }
    }
}

/// Breakpoints section with truncation metadata.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointsOutput<'a> {
    pub files: Vec<BreakpointFileEntry<'a>>,
    pub total_files: usize,
    pub shown_files: usize,

    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl<'a> BreakpointsOutput<'a> {
    pub fn from_response_context(
        context: &'a ResponseContext,
        config: &ContextConfig,
    ) -> Option<Self> {
        if context.breakpoints.is_empty() {
            return None;
        }

        let mut file_keys: Vec<&String> = context.breakpoints.keys().collect();
        file_keys.sort();
        let total_files = file_keys.len();
        let shown_files = total_files.min(config.max_source_files);

        let files = file_keys[..shown_files]
            .iter()
            .map(|key| BreakpointFileEntry::from_breakpoint_info(key, &context.breakpoints[*key]))
            .collect();

        Some(Self {
            files,
            total_files,
            shown_files,
            truncated: shown_files < total_files,
        })
    }
}

/// Exception breakpoint filters section. Mirrors `BreakpointsOutput` but
/// flat (filters aren't grouped by anything).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionFiltersOutput<'a> {
    pub filters: Vec<&'a ExceptionFilterEntry>,
}

impl<'a> ExceptionFiltersOutput<'a> {
    pub fn from_response_context(context: &'a ResponseContext) -> Option<Self> {
        if context.installed_exception_filters.is_empty() {
            return None;
        }
        // Tracker stores entries already sorted by filter id, but sort
        // defensively here so the JSON output is robust to refactors.
        let mut filters: Vec<&ExceptionFilterEntry> =
            context.installed_exception_filters.iter().collect();
        filters.sort_unstable_by(|a, b| a.filter.cmp(&b.filter));
        Some(Self { filters })
    }
}

/// A single output event entry.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEventEntry<'a> {
    pub seq: Seq,
    pub category: &'a str,
    pub output: &'a str,
}

impl<'a> OutputEventEntry<'a> {
    pub fn from_output_event(event: &'a OutputEvent) -> Self {
        Self {
            seq: event.seq,
            category: event
                .category
                .as_ref()
                .map_or("unspecified", |c| c.as_ref()),
            output: &event.output,
        }
    }
}

/// Output section with truncation metadata.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputSummary<'a> {
    pub events: Vec<OutputEventEntry<'a>>,
    pub shown_events: usize,
    pub total_count: usize,

    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_history_file: Option<String>,

    /// Where `events` splits into head and tail.
    #[serde(skip)]
    pub head_count: usize,
}

impl<'a> OutputSummary<'a> {
    pub fn from_response_context(context: &'a ResponseContext) -> Option<Self> {
        if context.output.is_empty() {
            return None;
        }

        let events: Vec<OutputEventEntry<'a>> = context
            .output
            .head
            .iter()
            .chain(context.output.tail.iter())
            .map(OutputEventEntry::from_output_event)
            .collect();
        let shown_events = events.len();

        Some(Self {
            events,
            shown_events,
            total_count: context.output.total_count,
            truncated: shown_events < context.output.total_count,
            output_history_file: context
                .output_history_file
                .as_ref()
                .map(|p| p.display().to_string()),
            head_count: context.output.head.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::enums::OutputCategory;
    use dapper_dap_protocol::enums::StoppedReason;
    use dapper_session::BufferedOutput;
    use dapper_session::ExecutionStateSummary;
    use dapper_session::ExecutionStatus;
    use dapper_session::VersionedExecutionStateSummary;

    use super::*;

    fn make_session_info(session_id: &str) -> SessionInfo {
        SessionInfo {
            session_id: session_id.into(),
            pid: 0,
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
    fn session_info_none_without_session() {
        let ctx = ResponseContext::default();
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        assert!(output.session.is_none());
    }

    #[test]
    fn session_info_launch() {
        let mut info = make_session_info("abc-123");
        info.session_type = Some("debugpy".to_string());
        info.request_type = Some(RequestType::Launch);
        info.program_path = Some("/path/to/main.py".to_string());

        let session = SessionInfoOutput::from_session_info(&info);
        assert_eq!(
            serde_json::to_value(&session).unwrap(),
            serde_json::json!({
                "sessionId": "abc-123",
                "debugger": "debugpy",
                "type": "launch",
                "program": "/path/to/main.py",
            })
        );
    }

    #[test]
    fn session_info_attach() {
        let mut info = make_session_info("def-456");
        info.session_type = Some("cppdbg".to_string());
        info.request_type = Some(RequestType::Attach);
        info.debuggee_process_id = Some(12345);

        let session = SessionInfoOutput::from_session_info(&info);
        assert_eq!(
            serde_json::to_value(&session).unwrap(),
            serde_json::json!({
                "sessionId": "def-456",
                "debugger": "cppdbg",
                "type": "attach",
                "pid": 12345,
            })
        );
    }

    #[test]
    fn session_info_minimal() {
        let info = make_session_info("min-session");
        let session = SessionInfoOutput::from_session_info(&info);
        assert_eq!(
            serde_json::to_value(&session).unwrap(),
            serde_json::json!({
                "sessionId": "min-session",
            })
        );
    }

    #[test]
    fn empty_context_is_empty() {
        let ctx = ResponseContext::default();
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        assert!(output.is_empty());
    }

    #[test]
    fn includes_other_sessions() {
        let ctx = ResponseContext {
            other_sessions: vec![make_session_info("other-1"), make_session_info("other-2")],
            ..Default::default()
        };
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        assert_eq!(
            serde_json::to_value(&output).unwrap(),
            serde_json::json!({
                "otherSessions": ["other-1", "other-2"],
            })
        );
    }

    #[test]
    fn includes_execution_state() {
        let ctx = ResponseContext {
            execution_state: Some(VersionedExecutionStateSummary {
                version: 1,
                state: ExecutionStateSummary {
                    status: ExecutionStatus::Stopped,
                    thread_id: Some(ThreadId(5)),
                    stop_reason: Some(StoppedReason::Breakpoint),
                    description: Some("hit breakpoint".to_string()),
                    additional_information: Some("extra info".to_string()),
                },
            }),
            ..Default::default()
        };
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        assert_eq!(
            serde_json::to_value(output.execution_state.unwrap()).unwrap(),
            serde_json::json!({
                "status": "stopped",
                "threadId": 5,
                "stopReason": "breakpoint",
                "description": "hit breakpoint",
                "additionalInformation": "extra info",
            })
        );
    }

    #[test]
    fn includes_breakpoints() {
        let ctx = ResponseContext {
            breakpoints: HashMap::from([(
                "/path/to/file.py".to_string(),
                vec![
                    BreakpointInfo {
                        line: 20,
                        verified: true,
                        ..Default::default()
                    },
                    BreakpointInfo {
                        line: 10,
                        verified: true,
                        ..Default::default()
                    },
                ],
            )]),
            ..Default::default()
        };
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        let bp = output.breakpoints.unwrap();
        assert_eq!(
            serde_json::to_value(&bp).unwrap(),
            serde_json::json!({
                "files": [{"path": "/path/to/file.py", "lines": [10, 20]}],
                "totalFiles": 1,
                "shownFiles": 1,
            })
        );
    }

    #[test]
    fn breakpoints_truncated() {
        let mut ctx = ResponseContext::default();
        for i in 0..30 {
            ctx.breakpoints.insert(
                format!("/path/to/file{}.py", i),
                vec![BreakpointInfo {
                    line: 1,
                    verified: true,
                    ..Default::default()
                }],
            );
        }

        let config = ContextConfig {
            max_source_files: 5,
            ..Default::default()
        };
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        let bp = output.breakpoints.unwrap();
        assert_eq!(
            serde_json::to_value(&bp).unwrap(),
            serde_json::json!({
                "files": [
                    {"path": "/path/to/file0.py", "lines": [1]},
                    {"path": "/path/to/file1.py", "lines": [1]},
                    {"path": "/path/to/file10.py", "lines": [1]},
                    {"path": "/path/to/file11.py", "lines": [1]},
                    {"path": "/path/to/file12.py", "lines": [1]},
                ],
                "totalFiles": 30,
                "shownFiles": 5,
                "truncated": true,
            })
        );
    }

    #[test]
    fn includes_installed_exception_filters() {
        let ctx = ResponseContext {
            installed_exception_filters: vec![
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: Some("x>5".to_string()),
                },
            ],
            ..Default::default()
        };
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        let filters = output.installed_exception_filters.unwrap();
        assert_eq!(
            serde_json::to_value(&filters).unwrap(),
            serde_json::json!({
                "filters": [
                    {"filter": "raised", "condition": "x>5"},
                    {"filter": "uncaught"},
                ],
            })
        );
    }

    #[test]
    fn omits_exception_filters_when_disabled() {
        let ctx = ResponseContext {
            installed_exception_filters: vec![ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            }],
            ..Default::default()
        };
        let config = ContextConfig {
            show_exception_breakpoints: false,
            ..Default::default()
        };
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        assert!(output.installed_exception_filters.is_none());
    }

    #[test]
    fn includes_output() {
        let ctx = ResponseContext {
            output: BufferedOutput {
                head: vec![OutputEvent {
                    seq: 10.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "Hello, World!".to_string(),
                    ..Default::default()
                }],
                tail: vec![],
                total_count: 1,
                ..Default::default()
            },
            output_history_file: Some(std::path::PathBuf::from("/tmp/output.log")),
            ..Default::default()
        };
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        let out = output.output.unwrap();
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!({
                "events": [{"seq": 10, "category": "stdout", "output": "Hello, World!"}],
                "totalCount": 1,
                "shownEvents": 1,
                "outputHistoryFile": "/tmp/output.log",
            })
        );
    }

    #[test]
    fn output_truncated() {
        let ctx = ResponseContext {
            output: BufferedOutput {
                head: vec![OutputEvent {
                    seq: 1.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "first line".to_string(),
                    ..Default::default()
                }],
                tail: vec![OutputEvent {
                    seq: 50.into(),
                    category: Some(OutputCategory::Stderr),
                    output: "last line".to_string(),
                    ..Default::default()
                }],
                total_count: 50,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        let out = output.output.unwrap();
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!({
                "events": [
                    {"seq": 1, "category": "stdout", "output": "first line"},
                    {"seq": 50, "category": "stderr", "output": "last line"},
                ],
                "shownEvents": 2,
                "totalCount": 50,
                "truncated": true,
            })
        );
    }

    #[test]
    fn response_combines_session_and_state() {
        let mut info = make_session_info("ctx-test");
        info.session_type = Some("debugpy".to_string());

        let ctx = ResponseContext {
            session: Some(info),
            execution_state: Some(VersionedExecutionStateSummary {
                version: 1,
                state: ExecutionStateSummary {
                    status: ExecutionStatus::Running,
                    ..Default::default()
                },
            }),
            ..Default::default()
        };
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        assert_eq!(
            serde_json::to_value(&output).unwrap(),
            serde_json::json!({
                "session": {
                    "sessionId": "ctx-test",
                    "debugger": "debugpy",
                },
                "executionState": {
                    "status": "running",
                },
            })
        );
    }

    #[test]
    fn response_empty_when_no_data() {
        let ctx = ResponseContext::default();
        let config = ContextConfig::default();
        let output = ResponseContextOutput::from_response_context(&ctx, &config);
        assert!(output.is_empty());
        assert_eq!(
            serde_json::to_value(&output).unwrap(),
            serde_json::json!({})
        );
    }
}
