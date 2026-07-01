// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_control_proto::NavigationType as ProtoNavigationType;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::requests::ContinueArguments;
use dapper_dap_protocol::requests::NextArguments;
use dapper_dap_protocol::requests::PauseArguments;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::requests::ReverseContinueArguments;
use dapper_dap_protocol::requests::StepBackArguments;
use dapper_dap_protocol::requests::StepInArguments;
use dapper_dap_protocol::requests::StepOutArguments;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    strum::Display
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum NavigationType {
    /// Step into functions
    StepIn,
    /// Step over functions
    StepOver,
    /// Step out of current frame
    StepOut,
    /// Continue execution until breakpoint or exit
    Continue,
    /// Pause a running program
    Pause,
    /// Step back one source line (reverse stepping). Requires the adapter to
    /// advertise `Capabilities.supportsStepBack`.
    StepBack,
    /// Resume reverse execution until a breakpoint or the start of recording.
    /// Requires the adapter to advertise `Capabilities.supportsStepBack`.
    ReverseContinue,
}

impl From<ProtoNavigationType> for NavigationType {
    fn from(proto_navigation_type: ProtoNavigationType) -> Self {
        match proto_navigation_type {
            ProtoNavigationType::StepIn => NavigationType::StepIn,
            ProtoNavigationType::StepOver => NavigationType::StepOver,
            ProtoNavigationType::StepOut => NavigationType::StepOut,
            ProtoNavigationType::Continue => NavigationType::Continue,
            ProtoNavigationType::Pause => NavigationType::Pause,
            ProtoNavigationType::StepBack => NavigationType::StepBack,
            ProtoNavigationType::ReverseContinue => NavigationType::ReverseContinue,
        }
    }
}

impl From<NavigationType> for ProtoNavigationType {
    fn from(navigation_type: NavigationType) -> Self {
        match navigation_type {
            NavigationType::StepIn => ProtoNavigationType::StepIn,
            NavigationType::StepOver => ProtoNavigationType::StepOver,
            NavigationType::StepOut => ProtoNavigationType::StepOut,
            NavigationType::Continue => ProtoNavigationType::Continue,
            NavigationType::Pause => ProtoNavigationType::Pause,
            NavigationType::StepBack => ProtoNavigationType::StepBack,
            NavigationType::ReverseContinue => ProtoNavigationType::ReverseContinue,
        }
    }
}

impl NavigationType {
    /// Returns the DAP command name for this navigation type
    pub fn command_name(&self) -> &'static str {
        match self {
            NavigationType::StepIn => "stepIn",
            NavigationType::StepOver => "next",
            NavigationType::StepOut => "stepOut",
            NavigationType::Continue => "continue",
            NavigationType::Pause => "pause",
            NavigationType::StepBack => "stepBack",
            NavigationType::ReverseContinue => "reverseContinue",
        }
    }

    /// Returns the success message for this navigation type
    pub fn command_success_description(&self) -> &'static str {
        match self {
            NavigationType::StepIn => "Step in command executed successfully",
            NavigationType::StepOver => "Next command executed successfully",
            NavigationType::StepOut => "Step out command executed successfully",
            NavigationType::Continue => "Continue command executed successfully",
            NavigationType::Pause => "Pause command executed successfully",
            NavigationType::StepBack => "Step back command executed successfully",
            NavigationType::ReverseContinue => "Reverse continue command executed successfully",
        }
    }

    pub fn to_request_command(
        &self,
        thread_id: ThreadId,
        single_thread: Option<bool>,
    ) -> RequestCommand {
        match self {
            NavigationType::Continue => RequestCommand::Continue(ContinueArguments {
                thread_id,
                single_thread,
                ..Default::default()
            }),
            NavigationType::Pause => RequestCommand::Pause(PauseArguments {
                thread_id,
                ..Default::default()
            }),
            NavigationType::StepIn => RequestCommand::StepIn(StepInArguments {
                thread_id,
                single_thread,
                ..Default::default()
            }),
            NavigationType::StepOver => RequestCommand::Next(NextArguments {
                thread_id,
                single_thread,
                ..Default::default()
            }),
            NavigationType::StepOut => RequestCommand::StepOut(StepOutArguments {
                thread_id,
                single_thread,
                ..Default::default()
            }),
            NavigationType::StepBack => RequestCommand::StepBack(StepBackArguments {
                thread_id,
                single_thread,
                ..Default::default()
            }),
            NavigationType::ReverseContinue => {
                RequestCommand::ReverseContinue(ReverseContinueArguments {
                    thread_id,
                    single_thread,
                    ..Default::default()
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_uses_snake_case() {
        assert_eq!(NavigationType::StepIn.to_string(), "step_in");
        assert_eq!(NavigationType::StepOver.to_string(), "step_over");
        assert_eq!(NavigationType::StepOut.to_string(), "step_out");
        assert_eq!(NavigationType::Continue.to_string(), "continue");
        assert_eq!(NavigationType::Pause.to_string(), "pause");
        assert_eq!(NavigationType::StepBack.to_string(), "step_back");
        assert_eq!(
            NavigationType::ReverseContinue.to_string(),
            "reverse_continue"
        );
    }

    #[test]
    fn proto_round_trip_preserves_variants() {
        for variant in [
            NavigationType::StepIn,
            NavigationType::StepOver,
            NavigationType::StepOut,
            NavigationType::Continue,
            NavigationType::Pause,
            NavigationType::StepBack,
            NavigationType::ReverseContinue,
        ] {
            let proto: ProtoNavigationType = variant.into();
            let back: NavigationType = proto.into();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn to_request_command_maps_reverse_variants() {
        let thread_id = ThreadId(7);
        match NavigationType::StepBack.to_request_command(thread_id, None) {
            RequestCommand::StepBack(args) => assert_eq!(args.thread_id, thread_id),
            other => panic!("expected StepBack, got {:?}", other),
        }
        match NavigationType::ReverseContinue.to_request_command(thread_id, None) {
            RequestCommand::ReverseContinue(args) => assert_eq!(args.thread_id, thread_id),
            other => panic!("expected ReverseContinue, got {:?}", other),
        }
    }

    #[test]
    fn to_request_command_passes_single_thread() {
        let thread_id = ThreadId(1);
        match NavigationType::Continue.to_request_command(thread_id, Some(true)) {
            RequestCommand::Continue(args) => {
                assert_eq!(args.thread_id, thread_id);
                assert_eq!(args.single_thread, Some(true));
            }
            other => panic!("expected Continue, got {:?}", other),
        }
        match NavigationType::StepOver.to_request_command(thread_id, Some(true)) {
            RequestCommand::Next(args) => {
                assert_eq!(args.single_thread, Some(true));
            }
            other => panic!("expected Next, got {:?}", other),
        }
    }

    #[test]
    fn command_name_uses_dap_camel_case() {
        assert_eq!(NavigationType::StepIn.command_name(), "stepIn");
        // StepOver maps to "next" — DAP's name for step-over surprises readers,
        // so lock it in alongside the camelCase variants.
        assert_eq!(NavigationType::StepOver.command_name(), "next");
        assert_eq!(NavigationType::StepOut.command_name(), "stepOut");
        assert_eq!(NavigationType::Continue.command_name(), "continue");
        assert_eq!(NavigationType::Pause.command_name(), "pause");
        assert_eq!(NavigationType::StepBack.command_name(), "stepBack");
        assert_eq!(
            NavigationType::ReverseContinue.command_name(),
            "reverseContinue"
        );
    }
}
