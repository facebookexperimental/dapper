// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::HashMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::BreakpointInfo;
use crate::BufferedOutput;
use crate::ExceptionFilterEntry;
use crate::SessionInfo;
use crate::VersionedExecutionStateSummary;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_sessions: Vec<SessionInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_state: Option<VersionedExecutionStateSummary>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub breakpoints: HashMap<String, Vec<BreakpointInfo>>,
    /// Exception breakpoint filters currently installed at the adapter,
    /// sorted by filter id. Populated by the proxy tracker when
    /// `ContextConfig::show_exception_breakpoints` is enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installed_exception_filters: Vec<ExceptionFilterEntry>,
    #[serde(default)]
    pub output: BufferedOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_history_file: Option<PathBuf>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::enums::StoppedReason;

    use super::*;
    use crate::ExecutionStateSummary;
    use crate::ExecutionStatus;

    #[test]
    fn serde_round_trip() {
        let original = ResponseContext {
            session: Some(SessionInfo::generate(
                "test-123".into(),
                None,
                None,
                None,
                None,
            )),
            execution_state: Some(VersionedExecutionStateSummary {
                version: 1,
                state: ExecutionStateSummary {
                    status: ExecutionStatus::Stopped,
                    thread_id: Some(ThreadId(5)),
                    stop_reason: Some(StoppedReason::Breakpoint),
                    ..Default::default()
                },
            }),
            breakpoints: HashMap::from([(
                "/test.py".to_string(),
                vec![BreakpointInfo {
                    line: 10,
                    verified: true,
                    ..Default::default()
                }],
            )]),
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: ResponseContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn serde_forward_compat() {
        let json = r#"{
            "otherSessions": [],
            "breakpoints": {},
            "output": {"head": [], "tail": [], "totalCount": 0},
            "futureContextField": {"nested": true}
        }"#;

        let result: ResponseContext = serde_json::from_str(json).expect("deserialize");
        assert!(result.session.is_none());
        assert!(result.other_sessions.is_empty());
        assert_eq!(
            result.extra.get("futureContextField"),
            Some(&serde_json::json!({"nested": true}))
        );
    }

    #[test]
    fn serde_missing_fields_use_defaults() {
        let result: ResponseContext = serde_json::from_str("{}").expect("deserialize");
        assert!(result.session.is_none());
        assert!(result.other_sessions.is_empty());
        assert!(result.execution_state.is_none());
        assert!(result.breakpoints.is_empty());
        assert!(result.installed_exception_filters.is_empty());
        assert!(result.output_history_file.is_none());
    }

    #[test]
    fn serde_with_installed_exception_filters() {
        let original = ResponseContext {
            installed_exception_filters: vec![
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: Some("x>5".to_string()),
                },
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
            ],
            ..Default::default()
        };

        let json = serde_json::to_string(&original).expect("serialize");
        assert!(
            json.contains(r#""installedExceptionFilters""#),
            "expected camelCase field name: {json}"
        );
        let deserialized: ResponseContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, deserialized);
    }
}
