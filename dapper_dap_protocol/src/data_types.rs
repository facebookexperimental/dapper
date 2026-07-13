// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::enums::BreakpointModeApplicability;
use crate::enums::BreakpointReason;
use crate::enums::ChecksumAlgorithm;
use crate::enums::ColumnDescriptorType;
use crate::enums::CompletionItemType;
use crate::enums::DataBreakpointAccessType;
use crate::enums::DisassembledInstructionPresentationHint;
use crate::enums::ExceptionBreakMode;
use crate::enums::ScopePresentationHint;
use crate::enums::SourcePresentationHint;
use crate::enums::StackFramePresentationHint;
use crate::enums::VariablePresentationHintAttributes;
use crate::enums::VariablePresentationHintKind;
use crate::enums::VariablePresentationHintVisibility;

/// Parse an `i64` from a JSON value that is either a number or a
/// string containing an integer. LLM clients (MCP tool callers)
/// sometimes send integer parameters as strings.
pub fn i64_from_value(value: &Value) -> Result<i64, String> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| format!("expected an integer, got {n}")),
        Value::String(s) => s
            .parse::<i64>()
            .map_err(|_| format!("invalid integer string: {s:?}")),
        other => Err(format!("expected integer or string, got {other}")),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IntOrString {
    Int(i64),
    String(String),
}

impl Default for IntOrString {
    fn default() -> Self {
        IntOrString::Int(0)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    derive_more::From,
    derive_more::Into
)]
#[serde(transparent)]
pub struct Seq(pub i64);

impl Seq {
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Returns the underlying integer value.
    ///
    /// Useful at `tracing` call sites where the field should be recorded as
    /// an integer rather than the Display string representation.
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    derive_more::From,
    derive_more::Into
)]
#[serde(try_from = "Value", into = "i64")]
pub struct ThreadId(pub i64);

impl ThreadId {
    /// Returns the underlying integer value.
    ///
    /// Useful at `tracing` call sites where the field should be recorded as
    /// an integer rather than the Display string representation.
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl TryFrom<Value> for ThreadId {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        i64_from_value(&value).map(ThreadId)
    }
}

impl std::str::FromStr for ThreadId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<i64>().map(ThreadId)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    derive_more::From,
    derive_more::Into
)]
#[serde(try_from = "Value", into = "i64")]
pub struct FrameId(pub i64);

impl FrameId {
    /// Returns the underlying integer value.
    ///
    /// Useful at `tracing` call sites where the field should be recorded as
    /// an integer rather than the Display string representation.
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl TryFrom<Value> for FrameId {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        i64_from_value(&value).map(FrameId)
    }
}

impl std::str::FromStr for FrameId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<i64>().map(FrameId)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    derive_more::From,
    derive_more::Into
)]
#[serde(try_from = "Value", into = "i64")]
pub struct VariablesReference(pub i64);

impl VariablesReference {
    pub fn has_children(self) -> bool {
        self.0 > 0
    }

    /// Returns the underlying integer value.
    ///
    /// Useful at `tracing` call sites where the field should be recorded as
    /// an integer rather than the Display string representation.
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl TryFrom<Value> for VariablesReference {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        i64_from_value(&value).map(VariablesReference)
    }
}

impl std::str::FromStr for VariablesReference {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<i64>().map(VariablesReference)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    derive_more::From,
    derive_more::Into
)]
#[serde(transparent)]
pub struct BreakpointId(pub i64);

impl BreakpointId {
    /// Returns the underlying integer value.
    ///
    /// Useful at `tracing` call sites where the field should be recorded as
    /// an integer rather than the Display string representation.
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_reference: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<SourcePresentationHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<Source>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksums: Option<Vec<Checksum>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    #[serde(default)]
    pub id: ThreadId,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrame {
    #[serde(default)]
    pub id: FrameId,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default)]
    pub line: i64,
    #[serde(default)]
    pub column: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_restart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_pointer_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_id: Option<IntOrString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<StackFramePresentationHint>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<ScopePresentationHint>,
    #[serde(default)]
    pub variables_reference: VariablesReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
    #[serde(default)]
    pub expensive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
}

impl Scope {
    pub fn is_locals(&self) -> bool {
        self.presentation_hint.as_ref() == Some(&ScopePresentationHint::Locals)
            || self.name.to_lowercase() == "locals"
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub var_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<VariablePresentationHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate_name: Option<String>,
    #[serde(default)]
    pub variables_reference: VariablesReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_location_reference: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_location_reference: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablePresentationHint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<VariablePresentationHintKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Vec<VariablePresentationHintAttributes>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<VariablePresentationHintVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lazy: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<BreakpointId>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<BreakpointReason>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointLocation {
    #[serde(default)]
    pub line: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakpoint {
    #[serde(default)]
    pub line: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionBreakpoint {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataBreakpoint {
    #[serde(default)]
    pub data_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_type: Option<DataBreakpointAccessType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionBreakpoint {
    #[serde(default)]
    pub instruction_reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Module {
    #[serde(default)]
    pub id: IntOrString,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_optimized: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_user_code: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_time_stamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_range: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DapMessage {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_telemetry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_user: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checksum {
    #[serde(default)]
    pub algorithm: ChecksumAlgorithm,
    #[serde(default)]
    pub checksum: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDescriptor {
    #[serde(default)]
    pub attribute_name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<ColumnDescriptorType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionBreakpointsFilter {
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_condition: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointMode {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub applies_to: Vec<BreakpointModeApplicability>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueFormat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrameFormat {
    #[serde(flatten)]
    pub value_format: ValueFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_types: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_names: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_values: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_all: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionFilterOptions {
    #[serde(default)]
    pub filter_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<ExceptionPathSegment>>,
    #[serde(default)]
    pub break_mode: ExceptionBreakMode,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionPathSegment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negate: Option<bool>,
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_exception: Option<Vec<ExceptionDetails>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisassembledInstruction {
    #[serde(default)]
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_bytes: Option<String>,
    #[serde(default)]
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<DisassembledInstructionPresentationHint>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepInTarget {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GotoTarget {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub line: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_pointer_reference: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<CompletionItemType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_start: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_length: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_i64_returns_inner_value() {
        assert_eq!(Seq(42).as_i64(), 42);
        assert_eq!(ThreadId(-7).as_i64(), -7);
        assert_eq!(FrameId(0).as_i64(), 0);
        assert_eq!(VariablesReference(123).as_i64(), 123);
        assert_eq!(BreakpointId(i64::MAX).as_i64(), i64::MAX);
    }
}
