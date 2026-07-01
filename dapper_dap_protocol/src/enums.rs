// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use serde::Deserialize;
use serde::Serialize;
use strum::AsRefStr;
use strum::Display;

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum StoppedReason {
    #[default]
    Step,
    Breakpoint,
    Exception,
    Pause,
    Entry,
    Goto,
    #[serde(rename = "function breakpoint")]
    #[strum(serialize = "function breakpoint")]
    FunctionBreakpoint,
    #[serde(rename = "data breakpoint")]
    #[strum(serialize = "data breakpoint")]
    DataBreakpoint,
    #[serde(rename = "instruction breakpoint")]
    #[strum(serialize = "instruction breakpoint")]
    InstructionBreakpoint,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum OutputCategory {
    Console,
    Important,
    Stdout,
    Stderr,
    Telemetry,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum OutputGroup {
    Start,
    StartCollapsed,
    End,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum InvalidatedAreas {
    All,
    Stacks,
    Threads,
    Variables,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ThreadReason {
    #[default]
    Started,
    Exited,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum BreakpointEventReason {
    Changed,
    #[default]
    New,
    Removed,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum SourcePresentationHint {
    Normal,
    Emphasize,
    Deemphasize,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum StackFramePresentationHint {
    Normal,
    Label,
    Subtle,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ScopePresentationHint {
    Arguments,
    Locals,
    Registers,
    ReturnValue,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum VariablePresentationHintKind {
    Property,
    Method,
    Class,
    Data,
    Event,
    BaseClass,
    InnerClass,
    Interface,
    MostDerivedClass,
    Virtual,
    DataBreakpoint,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum VariablePresentationHintAttributes {
    Static,
    Constant,
    ReadOnly,
    RawString,
    HasObjectId,
    CanHaveObjectId,
    HasSideEffects,
    HasDataBreakpoint,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum VariablePresentationHintVisibility {
    Public,
    Private,
    Protected,
    Internal,
    Final,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
pub enum ChecksumAlgorithm {
    #[serde(rename = "MD5")]
    #[strum(serialize = "MD5")]
    Md5,
    #[serde(rename = "SHA1")]
    #[strum(serialize = "SHA1")]
    Sha1,
    #[default]
    #[serde(rename = "SHA256")]
    #[strum(serialize = "SHA256")]
    Sha256,
    #[serde(rename = "timestamp")]
    #[strum(serialize = "timestamp")]
    Timestamp,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ColumnDescriptorType {
    String,
    Number,
    Boolean,
    #[serde(rename = "unixTimestampUTC")]
    #[strum(serialize = "unixTimestampUTC")]
    UnixTimestampUtc,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum CompletionItemType {
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Text,
    Color,
    File,
    Reference,
    #[serde(rename = "customcolor")]
    #[strum(serialize = "customcolor")]
    CustomColor,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum DataBreakpointAccessType {
    Read,
    Write,
    ReadWrite,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ExceptionBreakMode {
    #[default]
    Never,
    Always,
    Unhandled,
    UserUnhandled,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum SteppingGranularity {
    Statement,
    Line,
    Instruction,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum EvaluateContext {
    Watch,
    Repl,
    Hover,
    Clipboard,
    Variables,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum PathFormat {
    Path,
    Uri,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum DisassembledInstructionPresentationHint {
    Normal,
    Invalid,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ModuleEventReason {
    #[default]
    New,
    Changed,
    Removed,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum LoadedSourceEventReason {
    #[default]
    New,
    Changed,
    Removed,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ProcessStartMethod {
    Launch,
    Attach,
    AttachForSuspendedLaunch,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum RunInTerminalKind {
    Integrated,
    External,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    AsRefStr,
    Display
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum StartDebuggingType {
    #[default]
    Launch,
    Attach,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum BreakpointReason {
    Pending,
    Failed,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum VariablesFilter {
    Indexed,
    Named,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum BreakpointModeApplicability {
    Source,
    Exception,
    Data,
    Instruction,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr, Display)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum StartDebuggingOutputPresentation {
    Separate,
    MergeWithParent,
    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Other(String),
}
