// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::data_types::Breakpoint;
use dapper_dap_protocol::data_types::BreakpointId;
use dapper_dap_protocol::data_types::Source;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointInfo {
    #[serde(default)]
    pub line: i64,
    #[serde(default)]
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<BreakpointId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_message: Option<String>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl From<&BreakpointInfo> for SourceBreakpoint {
    fn from(bp: &BreakpointInfo) -> Self {
        Self {
            line: bp.line,
            condition: bp.condition.clone(),
            log_message: bp.log_message.clone(),
            ..Default::default()
        }
    }
}

impl BreakpointInfo {
    pub fn to_dap_breakpoint(&self, source_path: &str, source_name: &str) -> Breakpoint {
        Breakpoint {
            id: self.id,
            verified: self.verified,
            line: Some(self.line),
            source: Some(Source {
                name: Some(source_name.to_string()),
                path: Some(source_path.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}
