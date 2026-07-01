// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use strum::AsRefStr;

use crate::capabilities::Capabilities;
use crate::data_types::Breakpoint;
use crate::data_types::BreakpointId;
use crate::data_types::FrameId;
use crate::data_types::Module;
use crate::data_types::Source;
use crate::data_types::ThreadId;
use crate::data_types::VariablesReference;
use crate::enums::BreakpointEventReason;
use crate::enums::InvalidatedAreas;
use crate::enums::LoadedSourceEventReason;
use crate::enums::ModuleEventReason;
use crate::enums::OutputCategory;
use crate::enums::OutputGroup;
use crate::enums::ProcessStartMethod;
use crate::enums::StoppedReason;
use crate::enums::ThreadReason;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializedEventBody {
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoppedEventBody {
    pub reason: StoppedReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_focus_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_threads_stopped: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_breakpoint_ids: Option<Vec<BreakpointId>>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuedEventBody {
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_threads_continued: Option<bool>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitedEventBody {
    pub exit_code: i64,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminatedEventBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<Value>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEventBody {
    pub reason: ThreadReason,
    pub thread_id: ThreadId,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEventBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<OutputCategory>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<OutputGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables_reference: Option<VariablesReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_reference: Option<i64>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointEventBody {
    pub reason: BreakpointEventReason,
    pub breakpoint: Breakpoint,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleEventBody {
    pub reason: ModuleEventReason,
    pub module: Module,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedSourceEventBody {
    pub reason: LoadedSourceEventReason,
    pub source: Source,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEventBody {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_process_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_local_process: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_method: Option<ProcessStartMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer_size: Option<i64>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesEventBody {
    pub capabilities: Capabilities,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressStartEventBody {
    pub progress_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentage: Option<f64>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdateEventBody {
    pub progress_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentage: Option<f64>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEndEventBody {
    pub progress_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidatedEventBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub areas: Option<Vec<InvalidatedAreas>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_frame_id: Option<FrameId>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEventBody {
    pub memory_reference: String,
    pub offset: i64,
    pub count: i64,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRefStr)]
#[serde(tag = "event", content = "body", rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum EventKind {
    Initialized(Option<InitializedEventBody>),
    Stopped(StoppedEventBody),
    Continued(ContinuedEventBody),
    Exited(ExitedEventBody),
    Terminated(Option<TerminatedEventBody>),
    Thread(ThreadEventBody),
    Output(OutputEventBody),
    Breakpoint(BreakpointEventBody),
    Module(ModuleEventBody),
    LoadedSource(LoadedSourceEventBody),
    Process(ProcessEventBody),
    Capabilities(CapabilitiesEventBody),
    ProgressStart(ProgressStartEventBody),
    ProgressUpdate(ProgressUpdateEventBody),
    ProgressEnd(ProgressEndEventBody),
    Invalidated(InvalidatedEventBody),
    Memory(MemoryEventBody),
    #[serde(untagged)]
    Unknown(UnknownEvent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownEvent {
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl EventKind {
    pub fn event_name(&self) -> &str {
        match self {
            Self::Unknown(u) => &u.event,
            _ => self.as_ref(),
        }
    }
}
