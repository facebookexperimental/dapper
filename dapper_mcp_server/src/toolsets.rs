// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use clap::ValueEnum;
use strum::AsRefStr;
use strum::Display;
use strum::EnumString;
use strum::IntoStaticStr;
use strum::VariantNames;

/// Represents a debug tool that can be included in a toolset
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Display,
    EnumString,
    VariantNames,
    AsRefStr,
    IntoStaticStr
)]
pub enum DebugTool {
    #[strum(serialize = "debug_threads_command")]
    Threads,
    #[strum(serialize = "debug_stack_trace_command")]
    StackTrace,
    #[strum(serialize = "debug_scopes_command")]
    Scopes,
    #[strum(serialize = "debug_variables_command")]
    Variables,
    #[strum(serialize = "debug_set_breakpoints_command")]
    SetBreakpoints,
    #[strum(serialize = "debug_set_exception_breakpoints_command")]
    SetExceptionBreakpoints,
    #[strum(serialize = "debug_set_variable_command")]
    SetVariable,
    #[strum(serialize = "debug_navigate_command")]
    Navigate,
    #[strum(serialize = "debug_evaluate_command")]
    Evaluate,
    #[strum(serialize = "debug_stop_command")]
    Stop,
    #[strum(serialize = "debug_read_memory_command")]
    ReadMemory,
    #[strum(serialize = "debug_write_memory_command")]
    WriteMemory,
    #[strum(serialize = "debug_dap_request")]
    DapRequest,
    #[strum(serialize = "debug_status_command")]
    Status,
    #[strum(serialize = "debug_capabilities_command")]
    Capabilities,
    #[strum(serialize = "debug_sessions_command")]
    Sessions,
    #[strum(serialize = "debug_config_command")]
    Config,
    #[strum(serialize = "debug_thread_snapshot")]
    ThreadSnapshot,
}

impl From<DebugTool> for std::borrow::Cow<'static, str> {
    fn from(tool: DebugTool) -> Self {
        std::borrow::Cow::Borrowed(tool.into())
    }
}

/// A toolset defining which debugging tools are available
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolset {
    pub name: String,
    pub tools: Vec<DebugTool>,
}

/// Builtin, predefined toolsets
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Display, ValueEnum)]
#[clap(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum BuiltinToolset {
    Minimal,
    #[default]
    Standard,
    Full,
    Raw,
}

impl BuiltinToolset {
    pub fn tools(&self) -> Vec<DebugTool> {
        match self {
            Self::Minimal => vec![
                DebugTool::Status,
                DebugTool::Threads,
                DebugTool::StackTrace,
                DebugTool::Scopes,
                DebugTool::Variables,
                DebugTool::Capabilities,
            ],
            Self::Standard => vec![
                DebugTool::Status,
                DebugTool::Threads,
                DebugTool::StackTrace,
                DebugTool::Scopes,
                DebugTool::Variables,
                DebugTool::Navigate,
                DebugTool::SetBreakpoints,
                DebugTool::SetExceptionBreakpoints,
                DebugTool::Stop,
                DebugTool::Capabilities,
            ],
            Self::Full => vec![
                DebugTool::Status,
                DebugTool::Threads,
                DebugTool::StackTrace,
                DebugTool::Scopes,
                DebugTool::Variables,
                DebugTool::Navigate,
                DebugTool::SetBreakpoints,
                DebugTool::SetExceptionBreakpoints,
                DebugTool::Evaluate,
                DebugTool::SetVariable,
                DebugTool::ReadMemory,
                DebugTool::WriteMemory,
                DebugTool::Stop,
                DebugTool::Capabilities,
                DebugTool::ThreadSnapshot,
            ],
            Self::Raw => vec![DebugTool::DapRequest, DebugTool::Stop],
        }
    }
}

impl From<BuiltinToolset> for Toolset {
    fn from(builtin: BuiltinToolset) -> Self {
        Self {
            name: builtin.to_string(),
            tools: builtin.tools(),
        }
    }
}

impl Toolset {
    /// Create a custom toolset with the given name and tools
    pub fn custom(name: String, tools: Vec<DebugTool>) -> Self {
        Self { name, tools }
    }

    /// Check if this toolset contains a specific tool
    pub fn contains_tool(&self, tool_name: &str) -> bool {
        self.tools.iter().any(|t| t.as_ref() == tool_name)
    }

    /// Convert the toolset's tools to a Vec<String>
    pub fn to_tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_tool() {
        let toolset = Toolset::custom(
            "test".to_string(),
            vec![
                DebugTool::Threads,
                DebugTool::StackTrace,
                DebugTool::Variables,
            ],
        );

        assert!(toolset.contains_tool("debug_threads_command"));
        assert!(toolset.contains_tool("debug_stack_trace_command"));
        assert!(toolset.contains_tool("debug_variables_command"));

        assert!(!toolset.contains_tool("debug_evaluate_command"));
        assert!(!toolset.contains_tool("nonexistent"));
    }

    #[test]
    fn test_to_tool_names() {
        let minimal: Toolset = BuiltinToolset::Minimal.into();
        let minimal_tools = minimal.to_tool_names();
        assert_eq!(minimal.name, "minimal");
        assert_eq!(minimal_tools.len(), 6);
        assert!(minimal_tools.contains(&"debug_status_command".to_string()));
        assert!(minimal_tools.contains(&"debug_threads_command".to_string()));
        assert!(minimal_tools.contains(&"debug_variables_command".to_string()));
        assert!(minimal_tools.contains(&"debug_capabilities_command".to_string()));

        let standard: Toolset = BuiltinToolset::Standard.into();
        let standard_tools = standard.to_tool_names();
        assert_eq!(standard_tools.len(), 10);
        assert!(standard_tools.contains(&"debug_navigate_command".to_string()));
        assert!(standard_tools.contains(&"debug_set_exception_breakpoints_command".to_string()));

        let custom = Toolset::custom(
            "test".to_string(),
            vec![DebugTool::Threads, DebugTool::Navigate],
        );
        let tool_names = custom.to_tool_names();
        assert_eq!(tool_names.len(), 2);
        assert_eq!(tool_names[0], "debug_threads_command");
        assert_eq!(tool_names[1], "debug_navigate_command");

        let raw: Toolset = BuiltinToolset::Raw.into();
        let raw_tools = raw.to_tool_names();
        assert_eq!(raw.name, "raw");
        assert_eq!(raw_tools.len(), 2);
        assert!(raw_tools.contains(&"debug_dap_request".to_string()));
        assert!(raw_tools.contains(&"debug_stop_command".to_string()));
    }
}
