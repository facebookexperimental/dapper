// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use clap::ValueEnum;
use dapper_session::Expansion;
use enumset::EnumSet;
use enumset::EnumSetType;
use enumset::enum_set;
use strum::AsRefStr;
use strum::Display;
use strum::EnumString;
use strum::IntoStaticStr;
use strum::VariantNames;

/// Represents a debug tool that can be included in a toolset.
///
/// Declared in the order the help topics list tools, since an `EnumSet`
/// iterates in declaration order.
#[derive(
    Debug,
    EnumSetType,
    Display,
    EnumString,
    VariantNames,
    AsRefStr,
    IntoStaticStr
)]
pub enum DebugTool {
    #[strum(serialize = "debug_status_command")]
    Status,
    #[strum(serialize = "debug_threads_command")]
    Threads,
    #[strum(serialize = "debug_stack_trace_command")]
    StackTrace,
    #[strum(serialize = "debug_scopes_command")]
    Scopes,
    #[strum(serialize = "debug_variables_command")]
    Variables,
    #[strum(serialize = "debug_navigate_command")]
    Navigate,
    #[strum(serialize = "debug_set_breakpoints_command")]
    SetBreakpoints,
    #[strum(serialize = "debug_set_exception_breakpoints_command")]
    SetExceptionBreakpoints,
    #[strum(serialize = "debug_evaluate_command")]
    Evaluate,
    #[strum(serialize = "debug_set_variable_command")]
    SetVariable,
    #[strum(serialize = "debug_read_memory_command")]
    ReadMemory,
    #[strum(serialize = "debug_write_memory_command")]
    WriteMemory,
    #[strum(serialize = "debug_dap_request")]
    DapRequest,
    #[strum(serialize = "debug_stop_command")]
    Stop,
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

/// Tools the MCP server exposes whatever the toolset, custom ones included.
pub const ALWAYS_AVAILABLE: EnumSet<DebugTool> = enum_set!(
    DebugTool::Status | DebugTool::Capabilities | DebugTool::Sessions | DebugTool::Config
);

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
    pub const fn tools(self) -> EnumSet<DebugTool> {
        match self {
            Self::Minimal => enum_set!(
                DebugTool::Status
                    | DebugTool::Threads
                    | DebugTool::StackTrace
                    | DebugTool::Scopes
                    | DebugTool::Variables
                    | DebugTool::Capabilities
            ),
            Self::Standard => enum_set!(
                DebugTool::Status
                    | DebugTool::Threads
                    | DebugTool::StackTrace
                    | DebugTool::Scopes
                    | DebugTool::Variables
                    | DebugTool::Navigate
                    | DebugTool::SetBreakpoints
                    | DebugTool::SetExceptionBreakpoints
                    | DebugTool::Stop
                    | DebugTool::Capabilities
            ),
            Self::Full => enum_set!(
                DebugTool::Status
                    | DebugTool::Threads
                    | DebugTool::StackTrace
                    | DebugTool::Scopes
                    | DebugTool::Variables
                    | DebugTool::Navigate
                    | DebugTool::SetBreakpoints
                    | DebugTool::SetExceptionBreakpoints
                    | DebugTool::Evaluate
                    | DebugTool::SetVariable
                    | DebugTool::ReadMemory
                    | DebugTool::WriteMemory
                    | DebugTool::Stop
                    | DebugTool::Capabilities
                    | DebugTool::ThreadSnapshot
            ),
            Self::Raw => enum_set!(DebugTool::DapRequest | DebugTool::Stop),
        }
    }
}

/// The tools an MCP server exposes on top of [`ALWAYS_AVAILABLE`].
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    derive_more::From
)]
pub enum Toolset {
    #[display("{_0}")]
    Builtin(BuiltinToolset),
    #[display("custom")]
    Custom(EnumSet<DebugTool>),
}

impl Toolset {
    pub fn tools(self) -> EnumSet<DebugTool> {
        match self {
            Self::Builtin(builtin) => builtin.tools(),
            Self::Custom(tools) => tools,
        }
    }

    /// The auto-expansion to ask the proxy for: a nested result is inlined
    /// only when this toolset has the tool that fetches it on its own.
    pub fn expansion(self) -> Expansion {
        let tools = self.tools();
        Expansion {
            stack_trace: tools.contains(DebugTool::StackTrace),
            scopes: tools.contains(DebugTool::Scopes),
            locals: tools.contains(DebugTool::Variables),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_toolsets_build_on_each_other() {
        assert_eq!(
            BuiltinToolset::Minimal.tools()
                | DebugTool::Navigate
                | DebugTool::SetBreakpoints
                | DebugTool::SetExceptionBreakpoints
                | DebugTool::Stop,
            BuiltinToolset::Standard.tools(),
            "standard is minimal plus navigation, breakpoints and stop"
        );
        assert_eq!(
            BuiltinToolset::Standard.tools()
                | DebugTool::Evaluate
                | DebugTool::SetVariable
                | DebugTool::ReadMemory
                | DebugTool::WriteMemory
                | DebugTool::ThreadSnapshot,
            BuiltinToolset::Full.tools(),
            "only `full` has the tools that run code in or write to the debuggee, plus memory reads and thread snapshots"
        );
        assert_eq!(
            BuiltinToolset::Raw.tools(),
            DebugTool::DapRequest | DebugTool::Stop
        );
    }

    #[test]
    fn toolsets_list_tools_in_help_order() {
        assert_eq!(
            BuiltinToolset::Standard.tools().iter().collect::<Vec<_>>(),
            [
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
            ]
        );
        assert_eq!(
            ALWAYS_AVAILABLE.iter().collect::<Vec<_>>(),
            [
                DebugTool::Status,
                DebugTool::Capabilities,
                DebugTool::Sessions,
                DebugTool::Config,
            ]
        );
    }

    #[test]
    fn toolsets_display_their_name() {
        assert_eq!(
            Toolset::from(BuiltinToolset::Minimal).to_string(),
            "minimal"
        );
        assert_eq!(Toolset::Custom(EnumSet::empty()).to_string(), "custom");
    }

    #[test]
    fn expansion_follows_the_toolset_tools() {
        for builtin in [
            BuiltinToolset::Minimal,
            BuiltinToolset::Standard,
            BuiltinToolset::Full,
        ] {
            assert_eq!(
                Toolset::from(builtin).expansion(),
                Expansion::ALL,
                "{builtin} has every inspection tool, so it keeps every expansion"
            );
        }

        assert_eq!(
            Toolset::Custom(enum_set!(DebugTool::Threads | DebugTool::StackTrace)).expansion(),
            Expansion {
                stack_trace: true,
                scopes: false,
                locals: false,
            }
        );
        assert_eq!(
            Toolset::Custom(enum_set!(DebugTool::Threads | DebugTool::Scopes)).expansion(),
            Expansion {
                stack_trace: false,
                scopes: true,
                locals: false,
            }
        );
    }
}
