// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::requests::RequestCommandDiscriminants as Kind;
use enumset::EnumSet;
use enumset::enum_set;

use crate::Operation;

/// The operations sending `command` to the debug adapter needs, leaving out
/// what they require, which a [`Policy`](crate::Policy) allows along with
/// them: the most a command of its kind can need, whatever its arguments. A
/// command the DAP specification names needs what its name implies even
/// when its arguments did not parse and it reached dapper as
/// [`RequestCommand::Unknown`], since the adapter may still accept it.
pub fn operations_needed_by(command: &RequestCommand) -> EnumSet<Operation> {
    match command {
        RequestCommand::Unknown(unknown) => unknown
            .command
            .parse()
            .map_or(EnumSet::only(Operation::Unclassified), most_needed_by),
        typed => most_needed_by(Kind::from(typed)),
    }
}

/// The most a command of `kind` can need, whatever its arguments, leaving out
/// what those operations require.
fn most_needed_by(kind: Kind) -> EnumSet<Operation> {
    match kind {
        Kind::Threads
        | Kind::Scopes
        | Kind::Source
        | Kind::Modules
        | Kind::LoadedSources
        | Kind::Locations
        | Kind::BreakpointLocations
        | Kind::StepInTargets
        | Kind::GotoTargets => enum_set!(Operation::Inspect),
        Kind::StackTrace => enum_set!(Operation::Inspect | Operation::ReadValues),
        Kind::Variables | Kind::ExceptionInfo => enum_set!(Operation::ReadValues),
        Kind::Continue
        | Kind::Next
        | Kind::StepIn
        | Kind::StepOut
        | Kind::StepBack
        | Kind::ReverseContinue
        | Kind::Pause
        | Kind::Goto
        | Kind::RestartFrame => enum_set!(Operation::Navigate),
        Kind::SetBreakpoints
        | Kind::SetFunctionBreakpoints
        | Kind::SetExceptionBreakpoints
        | Kind::SetDataBreakpoints
        | Kind::SetInstructionBreakpoints
        | Kind::DataBreakpointInfo => enum_set!(Operation::Breakpoints),
        // debugpy evaluates the expression before the cursor to complete it.
        Kind::Evaluate | Kind::Completions => enum_set!(Operation::Evaluate),
        Kind::SetVariable | Kind::SetExpression => enum_set!(Operation::SetVariable),
        // Disassembly shows the bytes at whatever address it is given.
        Kind::ReadMemory | Kind::Disassemble => enum_set!(Operation::ReadMemory),
        Kind::WriteMemory => enum_set!(Operation::WriteMemory),
        Kind::Disconnect | Kind::Terminate | Kind::TerminateThreads => {
            enum_set!(Operation::Stop)
        }
        Kind::Cancel
        | Kind::Initialize
        | Kind::ConfigurationDone
        | Kind::Launch
        | Kind::Attach
        | Kind::Restart
        | Kind::RunInTerminal
        | Kind::StartDebugging
        | Kind::Unknown => enum_set!(Operation::Unclassified),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::json;
    use strum::IntoEnumIterator;

    use super::*;

    fn needed(command: &str, arguments: Option<Value>) -> EnumSet<Operation> {
        operations_needed_by(&RequestCommand::from_raw(command, arguments))
    }

    #[test]
    fn every_command_needs_something() {
        for kind in Kind::iter() {
            assert!(
                !most_needed_by(kind).is_empty(),
                "{kind:?} should need at least one operation"
            );
        }
    }

    #[test]
    fn commands_need_what_they_do() {
        for (command, arguments, operations) in [
            ("threads", None, EnumSet::only(Operation::Inspect)),
            (
                "variables",
                Some(json!({"variablesReference": 7})),
                EnumSet::only(Operation::ReadValues),
            ),
            (
                "next",
                Some(json!({"threadId": 1})),
                EnumSet::only(Operation::Navigate),
            ),
            (
                "evaluate",
                Some(json!({"expression": "x"})),
                EnumSet::only(Operation::Evaluate),
            ),
            (
                "setVariable",
                Some(json!({"variablesReference": 7, "name": "x", "value": "1"})),
                EnumSet::only(Operation::SetVariable),
            ),
            (
                "readMemory",
                Some(json!({"memoryReference": "0x10", "count": 4})),
                EnumSet::only(Operation::ReadMemory),
            ),
            (
                "disassemble",
                Some(json!({"memoryReference": "0x7ffd0000", "instructionCount": 16})),
                EnumSet::only(Operation::ReadMemory),
            ),
            (
                "writeMemory",
                Some(json!({"memoryReference": "0x10", "data": "AA=="})),
                EnumSet::only(Operation::WriteMemory),
            ),
            (
                "setDataBreakpoints",
                Some(json!({"breakpoints": [{"dataId": "1000/4", "condition": "x > 1"}]})),
                EnumSet::only(Operation::Breakpoints),
            ),
            (
                "disconnect",
                Some(json!({"terminateDebuggee": true})),
                EnumSet::only(Operation::Stop),
            ),
            (
                "launch",
                Some(json!({"program": "/bin/true"})),
                EnumSet::only(Operation::Unclassified),
            ),
            (
                "cancel",
                Some(json!({"requestId": 3})),
                EnumSet::only(Operation::Unclassified),
            ),
            (
                "pydevdSystemInfo",
                None,
                EnumSet::only(Operation::Unclassified),
            ),
        ] {
            assert_eq!(
                needed(command, arguments),
                operations,
                "`{command}` should need {operations:?}"
            );
        }
    }

    #[test]
    fn a_known_command_that_did_not_parse_needs_what_its_name_implies() {
        for name in ["writeMemory", "WriteMemory", "WRITEMEMORY"] {
            let command = RequestCommand::from_raw(name, Some(json!({"memoryReference": 7})));
            assert!(
                matches!(command, RequestCommand::Unknown(_)),
                "the malformed `{name}` should reach dapper unparsed"
            );
            assert_eq!(
                operations_needed_by(&command),
                EnumSet::only(Operation::WriteMemory),
                "`{name}` should still need write_memory"
            );
        }
        assert_eq!(
            needed("setBreakpoints", Some(json!({"breakpoints": "all"}))),
            EnumSet::only(Operation::Breakpoints),
            "an unparsed `setBreakpoints` should still need breakpoints"
        );
    }
}
