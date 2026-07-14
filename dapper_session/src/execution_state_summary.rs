// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt::Write;

use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::enums::StoppedReason;
use serde::Deserialize;
use serde::Serialize;

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::Display
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "lowercase")]
pub enum ExecutionStatus {
    #[default]
    Unknown,
    Running,
    Stopped,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStateSummary {
    pub status: ExecutionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StoppedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_information: Option<String>,
}

/// Wrapper around [`ExecutionStateSummary`] that includes a monotonically increasing
/// version counter, used for change-detection by the proxy server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VersionedExecutionStateSummary {
    pub version: u64,
    #[serde(flatten)]
    pub state: ExecutionStateSummary,
}

impl ExecutionStateSummary {
    pub fn format_summary(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(result, "Current execution status: {}", self.status);
        if let Some(ref reason) = self.stop_reason {
            let _ = writeln!(result, "Stop reason: {}", reason);
        }
        if let Some(ref tid) = self.thread_id {
            let _ = writeln!(result, "Thread: {}", tid);
        }
        if let Some(ref description) = self.description {
            let _ = writeln!(result, "Description: {}", description);
        }
        if let Some(ref additional_information) = self.additional_information {
            let _ = writeln!(result, "Additional information: {}", additional_information);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let summary = ExecutionStateSummary {
            status: ExecutionStatus::Stopped,
            thread_id: Some(ThreadId(5)),
            stop_reason: Some(StoppedReason::Breakpoint),
            description: Some("hit breakpoint".to_string()),
            additional_information: Some("extra info".to_string()),
        };

        let json = serde_json::to_string(&summary).expect("serialize");
        let deserialized: ExecutionStateSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(summary, deserialized);
    }

    #[test]
    fn versioned_serde_round_trip() {
        let versioned = VersionedExecutionStateSummary {
            version: 42,
            state: ExecutionStateSummary {
                status: ExecutionStatus::Stopped,
                thread_id: Some(ThreadId(5)),
                stop_reason: Some(StoppedReason::Breakpoint),
                description: Some("hit breakpoint".to_string()),
                additional_information: Some("extra info".to_string()),
            },
        };

        let json = serde_json::to_string(&versioned).expect("serialize");
        let deserialized: VersionedExecutionStateSummary =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(versioned, deserialized);
    }

    #[test]
    fn format_summary_stopped_with_all_fields() {
        let summary = ExecutionStateSummary {
            status: ExecutionStatus::Stopped,
            thread_id: Some(ThreadId(5)),
            stop_reason: Some(StoppedReason::Breakpoint),
            description: Some("hit breakpoint".to_string()),
            additional_information: Some("extra info".to_string()),
        };

        assert_eq!(
            summary.format_summary(),
            "Current execution status: stopped\n\
             Stop reason: breakpoint\n\
             Thread: 5\n\
             Description: hit breakpoint\n\
             Additional information: extra info\n"
        );
    }

    #[test]
    fn format_summary_running() {
        let summary = ExecutionStateSummary {
            status: ExecutionStatus::Running,
            ..Default::default()
        };

        assert_eq!(
            summary.format_summary(),
            "Current execution status: running\n"
        );
    }

    #[test]
    fn format_summary_exited() {
        let summary = ExecutionStateSummary {
            status: ExecutionStatus::Exited,
            ..Default::default()
        };

        assert_eq!(
            summary.format_summary(),
            "Current execution status: exited\n"
        );
    }

    #[test]
    fn serde_json_excludes_version() {
        let summary = ExecutionStateSummary {
            status: ExecutionStatus::Stopped,
            thread_id: Some(ThreadId(5)),
            stop_reason: Some(StoppedReason::Breakpoint),
            description: Some("hit breakpoint".to_string()),
            additional_information: Some("extra info".to_string()),
        };

        assert_eq!(
            serde_json::to_value(&summary).unwrap(),
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
    fn versioned_serde_json_includes_version() {
        let versioned = VersionedExecutionStateSummary {
            version: 42,
            state: ExecutionStateSummary {
                status: ExecutionStatus::Running,
                ..Default::default()
            },
        };

        assert_eq!(
            serde_json::to_value(&versioned).unwrap(),
            serde_json::json!({
                "version": 42,
                "status": "running",
            })
        );
    }
}
