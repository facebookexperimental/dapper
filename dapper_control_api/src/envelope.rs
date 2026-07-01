// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt::Write;

use dapper_config::ContextConfig;
use dapper_session::RequestType;
use dapper_session::SessionInfo;

use crate::ExceptionFilterEntry;
use crate::OutputEvent;
use crate::ResponseContext;

pub fn format_context_header(session_info: Option<&SessionInfo>) -> String {
    let Some(info) = session_info else {
        return String::new();
    };

    let mut parts = vec![format!("Session: {}", info.session_id)];

    if let Some(session_type) = &info.session_type {
        parts.push(format!("Debugger: {}", session_type));
    }

    if let Some(request_type) = info.request_type {
        parts.push(format!("Type: {}", request_type));

        match request_type {
            RequestType::Launch => {
                if let Some(program) = &info.program_path {
                    parts.push(format!("Program: {}", program));
                }
            }
            RequestType::Attach => {
                if let Some(pid) = info.debuggee_process_id {
                    parts.push(format!("PID: {}", pid));
                }
            }
        }
    }

    parts.join(" | ")
}

const MAX_OUTPUT_LINE_LENGTH: usize = 150;

pub fn format_context_footer(context: &ResponseContext, config: &ContextConfig) -> Option<String> {
    let mut result = String::new();

    if config.show_sessions && !context.other_sessions.is_empty() {
        let _ = writeln!(
            result,
            "Other active debug sessions ({}):",
            context.other_sessions.len()
        );
        for session in &context.other_sessions {
            let _ = writeln!(result, "  - {}", session.session_id);
        }
        result.push('\n');
    }

    if config.show_execution_state
        && let Some(ref versioned) = context.execution_state
    {
        result.push_str(&versioned.state.format_summary());
    }

    if config.show_breakpoints && !context.breakpoints.is_empty() {
        let mut files: Vec<_> = context.breakpoints.keys().collect();
        files.sort();

        let showing = files.len().min(config.max_source_files);

        if showing < files.len() {
            let _ = writeln!(
                result,
                "Source-line breakpoints (only showing first {}):",
                showing
            );
        } else {
            result.push_str("Source-line breakpoints:\n");
        }

        for file in &files[..showing] {
            let mut lines: Vec<i64> = context.breakpoints[*file]
                .iter()
                .map(|bp| bp.line)
                .collect();
            lines.sort();
            let _ = writeln!(result, "  {}: lines {:?}", file, lines);
        }
    }

    if config.show_exception_breakpoints && !context.installed_exception_filters.is_empty() {
        // Tracker stores entries sorted by filter id; sort defensively here
        // too in case a future tracker refactor relaxes that invariant.
        let mut entries: Vec<&ExceptionFilterEntry> =
            context.installed_exception_filters.iter().collect();
        entries.sort_unstable_by(|a, b| a.filter.cmp(&b.filter));

        result.push_str("Exception breakpoints:\n");
        for entry in entries {
            match &entry.condition {
                Some(cond) => {
                    let _ = writeln!(result, "  {} (condition: {})", entry.filter, cond);
                }
                None => {
                    let _ = writeln!(result, "  {}", entry.filter);
                }
            }
        }
    }

    if config.max_output_lines > 0 && !context.output.is_empty() {
        let _ = writeln!(
            result,
            "\nNew output ({} events since last response):",
            context.output.total_count
        );

        format_output_events(&context.output, &mut result);

        if let Some(ref path) = context.output_history_file {
            let _ = writeln!(
                result,
                "\nFull output history available at: {}",
                path.display()
            );
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn format_output_events(output: &crate::BufferedOutput, result: &mut String) {
    for event in &output.head {
        format_output_event(event, result);
    }

    let buffered_count = output.head.len() + output.tail.len();
    if output.total_count > buffered_count {
        let skipped = output.total_count - buffered_count;
        let _ = write!(result, "\n... ({} events omitted) ...\n\n", skipped);
    }

    for event in &output.tail {
        format_output_event(event, result);
    }
}

fn format_output_event(event: &OutputEvent, result: &mut String) {
    let category_str = event
        .category
        .as_ref()
        .map_or("unspecified", |c| c.as_ref());
    for line in event.output.lines() {
        let (display, suffix) = if line.len() > MAX_OUTPUT_LINE_LENGTH {
            (
                &line[..line.floor_char_boundary(MAX_OUTPUT_LINE_LENGTH)],
                "...",
            )
        } else {
            (line, "")
        };
        let _ = writeln!(
            result,
            "[seq:{} {}] {}{}",
            event.seq, category_str, display, suffix
        );
    }
}

pub fn format_envelope(result: &str, header: Option<&str>, footer: Option<&str>) -> String {
    let mut output = String::new();

    if let Some(header) = header {
        let _ = write!(output, "{}\n___\n\n", header);
    }

    output.push_str(result);

    if let Some(footer) = footer {
        if result.is_empty() {
            output.push_str(footer);
        } else {
            let _ = write!(output, "\n\n___\n{}", footer);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::enums::OutputCategory;
    use dapper_dap_protocol::enums::StoppedReason;

    use super::*;
    use crate::BreakpointInfo;
    use crate::BufferedOutput;
    use crate::ExceptionFilterEntry;
    use crate::ExecutionStateSummary;
    use crate::ExecutionStatus;
    use crate::VersionedExecutionStateSummary;

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
    fn format_context_header_without_session_info() {
        let output = format_context_header(None);
        assert_eq!(output, "");
    }

    #[test]
    fn format_context_header_launch_session() {
        let mut info = make_session_info("abc-123");
        info.session_type = Some("debugpy".to_string());
        info.request_type = Some(RequestType::Launch);
        info.program_path = Some("/path/to/main.py".to_string());

        let output = format_context_header(Some(&info));
        assert!(output.contains("Session: abc-123"));
        assert!(output.contains("Debugger: debugpy"));
        assert!(output.contains("Type: launch"));
        assert!(output.contains("Program: /path/to/main.py"));
    }

    #[test]
    fn format_context_header_attach_session() {
        let mut info = make_session_info("def-456");
        info.session_type = Some("cppdbg".to_string());
        info.request_type = Some(RequestType::Attach);
        info.debuggee_process_id = Some(12345);

        let output = format_context_header(Some(&info));
        assert!(output.contains("Session: def-456"));
        assert!(output.contains("Debugger: cppdbg"));
        assert!(output.contains("Type: attach"));
        assert!(output.contains("PID: 12345"));
    }

    #[test]
    fn format_context_header_minimal_session_info() {
        let info = make_session_info("min-session");
        let output = format_context_header(Some(&info));
        assert_eq!(output, "Session: min-session");
    }

    fn empty_context() -> ResponseContext {
        ResponseContext::default()
    }

    fn default_context_config() -> ContextConfig {
        ContextConfig::default()
    }

    #[test]
    fn format_context_footer_empty() {
        let ctx = empty_context();
        assert!(format_context_footer(&ctx, &default_context_config()).is_none());
    }

    #[test]
    fn format_context_footer_with_other_sessions() {
        let mut ctx = empty_context();
        ctx.other_sessions = vec![make_session_info("other-1"), make_session_info("other-2")];

        let output = format_context_footer(&ctx, &default_context_config()).unwrap();
        assert!(output.contains("Other active debug sessions (2):"));
        assert!(output.contains("  - other-1"));
        assert!(output.contains("  - other-2"));
    }

    #[test]
    fn format_context_footer_with_execution_state() {
        let mut ctx = empty_context();
        ctx.execution_state = Some(VersionedExecutionStateSummary {
            version: 1,
            state: ExecutionStateSummary {
                status: ExecutionStatus::Stopped,
                thread_id: Some(ThreadId(5)),
                stop_reason: Some(StoppedReason::Breakpoint),
                description: Some("hit breakpoint".to_string()),
                additional_information: Some("extra info".to_string()),
            },
        });

        let output = format_context_footer(&ctx, &default_context_config()).unwrap();
        assert!(output.contains("execution status: stopped"));
        assert!(output.contains("Stop reason: breakpoint"));
        assert!(output.contains("Thread: 5"));
        assert!(output.contains("Description: hit breakpoint"));
        assert!(output.contains("Additional information: extra info"));
    }

    #[test]
    fn format_context_footer_with_breakpoints() {
        let mut ctx = empty_context();
        ctx.breakpoints.insert(
            "/path/to/file.py".to_string(),
            vec![
                BreakpointInfo {
                    line: 20,
                    verified: true,
                    id: None,
                    ..Default::default()
                },
                BreakpointInfo {
                    line: 10,
                    verified: true,
                    id: None,
                    ..Default::default()
                },
            ],
        );

        let output = format_context_footer(&ctx, &default_context_config()).unwrap();
        assert!(output.contains("Source-line breakpoints:"));
        assert!(output.contains("/path/to/file.py: lines [10, 20]"));
    }

    #[test]
    fn format_context_footer_breakpoints_truncated() {
        let mut ctx = empty_context();
        for i in 0..30 {
            ctx.breakpoints.insert(
                format!("/path/to/file{}.py", i),
                vec![BreakpointInfo {
                    line: 1,
                    verified: true,
                    id: None,
                    ..Default::default()
                }],
            );
        }

        let config = ContextConfig {
            max_source_files: 5,
            ..Default::default()
        };
        let output = format_context_footer(&ctx, &config).unwrap();
        assert!(output.contains("only showing first 5"));
    }

    #[test]
    fn format_context_footer_with_output() {
        let mut ctx = empty_context();
        ctx.output = BufferedOutput {
            head: vec![OutputEvent {
                seq: 10.into(),
                category: Some(OutputCategory::Stdout),
                output: "Hello, World!".to_string(),
                ..Default::default()
            }],
            tail: vec![],
            total_count: 1,
            ..Default::default()
        };
        ctx.output_history_file = Some(std::path::PathBuf::from("/tmp/output.log"));

        let output = format_context_footer(&ctx, &default_context_config()).unwrap();
        assert!(output.contains("New output (1 events since last response):"));
        assert!(output.contains("[seq:10 stdout] Hello, World!"));
        assert!(output.contains("Full output history available at: /tmp/output.log"));
    }

    #[test]
    fn format_context_footer_output_truncated() {
        let mut ctx = empty_context();
        ctx.output = BufferedOutput {
            head: vec![
                OutputEvent {
                    seq: 0.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "line 0".to_string(),
                    ..Default::default()
                },
                OutputEvent {
                    seq: 1.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "line 1".to_string(),
                    ..Default::default()
                },
            ],
            tail: vec![
                OutputEvent {
                    seq: 8.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "line 8".to_string(),
                    ..Default::default()
                },
                OutputEvent {
                    seq: 9.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "line 9".to_string(),
                    ..Default::default()
                },
            ],
            total_count: 10,
            ..Default::default()
        };

        let config = ContextConfig::default();
        let output = format_context_footer(&ctx, &config).unwrap();
        assert!(output.contains("[seq:0 stdout] line 0"));
        assert!(output.contains("[seq:1 stdout] line 1"));
        assert!(output.contains("6 events omitted"));
        assert!(output.contains("[seq:8 stdout] line 8"));
        assert!(output.contains("[seq:9 stdout] line 9"));
    }

    #[test]
    fn format_output_events_with_prebounded_buffer() {
        let output = BufferedOutput {
            head: vec![
                OutputEvent {
                    seq: 1.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "first".to_string(),
                    ..Default::default()
                },
                OutputEvent {
                    seq: 2.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "second".to_string(),
                    ..Default::default()
                },
            ],
            tail: vec![
                OutputEvent {
                    seq: 9.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "ninth".to_string(),
                    ..Default::default()
                },
                OutputEvent {
                    seq: 10.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "tenth".to_string(),
                    ..Default::default()
                },
            ],
            total_count: 10,
            ..Default::default()
        };

        let mut result = String::new();
        format_output_events(&output, &mut result);

        assert!(result.contains("[seq:1 stdout] first"));
        assert!(result.contains("[seq:2 stdout] second"));
        assert!(result.contains("6 events omitted"));
        assert!(result.contains("[seq:9 stdout] ninth"));
        assert!(result.contains("[seq:10 stdout] tenth"));
    }

    #[test]
    fn format_output_events_no_omission_at_capacity() {
        let output = BufferedOutput {
            head: vec![
                OutputEvent {
                    seq: 1.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "first".to_string(),
                    ..Default::default()
                },
                OutputEvent {
                    seq: 2.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "second".to_string(),
                    ..Default::default()
                },
            ],
            tail: vec![
                OutputEvent {
                    seq: 3.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "third".to_string(),
                    ..Default::default()
                },
                OutputEvent {
                    seq: 4.into(),
                    category: Some(OutputCategory::Stdout),
                    output: "fourth".to_string(),
                    ..Default::default()
                },
            ],
            total_count: 4,
            ..Default::default()
        };

        let mut result = String::new();
        format_output_events(&output, &mut result);

        assert!(result.contains("[seq:1 stdout] first"));
        assert!(result.contains("[seq:2 stdout] second"));
        assert!(result.contains("[seq:3 stdout] third"));
        assert!(result.contains("[seq:4 stdout] fourth"));
        assert!(!result.contains("events omitted"));
    }

    #[test]
    fn format_output_event_should_handle_multibyte_utf8_truncation() {
        // Regression test for a production crash where box-drawing characters
        // (━ = \u2501, 3 bytes in UTF-8) caused &line[..150] to land
        // mid-character, panicking and poisoning the session tracker mutex.
        //
        // Layout: "│ ┏" (7 bytes) + 50×"━" (150 bytes) = 157 bytes total.
        // Byte 150 falls inside "━" at bytes 148..151, which is not a char
        // boundary. The fix uses floor_char_boundary to truncate safely.
        let line = format!("│ ┏{}", "━".repeat(50));
        assert!(line.len() > MAX_OUTPUT_LINE_LENGTH);
        assert!(!line.is_char_boundary(MAX_OUTPUT_LINE_LENGTH));

        let event = OutputEvent {
            seq: 1.into(),
            category: Some(OutputCategory::Stdout),
            output: line,
            ..Default::default()
        };

        let mut result = String::new();
        format_output_event(&event, &mut result);

        // floor_char_boundary(150) rounds down to byte 148 (start of the
        // 48th "━"), so 47 box-drawing chars survive truncation.
        let expected = format!("[seq:1 stdout] │ ┏{}...\n", "━".repeat(47));
        assert_eq!(result, expected);

        let prefix = "[seq:1 stdout] ";
        let suffix = "...\n";
        let display = &result[prefix.len()..result.len() - suffix.len()];
        assert_eq!(
            display.len(),
            MAX_OUTPUT_LINE_LENGTH - 2,
            "floor_char_boundary should round down by 2 bytes (byte 150 is \
             2 bytes into the 3-byte '━' character at bytes 148..151)",
        );
    }

    #[test]
    fn format_envelope_with_both() {
        let output = format_envelope("result text", Some("header"), Some("footer"));
        assert_eq!(output, "header\n___\n\nresult text\n\n___\nfooter");
    }

    #[test]
    fn format_envelope_header_only() {
        let output = format_envelope("result text", Some("header"), None);
        assert_eq!(output, "header\n___\n\nresult text");
    }

    #[test]
    fn format_envelope_footer_only() {
        let output = format_envelope("result text", None, Some("footer"));
        assert_eq!(output, "result text\n\n___\nfooter");
    }

    #[test]
    fn format_envelope_neither() {
        let output = format_envelope("result text", None, None);
        assert_eq!(output, "result text");
    }

    #[test]
    fn format_context_footer_with_exception_filters() {
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
        let footer = format_context_footer(&ctx, &config).expect("footer should be Some");
        // Defensive sort means raised renders before uncaught regardless of input order.
        assert!(
            footer.contains("Exception breakpoints:\n  raised (condition: x>5)\n  uncaught\n"),
            "unexpected footer:\n{footer}"
        );
    }

    #[test]
    fn format_context_footer_with_source_and_exception_filters() {
        // Exercise both blocks together so the spacing/separation between
        // them is locked in by an end-to-end test.
        let ctx = ResponseContext {
            breakpoints: HashMap::from([(
                "/test.py".to_string(),
                vec![BreakpointInfo {
                    line: 10,
                    verified: true,
                    ..Default::default()
                }],
            )]),
            installed_exception_filters: vec![ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            }],
            ..Default::default()
        };
        let config = ContextConfig::default();
        let footer = format_context_footer(&ctx, &config).expect("footer should be Some");
        // Source-line block first, then exception block, both terminated
        // by their own newlines from `writeln!`.
        assert!(
            footer.contains("Source-line breakpoints:\n  /test.py: lines [10]\n"),
            "missing source-line block:\n{footer}"
        );
        assert!(
            footer.contains("Exception breakpoints:\n  uncaught\n"),
            "missing exception-filter block:\n{footer}"
        );
        // Source block appears before exception block.
        let src_pos = footer.find("Source-line breakpoints:").unwrap();
        let exc_pos = footer.find("Exception breakpoints:").unwrap();
        assert!(
            src_pos < exc_pos,
            "source block should precede exception block:\n{footer}"
        );
    }

    #[test]
    fn format_context_footer_exception_filters_hidden_by_config() {
        let ctx = ResponseContext {
            installed_exception_filters: vec![ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: None,
            }],
            ..Default::default()
        };
        let config = ContextConfig {
            show_exception_breakpoints: false,
            ..Default::default()
        };
        // Footer should be empty (no other content either) — exception
        // filters are the only data, and we asked for them not to be shown.
        let footer = format_context_footer(&ctx, &config);
        assert!(
            footer.is_none(),
            "expected no footer when show_exception_breakpoints=false, got: {footer:?}"
        );
    }
}
