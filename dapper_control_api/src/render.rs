// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;

use dapper_config::DapperConfig;
use dapper_config::OutputFormat;
use serde::Serialize;

use crate::ControlPlaneResult;
use crate::ResponseContext;
use crate::envelope::format_context_footer;
use crate::envelope::format_context_header;
use crate::envelope::format_envelope;
use crate::response_context_output::ResponseContextOutput;

/// Render a `ControlPlaneResult` using the output format from the config.
pub fn render<T: fmt::Display + Serialize>(
    result: &ControlPlaneResult<T>,
    config: &DapperConfig,
) -> anyhow::Result<String> {
    match config.output_format {
        OutputFormat::Plaintext => Ok(render_plaintext(result, config)),
        OutputFormat::Json => render_json(result, config),
    }
}

/// Render a `ControlPlaneResult` as a JSON object with `result` and optional `context` fields.
/// The context mirrors what `render_with_envelope` shows in plaintext (context header + footer),
/// rather than dumping the entire `ResponseContext`.
pub fn render_json<T: Serialize>(
    result: &ControlPlaneResult<T>,
    config: &DapperConfig,
) -> anyhow::Result<String> {
    let mut map = serde_json::Map::new();
    map.insert("result".to_string(), serde_json::to_value(&result.result)?);
    if let Some(ref ctx) = result.context {
        let context = ResponseContextOutput::from_response_context(ctx, &config.context);
        if !context.is_empty() {
            map.insert("context".to_string(), serde_json::to_value(&context)?);
        }
    }
    Ok(serde_json::to_string(&map)?)
}

pub fn render_plaintext<T: fmt::Display>(
    result: &ControlPlaneResult<T>,
    config: &DapperConfig,
) -> String {
    let text = result.result.to_string();
    render_with_envelope(&text, result.context.as_ref(), config)
}

fn render_with_envelope(
    text: &str,
    context: Option<&ResponseContext>,
    config: &DapperConfig,
) -> String {
    let header = if config.context.show_session {
        context
            .and_then(|ctx| ctx.session.as_ref())
            .map(|info| format_context_header(Some(info)))
    } else {
        None
    };

    let footer = context.and_then(|ctx| format_context_footer(ctx, &config.context));

    format_envelope(text, header.as_deref(), footer.as_deref())
}

#[cfg(test)]
mod tests {
    use dapper_session::SessionInfo;

    use super::*;
    use crate::BreakpointInfo;
    use crate::ExecutionStateSummary;
    use crate::ExecutionStatus;
    use crate::ThreadsResult;
    use crate::VersionedExecutionStateSummary;

    #[test]
    fn render_structured_without_context() {
        let result = ControlPlaneResult {
            result: ThreadsResult::default(),
            context: None,
        };
        let config = DapperConfig::default();
        let output = render_plaintext(&result, &config);
        assert_eq!(output, ThreadsResult::default().to_string());
    }

    #[test]
    fn render_structured_with_session_context() {
        let mut session = SessionInfo::generate("test-123".into(), None, None, None, None);
        session.session_type = Some("debugpy".to_string());

        let result = ControlPlaneResult {
            result: ThreadsResult::default(),
            context: Some(ResponseContext {
                session: Some(session),
                ..Default::default()
            }),
        };
        let config = DapperConfig::default();
        let output = render_plaintext(&result, &config);

        assert!(output.contains("Session: test-123"));
        assert!(output.contains("Debugger: debugpy"));
        assert!(output.contains("___"));
        assert!(output.contains("Threads:"));
    }

    #[test]
    fn render_json_without_context() {
        let result = ControlPlaneResult {
            result: ThreadsResult::default(),
            context: None,
        };
        let config = DapperConfig::default();
        let output = render_json(&result, &config).expect("render_json should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("output should be valid JSON");

        assert_eq!(
            parsed,
            serde_json::json!({
                "result": {
                    "threads": [],
                },
            })
        );
    }

    #[test]
    fn render_json_with_context() {
        let mut session = SessionInfo::generate("test-456".into(), None, None, None, None);
        session.session_type = Some("debugpy".to_string());

        let result = ControlPlaneResult {
            result: ThreadsResult::default(),
            context: Some(ResponseContext {
                session: Some(session),
                execution_state: Some(VersionedExecutionStateSummary {
                    version: 1,
                    state: ExecutionStateSummary {
                        status: ExecutionStatus::Stopped,
                        ..Default::default()
                    },
                }),
                breakpoints: std::collections::HashMap::from([(
                    "/test.py".to_string(),
                    vec![BreakpointInfo {
                        line: 42,
                        verified: true,
                        ..Default::default()
                    }],
                )]),
                ..Default::default()
            }),
        };
        let config = DapperConfig::default();
        let output = render_json(&result, &config).expect("render_json should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("output should be valid JSON");

        assert_eq!(
            parsed,
            serde_json::json!({
                "result": {
                    "threads": [],
                },
                "context": {
                    "session": {
                        "sessionId": "test-456",
                        "debugger": "debugpy",
                    },
                    "executionState": {
                        "status": "stopped",
                    },
                    "breakpoints": {
                        "files": [{"path": "/test.py", "lines": [42]}],
                        "totalFiles": 1,
                        "shownFiles": 1,
                    },
                },
            })
        );
    }
}
