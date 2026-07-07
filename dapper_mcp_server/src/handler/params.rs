// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Tool parameter types, their lenient deserializers, and the JSON
//! schema helpers that keep the generated schemas Claude-API compatible.

use dapper_control_api::NavigationType;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_session::SessionId;
use rmcp::serde_json;
use schemars::JsonSchema;
use schemars::Schema;
use schemars::generate::SchemaGenerator;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EmptyParams {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SessionTargeted<T> {
    /// Id of the debug session to send the request to. If left unspecified, requests are sent to the last session this MCP server used (if still active), or the oldest active session otherwise.
    #[serde(default)]
    #[schemars(schema_with = "optional_schema::<String>")]
    pub(super) session_id: Option<SessionId>,
    #[serde(flatten)]
    pub inner: T,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StackTraceRequest {
    /// The thread id to execute the command on. To obtain thread ids, call `debug_threads_command` first.
    #[schemars(schema_with = "integer_schema")]
    pub(super) thread_id: ThreadId,
    /// The index of the first frame to return. If omitted, frames start at index 0.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_schema::<i64>")]
    pub(super) start_frame: Option<i64>,
    /// Maximum number of stack frames to return. If not specified, uses the configured default. Set to 0 to return all frames.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_schema::<i64>")]
    pub(super) levels: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FrameIdRequest {
    /// The frame id to execute the command on. To obtain frame ids, call `debug_stack_trace_command` first.
    #[schemars(schema_with = "integer_schema")]
    pub(super) frame_id: FrameId,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct VariablesReferenceRequest {
    /// The variable reference to execute the command on. Variable references are obtained from both `debug_scopes_command` (or other `debug_variables_command` calls when we want to look at nested variables). Note that variable references need to be re-obtained every time the debugger stops.
    #[schemars(schema_with = "integer_schema")]
    pub(super) variables_reference: VariablesReference,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetVariableRequest {
    /// The variable reference to execute the command on. Variable references are obtained from both `debug_scopes_command` (or other `debug_variables_command` calls when we want to look at nested variables). Note that variable references need to be re-obtained every time the debugger stops.
    #[schemars(schema_with = "integer_schema")]
    pub(super) variables_reference: VariablesReference,
    /// Name of the variable to set
    pub(super) name: String,
    /// New value for the variable. Note that string values need to be quoted with single quotes.
    pub(super) value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NavigateRequest {
    /// The thread id to execute the command on. To obtain thread ids, call `debug_threads_command` first.
    #[schemars(schema_with = "integer_schema")]
    pub(super) thread_id: ThreadId,
    /// The type of navigation to perform: "step_in" (step into functions), "step_over" (step over functions - most commonly used), "step_out" (step out of current frame), "continue" (resume execution until breakpoint or exit), "pause" (pause a running program), "step_back" (step back one source line, requires adapter `supportsStepBack`), or "reverse_continue" (resume reverse execution until a breakpoint or the start of recording, requires adapter `supportsStepBack`)
    pub(super) navigation_type: NavigationType,
    /// When true, only the specified thread is resumed; other suspended threads remain paused. Requires the adapter to advertise `supportsSingleThreadExecutionRequests`. If omitted or false, all threads are resumed.
    #[serde(default)]
    #[schemars(schema_with = "optional_schema::<bool>")]
    pub(super) single_thread: Option<bool>,
}

#[derive(Debug, serde::Deserialize, JsonSchema)]
struct BreakpointSpecObject {
    /// The line number where the breakpoint should be set.
    #[serde(deserialize_with = "deserialize_string_or_int")]
    #[schemars(schema_with = "integer_schema")]
    line: i64,
    /// An optional expression that controls when a breakpoint is hit. The breakpoint only stops execution when this expression evaluates to true.
    #[serde(default)]
    #[schemars(schema_with = "optional_schema::<String>")]
    condition: Option<String>,
    /// If specified, the debugger will log this message instead of stopping at the breakpoint. Expressions within {} are interpolated.
    #[serde(default, rename = "logMessage")]
    #[schemars(schema_with = "optional_schema::<String>")]
    log_message: Option<String>,
}

/// Lenient wrapper around `BreakpointSpecObject`: also accepts the other shapes LLM clients
/// send (bare line numbers, stringified JSON).
#[derive(Debug, serde::Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub struct BreakpointSpec {
    pub(super) line: i64,
    pub(super) condition: Option<String>,
    pub(super) log_message: Option<String>,
}

impl From<BreakpointSpecObject> for BreakpointSpec {
    fn from(obj: BreakpointSpecObject) -> Self {
        Self {
            line: obj.line,
            condition: obj.condition,
            log_message: obj.log_message,
        }
    }
}

impl BreakpointSpec {
    fn from_line(line: i64) -> Self {
        Self {
            line,
            condition: None,
            log_message: None,
        }
    }
}

impl JsonSchema for BreakpointSpec {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "BreakpointSpec".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        BreakpointSpecObject::json_schema(generator)
    }
}

impl TryFrom<serde_json::Value> for BreakpointSpec {
    type Error = anyhow::Error;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        match &value {
            serde_json::Value::Object(_) => {
                let obj: BreakpointSpecObject = serde_json::from_value(value)?;
                Ok(obj.into())
            }
            serde_json::Value::String(s) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                    return BreakpointSpec::try_from(parsed);
                }
                let line: i64 = s.parse().map_err(|_| {
                    anyhow::anyhow!(
                        "invalid breakpoint spec: expected a JSON object like \
                         {{\"line\": 10}} or a line number, got {s:?}"
                    )
                })?;
                Ok(BreakpointSpec::from_line(line))
            }
            serde_json::Value::Number(n) => {
                let line = n
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("breakpoint line must be an integer"))?;
                Ok(BreakpointSpec::from_line(line))
            }
            _ => anyhow::bail!(
                "invalid breakpoint spec: expected a JSON object like {{\"line\": 10}}, \
                 a line number, or a JSON string"
            ),
        }
    }
}

impl From<BreakpointSpec> for SourceBreakpoint {
    fn from(spec: BreakpointSpec) -> Self {
        Self {
            line: spec.line,
            condition: spec.condition,
            log_message: spec.log_message,
            ..Default::default()
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetBreakpointsRequest {
    /// The absolute path to the source file where breakpoints should be set.
    pub(super) source_path: String,
    /// Per-breakpoint specifications. Each entry specifies a line and optional condition/logMessage.
    pub(super) breakpoints: Vec<BreakpointSpec>,
    /// When true, clears all existing breakpoints in the file before adding new ones.
    /// When false (default), new breakpoints are appended to existing ones.
    #[serde(default)]
    pub(super) clear_existing: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetExceptionBreakpointsRequest {
    /// Adapter-advertised exception filter ids (e.g. "raised", "uncaught").
    /// Discover supported ids via `debug_capabilities_command`'s
    /// `exceptionBreakpointFilters` section.
    #[serde(default)]
    pub(super) filters: Vec<String>,
    /// When true, clears all existing exception filters before enabling
    /// these. When false (default), the explicit list is merged with the
    /// currently-installed set; existing conditions on re-specified
    /// filters are preserved.
    #[serde(default)]
    pub(super) clear_existing: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EvaluateRequest {
    /// The input to evaluate in the REPL context. Depending on the debugger, this can be a debugger command or a valid expression in the debugged language to be evaluated.
    pub(super) expression: String,
    /// The stack frame in which to evaluate the expression. If omitted, the expression is evaluated in the global scope. To obtain frame ids, call `debug_stack_trace_command` first.
    #[serde(default)]
    #[schemars(schema_with = "optional_schema::<i64>")]
    pub(super) frame_id: Option<FrameId>,
}

pub(super) fn default_timeout() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn default_stack_depth() -> i64 {
    10
}

fn default_max_threads() -> i64 {
    50
}

/// Hard upper bound on threads enumerated per snapshot, regardless of caller request.
/// Protects against pathological binaries (tens of thousands of threads) where the
/// raw stack output would be unmanageable for both transport and the LLM client.
pub(super) const MAX_THREADS_HARD_CAP: usize = 500;

/// Hard upper bound on stack frames per thread.
pub(super) const MAX_STACK_DEPTH: i64 = 512;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RawDapRequestParams {
    /// The DAP command name (e.g., "threads", "pause", "setExceptionBreakpoints")
    pub(super) command: String,
    /// Arguments as JSON object. Pass {} or omit if no arguments needed.
    #[serde(default)]
    pub(super) arguments: Option<serde_json::Value>,
    /// Wait for stopped/exited events after request (for pause, continue, step commands)
    #[serde(default)]
    pub(super) wait_for_event: bool,
    /// Timeout in seconds for event wait. Default: 60
    #[serde(default = "default_timeout")]
    pub(super) timeout_seconds: u64,
}

fn default_read_count() -> i64 {
    256
}

/// Upper bound on `count` for a single readMemory request (1 MiB). DAP itself
/// has no cap, so without this an MCP client could ask the adapter to materialize
/// arbitrarily large reads.
pub(super) const MAX_READ_BYTES: i64 = 1 << 20;

/// Upper bound on the byte payload of a single writeMemory request (1 MiB).
/// Same reasoning as MAX_READ_BYTES — bound the allocation an MCP client can
/// force before the adapter sees the request.
pub(super) const MAX_WRITE_BYTES: usize = 1 << 20;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadMemoryRequest {
    /// Memory reference address (e.g., "0x7fff5fbff8a0") or expression that evaluates to a memory address. Obtain memory references from `debug_evaluate_command` or variable `memoryReference` fields.
    pub(super) memory_reference: String,
    /// Number of bytes to read. Must be > 0. Default: 256.
    #[serde(
        default = "default_read_count",
        deserialize_with = "deserialize_string_or_int"
    )]
    #[schemars(schema_with = "integer_schema")]
    pub(super) count: i64,
    /// Byte offset from the memory reference.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_schema::<i64>")]
    pub(super) offset: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WriteMemoryRequest {
    /// Memory reference address (e.g., "0x7fff5fbff8a0") or expression that evaluates to a memory address.
    pub(super) memory_reference: String,
    /// Hex string of bytes to write (e.g., "48656C6C6F" to write "Hello"). Each pair of hex digits represents one byte.
    pub(super) data: String,
    /// Byte offset from the memory reference.
    #[serde(default, deserialize_with = "deserialize_optional_string_or_int")]
    #[schemars(schema_with = "optional_schema::<i64>")]
    pub(super) offset: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ThreadSnapshotRequest {
    /// Include stack traces for each thread (default true).
    #[serde(default = "default_true")]
    pub(super) include_stacks: bool,
    /// Maximum stack frames per thread (default 10, hard cap 512).
    #[serde(
        default = "default_stack_depth",
        deserialize_with = "deserialize_string_or_int"
    )]
    #[schemars(schema_with = "integer_schema")]
    pub(super) stack_depth: i64,
    /// Maximum threads to enumerate (default 50, hard cap 500). Protects against
    /// pathological processes with tens of thousands of threads.
    #[serde(
        default = "default_max_threads",
        deserialize_with = "deserialize_string_or_int"
    )]
    #[schemars(schema_with = "integer_schema")]
    pub(super) max_threads: i64,
}

/// Clamp a snapshot request's limits to their hard caps.
pub(super) fn clamp_snapshot_limits(req: &ThreadSnapshotRequest) -> (i64, usize) {
    let stack_depth = req.stack_depth.clamp(1, MAX_STACK_DEPTH);
    let max_threads = (req.max_threads.max(1) as usize).min(MAX_THREADS_HARD_CAP);
    (stack_depth, max_threads)
}

fn integer_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = i64::json_schema(generator);
    schema.remove("format");
    schema
}

/// Convert a schemars-generated `"type": ["T", "null"]` schema into
/// the `"anyOf": [{"type": "T"}, {"type": "null"}]` form that the
/// Claude API accepts (matching Pydantic v2 output).
fn type_array_to_any_of(schema: &mut Schema) {
    if let Some(type_val) = schema.get("type")
        && let Some(type_array) = type_val.as_array()
        && type_array.len() > 1
    {
        let any_of: Vec<serde_json::Value> = type_array
            .iter()
            .map(|t| serde_json::json!({"type": t}))
            .collect();
        schema.remove("type");
        schema.insert("anyOf".to_string(), serde_json::Value::Array(any_of));
    }
}

/// Schema for `Option<T>` fields: strips the `format` annotation (a no-op
/// for types without one) and converts the `"type": [T, "null"]` array to
/// the `anyOf` form. Use as `schema_with = "optional_schema::<i64>"`.
fn optional_schema<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = Option::<T>::json_schema(generator);
    schema.remove("format");
    type_array_to_any_of(&mut schema);
    schema
}

/// Accepts a JSON integer or a JSON string containing an integer.
/// LLM clients sometimes send integer parameters as strings.
fn deserialize_string_or_int<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    use serde::de::Error;

    let value = serde_json::Value::deserialize(deserializer)?;
    dapper_dap_protocol::data_types::i64_from_value(&value).map_err(D::Error::custom)
}

/// Accepts a JSON integer, a JSON string containing an integer, or null.
/// LLM clients sometimes send integer parameters as strings.
fn deserialize_optional_string_or_int<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    use serde::de::Error;

    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => dapper_dap_protocol::data_types::i64_from_value(&v)
            .map(Some)
            .map_err(D::Error::custom),
    }
}

/// Convert a hex string (e.g., "48656C6C6F") into raw bytes.
pub(super) fn hex_string_to_bytes(hex: &str) -> anyhow::Result<Vec<u8>> {
    let hex = hex
        .strip_prefix("0x")
        .or_else(|| hex.strip_prefix("0X"))
        .unwrap_or(hex);
    anyhow::ensure!(
        hex.is_ascii(),
        "hex string must contain only ASCII characters"
    );
    anyhow::ensure!(
        hex.len().is_multiple_of(2),
        "hex string must have an even number of digits"
    );
    let byte_len = hex.len() / 2;
    anyhow::ensure!(
        byte_len <= MAX_WRITE_BYTES,
        "hex payload of {} bytes exceeds maximum of {} bytes",
        byte_len,
        MAX_WRITE_BYTES
    );
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| anyhow::anyhow!("invalid hex at position {}: {:?}", i, &hex[i..i + 2]))
        })
        .collect()
}
