// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use enumset::EnumSet;
use enumset::EnumSetType;
use enumset::enum_set;
use enumset::enum_set_complement;
use serde::Deserialize;
use serde::Serialize;
use strum::Display;
use strum::EnumString;
use strum::IntoStaticStr;

/// Something a control-plane call does to the debug session or reads from it.
/// Its name appears in `--allow` lists, session configs and error messages,
/// so a released name never changes.
#[derive(
    Debug,
    Hash,
    EnumSetType,
    Display,
    EnumString,
    IntoStaticStr,
    Serialize,
    Deserialize
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// The program's structure: threads, stack frames, scope names, modules
    /// and sources.
    Inspect,
    /// The program's data: variable values, exception details and what the
    /// program prints.
    ReadValues,
    /// Resume, step, pause or rewind execution.
    Navigate,
    /// Set source, function, instruction, data and exception breakpoints.
    Breakpoints,
    /// Evaluate expressions and debugger commands.
    Evaluate,
    /// Change the value of a variable or an expression.
    SetVariable,
    /// Read the debuggee's memory, disassembly included, since it shows the
    /// bytes at any address.
    ReadMemory,
    /// Write the debuggee's memory.
    WriteMemory,
    /// End the debug session or some of its threads.
    Stop,
    /// A request whose effect dapper does not know: the session's lifecycle
    /// (initialize, configurationDone, launch, attach, restart, cancel), the
    /// reverse requests an adapter sends its client (runInTerminal,
    /// startDebugging) and commands outside the DAP specification.
    Unclassified,
}

impl Operation {
    /// What a policy must also allow to allow this operation: what using it
    /// reveals or relies on. Evaluating, setting a variable and reading memory
    /// reveal values, and a request dapper cannot classify could do anything,
    /// so it needs every other operation. Requirements stop at the request:
    /// an expression or debugger command (lldb's `memory write`, any code in
    /// debugpy), a variable's new value or a breakpoint's condition, which the
    /// adapter may evaluate, and a write over the program's code can each do
    /// what other operations do.
    pub const fn requires(self) -> EnumSet<Operation> {
        match self {
            Operation::Evaluate | Operation::SetVariable | Operation::ReadMemory => {
                enum_set!(Operation::ReadValues)
            }
            Operation::Unclassified => enum_set_complement!(Operation::Unclassified),
            Operation::Inspect
            | Operation::ReadValues
            | Operation::Navigate
            | Operation::Breakpoints
            | Operation::WriteMemory
            | Operation::Stop => EnumSet::empty(),
        }
    }
}

/// `operations` as a comma-separated list of names, or "nothing", for messages.
pub fn operation_names(operations: &EnumSet<Operation>) -> String {
    if operations.is_empty() {
        return "nothing".to_owned();
    }
    operations
        .iter()
        .map(<&'static str>::from)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_names_never_change() {
        let names: Vec<&str> = EnumSet::<Operation>::all()
            .iter()
            .map(<&'static str>::from)
            .collect();
        assert_eq!(
            names,
            [
                "inspect",
                "read_values",
                "navigate",
                "breakpoints",
                "evaluate",
                "set_variable",
                "read_memory",
                "write_memory",
                "stop",
                "unclassified",
            ],
            "released names are part of dapper's interface"
        );
    }

    #[test]
    fn serde_and_display_agree_on_names() {
        for operation in EnumSet::<Operation>::all() {
            assert_eq!(
                serde_json::to_value(operation).expect("an operation should serialize"),
                serde_json::json!(operation.to_string()),
            );
            assert_eq!(operation.to_string().parse(), Ok(operation));
        }
    }

    #[test]
    fn unclassified_requests_need_every_other_operation() {
        assert_eq!(
            Operation::Unclassified.requires(),
            EnumSet::all() - Operation::Unclassified
        );
    }

    #[test]
    fn requirements_stop_at_what_an_operation_reveals() {
        for (operation, requires, why) in [
            (
                Operation::Evaluate,
                EnumSet::only(Operation::ReadValues),
                "evaluate allows evaluate requests, not what their code could go on to do",
            ),
            (
                Operation::SetVariable,
                EnumSet::only(Operation::ReadValues),
                "set_variable allows setVariable requests, though debugpy evaluates the new value",
            ),
            (
                Operation::WriteMemory,
                EnumSet::empty(),
                "write_memory allows writeMemory requests, not what the written code could do",
            ),
        ] {
            assert_eq!(operation.requires(), requires, "{why}");
        }
    }

    #[test]
    fn what_an_operation_requires_requires_nothing_more() {
        for operation in EnumSet::<Operation>::all() {
            let with_requirements = operation | operation.requires();
            for required in with_requirements {
                assert!(
                    with_requirements.is_superset(required.requires()),
                    "{required}, which {operation} requires, should require nothing more"
                );
            }
        }
    }

    #[test]
    fn no_operation_requires_itself() {
        for operation in EnumSet::<Operation>::all() {
            assert!(
                !operation.requires().contains(operation),
                "{operation} should not require itself"
            );
        }
    }

    #[test]
    fn lists_name_operations_in_declaration_order() {
        assert_eq!(
            operation_names(&(Operation::Stop | Operation::Inspect)),
            "inspect, stop"
        );
        assert_eq!(operation_names(&EnumSet::empty()), "nothing");
    }
}
