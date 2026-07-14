// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::enums::OutputCategory;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEvent {
    #[serde(default)]
    pub seq: Seq,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<OutputCategory>,
    #[serde(default)]
    pub output: String,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferedOutput {
    #[serde(default)]
    pub head: Vec<OutputEvent>,
    #[serde(default)]
    pub tail: Vec<OutputEvent>,
    #[serde(default)]
    pub total_count: usize,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl BufferedOutput {
    pub fn is_empty(&self) -> bool {
        self.total_count == 0
    }
}
