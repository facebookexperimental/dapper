// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::HashSet;

use serde_json::json;

use crate::capabilities::Capabilities;
use crate::data_types::BreakpointId;
use crate::data_types::FrameId;
use crate::data_types::Seq;
use crate::data_types::Source;
use crate::data_types::ThreadId;
use crate::data_types::VariablesReference;
use crate::data_types::i64_from_value;
use crate::enums::*;
use crate::events::*;
use crate::protocol::Message;
use crate::protocol::MessageType;
use crate::protocol::ProtocolError;
use crate::protocol::Request;
use crate::protocol::Response;
use crate::requests::*;
use crate::responses::*;

fn parse_message(json: serde_json::Value) -> Message {
    serde_json::from_value(json).expect("Failed to parse message")
}

#[test]
fn test_parse_initialize_request() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {
            "clientID": "vscode",
            "clientName": "Visual Studio Code",
            "adapterID": "lldb",
            "pathFormat": "path",
            "linesStartAt1": true,
            "columnsStartAt1": true
        }
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request, got {:?}", msg);
    };
    assert_eq!(req.seq, Seq(1));
    assert_eq!(req.command_name(), "initialize");
    let RequestCommand::Initialize(args) = &req.command else {
        panic!("Expected Initialize, got {:?}", req.command);
    };
    assert_eq!(args.client_id.as_deref(), Some("vscode"));
    assert_eq!(args.adapter_id, "lldb");
}

#[test]
fn test_parse_launch_request() {
    let msg = parse_message(json!({
        "seq": 2,
        "type": "request",
        "command": "launch",
        "arguments": {
            "program": "/path/to/program",
            "stopOnEntry": true
        }
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    assert_eq!(req.command_name(), "launch");
    let RequestCommand::Launch(args) = &req.command else {
        panic!("Expected Launch");
    };
    assert_eq!(
        args.extra.get("program").and_then(|v| v.as_str()),
        Some("/path/to/program")
    );
}

#[test]
fn test_parse_threads_request() {
    let msg = parse_message(json!({
        "seq": 5,
        "type": "request",
        "command": "threads"
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    assert_eq!(req.command_name(), "threads");
    assert!(matches!(req.command, RequestCommand::Threads));
}

#[test]
fn test_parse_unknown_request() {
    let msg = parse_message(json!({
        "seq": 99,
        "type": "request",
        "command": "customRequest",
        "arguments": { "key": "value" }
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    assert_eq!(req.command_name(), "customRequest");
}

#[test]
fn test_parse_unknown_message() {
    let msg = parse_message(json!({
        "some_field": "some_value"
    }));

    let MessageType::Other(ref type_) = msg.message_type() else {
        panic!("Expected Other Message Type");
    };
    assert_eq!(type_, "custom");

    let Message::Custom(body) = msg else {
        panic!("Expected Custom Message");
    };
    assert_eq!(
        body.extra.get("some_field").and_then(|v| v.as_str()),
        Some("some_value")
    );
}

#[test]
fn test_parse_initialize_response() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "response",
        "request_seq": 1,
        "success": true,
        "command": "initialize",
        "body": {
            "supportsSingleThreadExecutionRequests": true,
            "supportsConfigurationDoneRequest": true
        }
    }));

    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    assert!(resp.success);
    assert_eq!(resp.command_name(), "initialize");
    let ResponseBody::Initialize(Some(caps)) = &resp.body else {
        panic!("Expected Initialize body");
    };
    assert_eq!(caps.supports_single_thread_execution_requests, Some(true));
}

#[test]
fn test_parse_continue_response() {
    let msg = parse_message(json!({
        "seq": 10,
        "type": "response",
        "request_seq": 5,
        "success": true,
        "command": "continue",
        "body": {
            "allThreadsContinued": true
        }
    }));

    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    let ResponseBody::Continue(body) = &resp.body else {
        panic!("Expected Continue body");
    };
    assert_eq!(body.all_threads_continued, Some(true));
}

#[test]
fn test_parse_error_response() {
    let msg = parse_message(json!({
        "seq": 10,
        "type": "response",
        "request_seq": 5,
        "success": false,
        "command": "continue",
        "message": "Thread not found"
    }));

    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    assert!(!resp.success);
    assert_eq!(resp.message.as_deref(), Some("Thread not found"));
    assert!(resp.check_success().is_err());
}

#[test]
fn test_parse_stopped_event() {
    let msg = parse_message(json!({
        "seq": 50,
        "type": "event",
        "event": "stopped",
        "body": {
            "reason": "breakpoint",
            "threadId": 1,
            "allThreadsStopped": true
        }
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert_eq!(event.event_name(), "stopped");
    let EventKind::Stopped(body) = &event.event else {
        panic!("Expected Stopped event");
    };
    assert_eq!(body.thread_id, Some(ThreadId(1)));
    assert_eq!(body.all_threads_stopped, Some(true));
}

#[test]
fn test_parse_initialized_event_no_body() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "event",
        "event": "initialized"
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert!(matches!(&event.event, EventKind::Initialized(_)));
}

#[test]
fn test_parse_initialized_event_with_empty_body() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "event",
        "event": "initialized",
        "body": {}
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert!(matches!(&event.event, EventKind::Initialized(_)));
}

#[test]
fn test_parse_exited_event() {
    let msg = parse_message(json!({
        "seq": 60,
        "type": "event",
        "event": "exited",
        "body": { "exitCode": 0 }
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    let EventKind::Exited(body) = &event.event else {
        panic!("Expected Exited event");
    };
    assert_eq!(body.exit_code, 0);
}

#[test]
fn test_parse_terminated_event_no_body() {
    let msg = parse_message(json!({
        "seq": 70,
        "type": "event",
        "event": "terminated"
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert!(matches!(&event.event, EventKind::Terminated(_)));
}

#[test]
fn test_parse_custom_event() {
    let msg = parse_message(json!({
        "seq": 80,
        "type": "event",
        "event": "dapper",
        "body": { "status": "ready" }
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert_eq!(event.event_name(), "dapper");
    let EventKind::Unknown(u) = &event.event else {
        panic!("Expected Unknown event");
    };
    assert_eq!(u.event, "dapper");
}

#[test]
fn test_roundtrip_request() {
    let original = Message::Request(Request {
        seq: 1.into(),
        command: RequestCommand::Threads,
    });

    let json = serde_json::to_string(&original).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(original, parsed);
}

#[test]
fn test_roundtrip_response() {
    let original = Message::Response(Response {
        seq: 1.into(),
        request_seq: 1.into(),
        success: true,
        message: None,
        body: ResponseBody::Threads(ThreadsResponseBody {
            threads: vec![crate::data_types::Thread {
                id: 1.into(),
                name: "main".to_owned(),
            }],
            ..Default::default()
        }),
    });

    let json = serde_json::to_string(&original).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(original, parsed);
}

#[test]
fn test_parse_request_without_seq() {
    let msg = parse_message(json!({
        "type": "request",
        "command": "threads"
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    assert_eq!(req.seq, Seq(0));
    assert!(matches!(req.command, RequestCommand::Threads));
}

#[test]
fn test_parse_response_without_seq() {
    let msg = parse_message(json!({
        "type": "response",
        "request_seq": 1,
        "success": true,
        "command": "threads",
        "body": { "threads": [] }
    }));

    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    assert_eq!(resp.seq, Seq(0));
    assert_eq!(resp.request_seq, Seq(1));
}

#[test]
fn test_parse_event_without_seq() {
    let msg = parse_message(json!({
        "type": "event",
        "event": "stopped",
        "body": { "reason": "breakpoint", "threadId": 1 }
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert_eq!(event.seq, Seq(0));
    assert!(matches!(&event.event, EventKind::Stopped(_)));
}

#[test]
fn test_parse_request_with_extra_top_level_fields() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "request",
        "command": "threads",
        "__source": "vscode",
        "metadata": { "timestamp": 12345 }
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    assert_eq!(req.seq, Seq(1));
    assert!(matches!(req.command, RequestCommand::Threads));
}

#[test]
fn test_parse_response_with_extra_top_level_fields() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "response",
        "request_seq": 1,
        "success": true,
        "command": "continue",
        "body": { "allThreadsContinued": true },
        "__timing": { "elapsed_ms": 42 }
    }));

    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    assert!(resp.success);
    let ResponseBody::Continue(body) = &resp.body else {
        panic!("Expected Continue body");
    };
    assert_eq!(body.all_threads_continued, Some(true));
}

#[test]
fn test_parse_event_with_extra_top_level_fields() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "event",
        "event": "stopped",
        "body": { "reason": "breakpoint", "threadId": 5 },
        "__source": "lldb-dap",
        "extra_info": "should be preserved"
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    let EventKind::Stopped(body) = &event.event else {
        panic!("Expected Stopped event");
    };
    assert_eq!(body.thread_id, Some(ThreadId(5)));
}

#[test]
fn test_roundtrip_request_with_arguments() {
    let original = Message::Request(Request::new(RequestCommand::SetBreakpoints(
        SetBreakpointsArguments {
            source: Source {
                path: Some("/tmp/test.rs".to_owned()),
                ..Default::default()
            },
            breakpoints: Some(vec![crate::data_types::SourceBreakpoint {
                line: 42,
                ..Default::default()
            }]),
            ..Default::default()
        },
    )));

    let json = serde_json::to_string(&original).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(original, parsed);
}

#[test]
fn test_stopped_event_with_custom_reason() {
    let msg = parse_message(json!({
        "seq": 50,
        "type": "event",
        "event": "stopped",
        "body": {
            "reason": "custom_reason",
            "threadId": 1
        }
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    let EventKind::Stopped(body) = &event.event else {
        panic!("Expected Stopped event");
    };
    assert_eq!(
        body.reason,
        StoppedReason::Other("custom_reason".to_owned())
    );
    assert_eq!(body.thread_id, Some(ThreadId(1)));

    let json = serde_json::to_string(&event).unwrap();
    let roundtrip: crate::protocol::Event = serde_json::from_str(&json).unwrap();
    let EventKind::Stopped(roundtrip_body) = &roundtrip.event else {
        panic!("Expected Stopped event after roundtrip");
    };
    assert_eq!(
        roundtrip_body.reason,
        StoppedReason::Other("custom_reason".to_owned())
    );
}

#[test]
fn test_parse_set_breakpoints_request() {
    let msg = parse_message(json!({
        "seq": 3,
        "type": "request",
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": "/tmp/main.rs" },
            "breakpoints": [
                { "line": 10 },
                { "line": 20, "condition": "x > 5" }
            ]
        }
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    assert_eq!(req.command_name(), "setBreakpoints");
    let RequestCommand::SetBreakpoints(args) = &req.command else {
        panic!("Expected SetBreakpoints");
    };
    assert_eq!(args.source.path.as_deref(), Some("/tmp/main.rs"));
    let breakpoints = args.breakpoints.as_ref().expect("Expected breakpoints");
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].line, 10);
    assert_eq!(breakpoints[1].condition.as_deref(), Some("x > 5"));
}

#[test]
fn test_parse_output_event() {
    let msg = parse_message(json!({
        "seq": 55,
        "type": "event",
        "event": "output",
        "body": {
            "category": "stdout",
            "output": "Hello, world!\n"
        }
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert_eq!(event.event_name(), "output");
    let EventKind::Output(body) = &event.event else {
        panic!("Expected Output event");
    };
    assert_eq!(body.category, Some(OutputCategory::Stdout));
    assert_eq!(body.output, "Hello, world!\n");
}

#[test]
fn test_parse_breakpoint_event() {
    let msg = parse_message(json!({
        "seq": 56,
        "type": "event",
        "event": "breakpoint",
        "body": {
            "reason": "new",
            "breakpoint": {
                "id": 1,
                "verified": true,
                "line": 42,
                "source": { "path": "/tmp/main.rs" }
            }
        }
    }));

    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert_eq!(event.event_name(), "breakpoint");
    let EventKind::Breakpoint(body) = &event.event else {
        panic!("Expected Breakpoint event");
    };
    assert_eq!(body.reason, BreakpointEventReason::New);
    assert_eq!(body.breakpoint.id, Some(1.into()));
    assert!(body.breakpoint.verified);
    assert_eq!(body.breakpoint.line, Some(42));
}

#[test]
fn test_parse_configuration_done_response() {
    let msg = parse_message(json!({
        "seq": 5,
        "type": "response",
        "request_seq": 3,
        "success": true,
        "command": "configurationDone"
    }));

    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    assert!(resp.success);
    assert_eq!(resp.command_name(), "configurationDone");
    assert!(matches!(resp.body, ResponseBody::ConfigurationDone));
}

#[test]
fn test_parse_next_response_no_body() {
    let msg = parse_message(json!({
        "seq": 15,
        "type": "response",
        "request_seq": 10,
        "success": true,
        "command": "next"
    }));

    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    assert!(resp.success);
    assert_eq!(resp.command_name(), "next");
    assert!(matches!(resp.body, ResponseBody::Next));
}

fn check_newtype_roundtrip<T>(original: T, expected_val: i64)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json_val = serde_json::to_value(&original).unwrap();
    assert!(json_val.is_number());
    assert_eq!(json_val, serde_json::Value::Number(expected_val.into()));
    let roundtrip: T = serde_json::from_value(json_val).unwrap();
    assert_eq!(original, roundtrip);
}

#[test]
fn test_newtype_serde_roundtrip_all() {
    check_newtype_roundtrip(Seq(42), 42);
    check_newtype_roundtrip(ThreadId(7), 7);
    check_newtype_roundtrip(FrameId(100), 100);
    check_newtype_roundtrip(VariablesReference(999), 999);
    check_newtype_roundtrip(BreakpointId(3), 3);
}

#[test]
fn test_newtype_edge_values() {
    let edge_values: &[i64] = &[0, -1, i64::MAX, i64::MIN];

    for &v in edge_values {
        check_newtype_roundtrip(Seq(v), v);
        check_newtype_roundtrip(ThreadId(v), v);
        check_newtype_roundtrip(FrameId(v), v);
        check_newtype_roundtrip(VariablesReference(v), v);
        check_newtype_roundtrip(BreakpointId(v), v);
    }
}

#[test]
fn test_newtype_display_formatting() {
    assert_eq!(Seq(42).to_string(), "42");
    assert_eq!(ThreadId(7).to_string(), "7");
    assert_eq!(FrameId(100).to_string(), "100");
    assert_eq!(VariablesReference(999).to_string(), "999");
    assert_eq!(BreakpointId(3).to_string(), "3");
    assert_eq!(Seq(-1).to_string(), "-1");
    assert_eq!(ThreadId(0).to_string(), "0");
}

#[test]
fn test_newtype_from_i64_roundtrip() {
    let val: i64 = 42;

    let seq = Seq::from(val);
    let back: i64 = seq.into();
    assert_eq!(back, val);

    let tid = ThreadId::from(val);
    let back: i64 = tid.into();
    assert_eq!(back, val);

    let fid = FrameId::from(val);
    let back: i64 = fid.into();
    assert_eq!(back, val);

    let vr = VariablesReference::from(val);
    let back: i64 = vr.into();
    assert_eq!(back, val);

    let bid = BreakpointId::from(val);
    let back: i64 = bid.into();
    assert_eq!(back, val);
}

#[test]
fn test_thread_id_from_str() {
    let tid: ThreadId = "42".parse().unwrap();
    assert_eq!(tid, ThreadId(42));

    let tid: ThreadId = "-1".parse().unwrap();
    assert_eq!(tid, ThreadId(-1));

    let tid: ThreadId = "0".parse().unwrap();
    assert_eq!(tid, ThreadId(0));

    assert!("abc".parse::<ThreadId>().is_err());
    assert!("".parse::<ThreadId>().is_err());
    assert!("3.14".parse::<ThreadId>().is_err());
}

#[test]
fn test_frame_id_from_str() {
    let fid: FrameId = "100".parse().unwrap();
    assert_eq!(fid, FrameId(100));

    let fid: FrameId = "-5".parse().unwrap();
    assert_eq!(fid, FrameId(-5));

    assert!("not_a_number".parse::<FrameId>().is_err());
    assert!("".parse::<FrameId>().is_err());
}

#[test]
fn test_variables_reference_from_str() {
    let vr: VariablesReference = "999".parse().unwrap();
    assert_eq!(vr, VariablesReference(999));

    let vr: VariablesReference = "0".parse().unwrap();
    assert_eq!(vr, VariablesReference(0));

    assert!("xyz".parse::<VariablesReference>().is_err());
    assert!("".parse::<VariablesReference>().is_err());
}

#[test]
fn test_newtype_default_values() {
    assert_eq!(Seq::default(), Seq(0));
    assert_eq!(ThreadId::default(), ThreadId(0));
    assert_eq!(FrameId::default(), FrameId(0));
    assert_eq!(VariablesReference::default(), VariablesReference(0));
    assert_eq!(BreakpointId::default(), BreakpointId(0));
}

#[test]
fn test_newtype_hash_in_collections() {
    let mut set = HashSet::new();
    set.insert(ThreadId(1));
    set.insert(ThreadId(2));
    set.insert(ThreadId(1));
    assert_eq!(set.len(), 2);
    assert!(set.contains(&ThreadId(1)));
    assert!(set.contains(&ThreadId(2)));
    assert!(!set.contains(&ThreadId(3)));
}

#[test]
fn test_newtype_in_struct_context() {
    let args: ContinueArguments = serde_json::from_value(json!({
        "threadId": 7
    }))
    .unwrap();
    assert_eq!(args.thread_id, ThreadId(7));
}

fn all_request_commands() -> Vec<RequestCommand> {
    vec![
        RequestCommand::Cancel(Some(CancelArguments::default())),
        RequestCommand::Initialize(InitializeRequestArguments {
            adapter_id: "test".to_owned(),
            ..Default::default()
        }),
        RequestCommand::ConfigurationDone(Some(ConfigurationDoneArguments::default())),
        RequestCommand::Launch(LaunchRequestArguments::default()),
        RequestCommand::Attach(AttachRequestArguments::default()),
        RequestCommand::Restart(Some(RestartArguments::default())),
        RequestCommand::Disconnect(Some(DisconnectArguments::default())),
        RequestCommand::Terminate(Some(TerminateArguments::default())),
        RequestCommand::BreakpointLocations(Some(BreakpointLocationsArguments {
            source: Source::default(),
            line: 1,
            ..Default::default()
        })),
        RequestCommand::SetBreakpoints(SetBreakpointsArguments {
            source: Source::default(),
            ..Default::default()
        }),
        RequestCommand::SetFunctionBreakpoints(SetFunctionBreakpointsArguments::default()),
        RequestCommand::SetExceptionBreakpoints(SetExceptionBreakpointsArguments::default()),
        RequestCommand::DataBreakpointInfo(DataBreakpointInfoArguments {
            name: "x".to_owned(),
            ..Default::default()
        }),
        RequestCommand::SetDataBreakpoints(SetDataBreakpointsArguments::default()),
        RequestCommand::SetInstructionBreakpoints(SetInstructionBreakpointsArguments::default()),
        RequestCommand::Continue(ContinueArguments::default()),
        RequestCommand::Next(NextArguments::default()),
        RequestCommand::StepIn(StepInArguments::default()),
        RequestCommand::StepOut(StepOutArguments::default()),
        RequestCommand::StepBack(StepBackArguments::default()),
        RequestCommand::ReverseContinue(ReverseContinueArguments::default()),
        RequestCommand::RestartFrame(RestartFrameArguments::default()),
        RequestCommand::Goto(GotoArguments::default()),
        RequestCommand::Pause(PauseArguments::default()),
        RequestCommand::StackTrace(StackTraceArguments::default()),
        RequestCommand::Scopes(ScopesArguments::default()),
        RequestCommand::Variables(VariablesArguments::default()),
        RequestCommand::SetVariable(SetVariableArguments {
            variables_reference: 1.into(),
            name: "x".to_owned(),
            value: "42".to_owned(),
            ..Default::default()
        }),
        RequestCommand::Source(SourceArguments::default()),
        RequestCommand::Threads,
        RequestCommand::TerminateThreads(TerminateThreadsArguments::default()),
        RequestCommand::Modules(ModulesArguments::default()),
        RequestCommand::LoadedSources(Some(LoadedSourcesArguments::default())),
        RequestCommand::Evaluate(EvaluateArguments {
            expression: "x + 1".to_owned(),
            ..Default::default()
        }),
        RequestCommand::SetExpression(SetExpressionArguments {
            expression: "x".to_owned(),
            value: "42".to_owned(),
            ..Default::default()
        }),
        RequestCommand::StepInTargets(StepInTargetsArguments::default()),
        RequestCommand::GotoTargets(GotoTargetsArguments {
            source: Source::default(),
            line: 1,
            ..Default::default()
        }),
        RequestCommand::Completions(CompletionsArguments {
            text: "hel".to_owned(),
            column: 3,
            ..Default::default()
        }),
        RequestCommand::ExceptionInfo(ExceptionInfoArguments::default()),
        RequestCommand::ReadMemory(ReadMemoryArguments {
            memory_reference: "0x1000".to_owned(),
            count: 16,
            ..Default::default()
        }),
        RequestCommand::WriteMemory(WriteMemoryArguments {
            memory_reference: "0x1000".to_owned(),
            data: "AQID".to_owned(),
            ..Default::default()
        }),
        RequestCommand::Disassemble(DisassembleArguments {
            memory_reference: "0x1000".to_owned(),
            instruction_count: 10,
            ..Default::default()
        }),
        RequestCommand::Locations(LocationsArguments {
            location_reference: 1,
            ..Default::default()
        }),
        RequestCommand::RunInTerminal(RunInTerminalRequestArguments {
            cwd: "/tmp".to_owned(),
            args: vec!["ls".to_owned()],
            ..Default::default()
        }),
        RequestCommand::StartDebugging(StartDebuggingRequestArguments {
            configuration: Default::default(),
            request: StartDebuggingType::Launch,
            ..Default::default()
        }),
    ]
}

fn all_response_bodies() -> Vec<ResponseBody> {
    vec![
        ResponseBody::Cancel,
        ResponseBody::Initialize(Some(Capabilities::default())),
        ResponseBody::ConfigurationDone,
        ResponseBody::Launch,
        ResponseBody::Attach,
        ResponseBody::Restart,
        ResponseBody::Disconnect,
        ResponseBody::Terminate,
        ResponseBody::BreakpointLocations(BreakpointLocationsResponseBody::default()),
        ResponseBody::SetBreakpoints(SetBreakpointsResponseBody::default()),
        ResponseBody::SetFunctionBreakpoints(SetFunctionBreakpointsResponseBody::default()),
        ResponseBody::SetExceptionBreakpoints(Some(SetExceptionBreakpointsResponseBody::default())),
        ResponseBody::DataBreakpointInfo(DataBreakpointInfoResponseBody {
            description: "test".to_owned(),
            ..Default::default()
        }),
        ResponseBody::SetDataBreakpoints(SetDataBreakpointsResponseBody::default()),
        ResponseBody::SetInstructionBreakpoints(SetInstructionBreakpointsResponseBody::default()),
        ResponseBody::Continue(ContinueResponseBody::default()),
        ResponseBody::Next,
        ResponseBody::StepIn,
        ResponseBody::StepOut,
        ResponseBody::StepBack,
        ResponseBody::ReverseContinue,
        ResponseBody::RestartFrame,
        ResponseBody::Goto,
        ResponseBody::Pause,
        ResponseBody::StackTrace(StackTraceResponseBody::default()),
        ResponseBody::Scopes(ScopesResponseBody::default()),
        ResponseBody::Variables(VariablesResponseBody::default()),
        ResponseBody::SetVariable(SetVariableResponseBody {
            value: "42".to_owned(),
            ..Default::default()
        }),
        ResponseBody::Source(SourceResponseBody {
            content: "fn main() {}".to_owned(),
            ..Default::default()
        }),
        ResponseBody::Threads(ThreadsResponseBody::default()),
        ResponseBody::TerminateThreads,
        ResponseBody::Modules(ModulesResponseBody::default()),
        ResponseBody::LoadedSources(LoadedSourcesResponseBody::default()),
        ResponseBody::Evaluate(EvaluateResponseBody {
            result: "42".to_owned(),
            ..Default::default()
        }),
        ResponseBody::SetExpression(SetExpressionResponseBody {
            value: "42".to_owned(),
            ..Default::default()
        }),
        ResponseBody::StepInTargets(StepInTargetsResponseBody::default()),
        ResponseBody::GotoTargets(GotoTargetsResponseBody::default()),
        ResponseBody::Completions(CompletionsResponseBody::default()),
        ResponseBody::ExceptionInfo(ExceptionInfoResponseBody {
            exception_id: "exc1".to_owned(),
            break_mode: ExceptionBreakMode::Always,
            ..Default::default()
        }),
        ResponseBody::ReadMemory(Some(ReadMemoryResponseBody {
            address: "0x1000".to_owned(),
            ..Default::default()
        })),
        ResponseBody::WriteMemory(Some(WriteMemoryResponseBody::default())),
        ResponseBody::Disassemble(Some(DisassembleResponseBody::default())),
        ResponseBody::Locations(Some(LocationsResponseBody {
            source: Source::default(),
            line: 1,
            ..Default::default()
        })),
        ResponseBody::RunInTerminal(RunInTerminalResponseBody::default()),
        ResponseBody::StartDebugging,
    ]
}

fn all_event_kinds() -> Vec<EventKind> {
    vec![
        EventKind::Initialized(None),
        EventKind::Initialized(Some(InitializedEventBody::default())),
        EventKind::Stopped(StoppedEventBody {
            reason: StoppedReason::Breakpoint,
            ..Default::default()
        }),
        EventKind::Continued(ContinuedEventBody::default()),
        EventKind::Exited(ExitedEventBody {
            exit_code: 0,
            ..Default::default()
        }),
        EventKind::Terminated(None),
        EventKind::Terminated(Some(TerminatedEventBody::default())),
        EventKind::Thread(ThreadEventBody {
            reason: ThreadReason::Started,
            thread_id: ThreadId(1),
            ..Default::default()
        }),
        EventKind::Output(OutputEventBody {
            output: "hello".to_owned(),
            ..Default::default()
        }),
        EventKind::Breakpoint(BreakpointEventBody {
            reason: BreakpointEventReason::New,
            breakpoint: crate::data_types::Breakpoint::default(),
            ..Default::default()
        }),
        EventKind::Module(ModuleEventBody {
            reason: ModuleEventReason::New,
            module: crate::data_types::Module::default(),
            ..Default::default()
        }),
        EventKind::LoadedSource(LoadedSourceEventBody {
            reason: LoadedSourceEventReason::New,
            source: Source::default(),
            ..Default::default()
        }),
        EventKind::Process(ProcessEventBody {
            name: "test".to_owned(),
            ..Default::default()
        }),
        EventKind::Capabilities(CapabilitiesEventBody {
            capabilities: Capabilities::default(),
            ..Default::default()
        }),
        EventKind::ProgressStart(ProgressStartEventBody {
            progress_id: "p1".to_owned(),
            title: "Loading".to_owned(),
            ..Default::default()
        }),
        EventKind::ProgressUpdate(ProgressUpdateEventBody {
            progress_id: "p1".to_owned(),
            ..Default::default()
        }),
        EventKind::ProgressEnd(ProgressEndEventBody {
            progress_id: "p1".to_owned(),
            ..Default::default()
        }),
        EventKind::Invalidated(InvalidatedEventBody::default()),
        EventKind::Memory(MemoryEventBody {
            memory_reference: "0x1000".to_owned(),
            offset: 0,
            count: 16,
            ..Default::default()
        }),
    ]
}

#[test]
fn test_roundtrip_all_request_commands() {
    for cmd in all_request_commands() {
        let original = Message::Request(Request::new(cmd));
        let json_str = serde_json::to_string(&original).unwrap();
        let parsed: Message = serde_json::from_str(&json_str).unwrap();
        assert_eq!(original, parsed, "roundtrip failed for request: {json_str}");
    }
}

#[test]
fn test_roundtrip_all_response_body_variants() {
    for body in all_response_bodies() {
        let original = Message::Response(Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body,
        });
        let json_str = serde_json::to_string(&original).unwrap();
        let parsed: Message = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            original, parsed,
            "roundtrip failed for response: {json_str}"
        );
    }
}

#[test]
fn test_roundtrip_all_event_kind_variants() {
    for event in all_event_kinds() {
        let original = Message::Event(crate::protocol::Event::new(event));
        let json_str = serde_json::to_string(&original).unwrap();
        let parsed: Message = serde_json::from_str(&json_str).unwrap();
        assert_eq!(original, parsed, "roundtrip failed for event: {json_str}");
    }
}

#[test]
fn test_unit_variants_no_body_in_json() {
    let unit_responses = vec![
        ResponseBody::Cancel,
        ResponseBody::ConfigurationDone,
        ResponseBody::Launch,
        ResponseBody::Attach,
        ResponseBody::Restart,
        ResponseBody::Disconnect,
        ResponseBody::Terminate,
        ResponseBody::Next,
        ResponseBody::StepIn,
        ResponseBody::StepOut,
        ResponseBody::StepBack,
        ResponseBody::ReverseContinue,
        ResponseBody::RestartFrame,
        ResponseBody::Goto,
        ResponseBody::Pause,
        ResponseBody::TerminateThreads,
        ResponseBody::StartDebugging,
    ];

    for body in unit_responses {
        let msg = Message::Response(Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body,
        });
        let json_val: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert!(
            json_val.get("body").is_none(),
            "expected no body field for response: {}",
            json_val
        );
    }

    let threads_req = Message::Request(Request::new(RequestCommand::Threads));
    let json_val: serde_json::Value = serde_json::to_value(&threads_req).unwrap();
    assert!(
        json_val.get("arguments").is_none(),
        "expected no arguments for Threads request"
    );
}

#[test]
fn test_command_name_matches_serde_tag() {
    for cmd in all_request_commands() {
        let name = cmd.command_name().to_owned();
        let req = Request::new(cmd);
        let json_val: serde_json::Value = serde_json::to_value(&req).unwrap();
        let json_command = json_val["command"].as_str().unwrap();
        assert_eq!(
            name, json_command,
            "command_name() does not match serialized command field"
        );
    }

    for body in all_response_bodies() {
        let name = body.command_name().to_owned();
        let resp = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body,
        };
        let json_val: serde_json::Value = serde_json::to_value(&resp).unwrap();
        let json_command = json_val["command"].as_str().unwrap();
        assert_eq!(name, json_command);
    }

    for ek in all_event_kinds() {
        let name = ek.event_name().to_owned();
        let event = crate::protocol::Event::new(ek);
        let json_val: serde_json::Value = serde_json::to_value(&event).unwrap();
        let json_event = json_val["event"].as_str().unwrap();
        assert_eq!(name, json_event);
    }
}

#[test]
fn test_unknown_request_roundtrip() {
    let cmd = RequestCommand::Unknown(UnknownCommand {
        command: "myCustomCommand".to_owned(),
        arguments: Some(json!({"key": "value"})),
        extra: Default::default(),
    });

    let original = Message::Request(Request::new(cmd));
    let json_str = serde_json::to_string(&original).unwrap();
    let parsed: Message = serde_json::from_str(&json_str).unwrap();
    assert_eq!(original, parsed);

    let Message::Request(req) = &parsed else {
        panic!("Expected Request");
    };
    assert_eq!(req.command_name(), "myCustomCommand");
}

#[test]
fn test_unknown_response_roundtrip() {
    let body = ResponseBody::Unknown(UnknownResponseBody {
        command: "myCustomResponse".to_owned(),
        body: Some(json!({"result": 42})),
        extra: Default::default(),
    });

    let original = Message::Response(Response {
        seq: 1.into(),
        request_seq: 1.into(),
        success: true,
        message: None,
        body,
    });
    let json_str = serde_json::to_string(&original).unwrap();
    let parsed: Message = serde_json::from_str(&json_str).unwrap();
    assert_eq!(original, parsed);
}

#[test]
fn test_unknown_event_roundtrip() {
    let ek = EventKind::Unknown(UnknownEvent {
        event: "myCustomEvent".to_owned(),
        body: Some(json!({"status": "ok"})),
        extra: Default::default(),
    });

    let original = Message::Event(crate::protocol::Event::new(ek));
    let json_str = serde_json::to_string(&original).unwrap();
    let parsed: Message = serde_json::from_str(&json_str).unwrap();
    assert_eq!(original, parsed);

    let Message::Event(event) = &parsed else {
        panic!("Expected Event");
    };
    assert_eq!(event.event_name(), "myCustomEvent");
}

#[test]
fn test_unknown_deserialized_from_json() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "request",
        "command": "fooBarCustom",
        "arguments": {"x": 1}
    }));
    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    assert_eq!(req.command_name(), "fooBarCustom");
    assert!(matches!(req.command, RequestCommand::Unknown(_)));

    let msg = parse_message(json!({
        "seq": 1,
        "type": "response",
        "request_seq": 1,
        "success": true,
        "command": "fooBarCustom",
        "body": {"data": true}
    }));
    let Message::Response(resp) = msg else {
        panic!("Expected Response");
    };
    assert_eq!(resp.command_name(), "fooBarCustom");
    assert!(matches!(resp.body, ResponseBody::Unknown(_)));

    let msg = parse_message(json!({
        "seq": 1,
        "type": "event",
        "event": "fooBarCustom",
        "body": {"data": true}
    }));
    let Message::Event(event) = msg else {
        panic!("Expected Event");
    };
    assert_eq!(event.event_name(), "fooBarCustom");
    assert!(matches!(event.event, EventKind::Unknown(_)));
}

#[test]
fn test_all_string_enums_other_fallback() {
    let fb: StoppedReason = serde_json::from_value(json!("function breakpoint")).unwrap();
    assert_eq!(fb, StoppedReason::FunctionBreakpoint);
    let serialized = serde_json::to_value(&fb).unwrap();
    assert_eq!(serialized.as_str().unwrap(), "function breakpoint");

    let sha: ChecksumAlgorithm = serde_json::from_value(json!("SHA256")).unwrap();
    assert_eq!(sha, ChecksumAlgorithm::Sha256);
    let serialized = serde_json::to_value(&sha).unwrap();
    assert_eq!(serialized.as_str().unwrap(), "SHA256");

    let md5: ChecksumAlgorithm = serde_json::from_value(json!("MD5")).unwrap();
    assert_eq!(md5, ChecksumAlgorithm::Md5);

    let db: StoppedReason = serde_json::from_value(json!("data breakpoint")).unwrap();
    assert_eq!(db, StoppedReason::DataBreakpoint);

    let ib: StoppedReason = serde_json::from_value(json!("instruction breakpoint")).unwrap();
    assert_eq!(ib, StoppedReason::InstructionBreakpoint);
}

#[test]
fn test_extra_fields_captured_on_request_arguments() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {
            "adapterID": "test",
            "clientID": "vscode",
            "customFeature": true,
            "debugLevel": 3
        }
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    let RequestCommand::Initialize(args) = &req.command else {
        panic!("Expected Initialize");
    };
    assert_eq!(args.adapter_id, "test");
    assert_eq!(args.extra.get("customFeature"), Some(&json!(true)));
    assert_eq!(args.extra.get("debugLevel"), Some(&json!(3)));
}

#[test]
fn test_extra_fields_roundtrip() {
    let mut args = InitializeRequestArguments {
        adapter_id: "test".to_owned(),
        ..Default::default()
    };
    args.extra
        .insert("customSetting".to_owned(), json!("myValue"));
    args.extra.insert("numericExtra".to_owned(), json!(99));

    let original = Message::Request(Request::new(RequestCommand::Initialize(args)));
    let json_str = serde_json::to_string(&original).unwrap();
    let parsed: Message = serde_json::from_str(&json_str).unwrap();
    assert_eq!(original, parsed);

    let Message::Request(req) = &parsed else {
        panic!("Expected Request");
    };
    let RequestCommand::Initialize(parsed_args) = &req.command else {
        panic!("Expected Initialize");
    };
    assert_eq!(
        parsed_args.extra.get("customSetting"),
        Some(&json!("myValue"))
    );
    assert_eq!(parsed_args.extra.get("numericExtra"), Some(&json!(99)));
}

#[test]
fn test_empty_extra_not_serialized() {
    let args = InitializeRequestArguments {
        adapter_id: "test".to_owned(),
        ..Default::default()
    };
    assert!(args.extra.is_empty());

    let req = Request::new(RequestCommand::Initialize(args));
    let json_val: serde_json::Value = serde_json::to_value(&req).unwrap();

    let arguments = json_val.get("arguments").unwrap();
    let obj = arguments.as_object().unwrap();
    assert!(
        !obj.contains_key("extra"),
        "extra field should not appear in serialized output"
    );
    assert_eq!(obj.get("adapterID").unwrap(), "test");
}

#[test]
fn test_launch_arguments_adapter_specific_fields() {
    let msg = parse_message(json!({
        "seq": 1,
        "type": "request",
        "command": "launch",
        "arguments": {
            "program": "/path/to/app",
            "args": ["--verbose"],
            "env": {"HOME": "/tmp"},
            "stopOnEntry": false
        }
    }));

    let Message::Request(req) = msg else {
        panic!("Expected Request");
    };
    let RequestCommand::Launch(args) = &req.command else {
        panic!("Expected Launch");
    };
    assert_eq!(
        args.extra.get("program").and_then(|v| v.as_str()),
        Some("/path/to/app")
    );
    assert_eq!(args.extra.get("args"), Some(&json!(["--verbose"])));
    assert_eq!(args.extra.get("env"), Some(&json!({"HOME": "/tmp"})));
    assert_eq!(args.extra.get("stopOnEntry"), Some(&json!(false)));
    assert_eq!(args.no_debug, None);
}

#[test]
fn test_capabilities_extra_fields() {
    let caps: Capabilities = serde_json::from_value(json!({
        "supportsConfigurationDoneRequest": true,
        "customCapability": "fancy",
        "experimentalFeature": 42
    }))
    .unwrap();

    assert_eq!(caps.supports_configuration_done_request, Some(true));
    assert_eq!(caps.extra.get("customCapability"), Some(&json!("fancy")));
    assert_eq!(caps.extra.get("experimentalFeature"), Some(&json!(42)));

    let serialized = serde_json::to_value(&caps).unwrap();
    assert_eq!(
        serialized
            .get("customCapability")
            .unwrap()
            .as_str()
            .unwrap(),
        "fancy"
    );
}

#[tokio::test]
async fn test_wire_malformed_header() {
    let data = b"Garbage Line Without Colon\r\n\r\n{}";
    let mut cursor = tokio::io::BufReader::new(&data[..]);
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ProtocolError::HeaderParseError(_)
    ));
}

#[tokio::test]
async fn test_wire_non_numeric_length() {
    let data = b"Content-Length: abc\r\n\r\n{}";
    let mut cursor = tokio::io::BufReader::new(&data[..]);
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ProtocolError::HeaderParseError(_)
    ));
}

#[tokio::test]
async fn test_wire_oversized_content_length() {
    // Content-Length exceeds MAX_DAP_MESSAGE_SIZE (10 MB)
    let data = b"Content-Length: 999999999\r\n\r\n{}";
    let mut cursor = tokio::io::BufReader::new(&data[..]);
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        ProtocolError::HeaderParseError(msg) => {
            assert!(
                msg.contains("exceeds maximum allowed size"),
                "unexpected error message: {}",
                msg
            );
        }
        other => panic!("Expected HeaderParseError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_wire_unknown_header_without_content_length() {
    // Unknown headers are ignored, but Content-Length is still required
    let data = b"X-Custom: val\r\n\r\n{}";
    let mut cursor = tokio::io::BufReader::new(&data[..]);
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        ProtocolError::HeaderParseError(msg) => {
            assert!(
                msg.contains("Missing Content-Length"),
                "unexpected error: {}",
                msg
            );
        }
        other => panic!("Expected HeaderParseError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_wire_content_type_header_accepted() {
    // Content-Type is a valid DAP header that should be accepted and ignored
    let json = r#"{"seq":1,"type":"event","event":"initialized","body":{}}"#;
    let header = format!(
        "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n",
        json.len()
    );
    let data = format!("{}{}", header, json);
    let mut cursor = tokio::io::BufReader::new(data.as_bytes());
    let msg = Message::read(&mut cursor)
        .await
        .expect("message with Content-Type header should parse successfully");
    let msg = msg.expect("should return Some(message)");
    assert!(matches!(msg, Message::Event(_)));
}

#[tokio::test]
async fn test_wire_truncated_body() {
    let data = b"Content-Length: 100\r\n\r\n{\"short\": true}";
    let mut cursor = tokio::io::BufReader::new(&data[..]);
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ProtocolError::IoError(_)));
}

#[tokio::test]
async fn test_wire_valid_roundtrip() {
    let original = Message::Request(Request::new(RequestCommand::Threads));
    let wire = original.format().unwrap();
    let mut cursor = tokio::io::BufReader::new(wire.as_slice());
    let parsed = Message::read(&mut cursor).await.unwrap().unwrap();
    assert_eq!(original, parsed);
}

#[tokio::test]
async fn test_wire_multiple_messages_sequential() {
    let msg1 = Message::Request(Request::new(RequestCommand::Threads));
    let msg2 = Message::Event(crate::protocol::Event::new(EventKind::Exited(
        ExitedEventBody {
            exit_code: 0,
            ..Default::default()
        },
    )));

    let mut wire = msg1.format().unwrap();
    wire.extend_from_slice(&msg2.format().unwrap());

    let mut cursor = tokio::io::BufReader::new(wire.as_slice());
    let parsed1 = Message::read(&mut cursor).await.unwrap().unwrap();
    let parsed2 = Message::read(&mut cursor).await.unwrap().unwrap();
    assert_eq!(msg1, parsed1);
    assert_eq!(msg2, parsed2);
}

#[tokio::test]
async fn test_wire_eof_returns_none() {
    let data: &[u8] = b"";
    let mut cursor = tokio::io::BufReader::new(data);
    let result = Message::read(&mut cursor).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_wire_unicode_content_length() {
    let unicode_expr = "\u{1F600}\u{1F4A9}\u{00E9}";
    let original = Message::Request(Request::new(RequestCommand::Evaluate(EvaluateArguments {
        expression: unicode_expr.to_owned(),
        ..Default::default()
    })));

    let wire = original.format().unwrap();
    let wire_str = std::str::from_utf8(&wire).unwrap();

    let header_end = wire_str.find("\r\n\r\n").unwrap();
    let header = &wire_str[..header_end];
    let claimed_length: usize = header
        .strip_prefix("Content-Length: ")
        .unwrap()
        .parse()
        .unwrap();
    let body = &wire_str[header_end + 4..];
    assert_eq!(claimed_length, body.len());

    let mut cursor = tokio::io::BufReader::new(wire.as_slice());
    let parsed = Message::read(&mut cursor).await.unwrap().unwrap();
    assert_eq!(original, parsed);
}

#[tokio::test]
async fn test_wire_header_line_too_long() {
    // A single header line of ~9 KB exceeds MAX_DAP_HEADER_LINE_SIZE (8 KB)
    // and must be rejected before being read into memory.
    let oversized_value = "x".repeat(9 * 1024);
    let data = format!("X-Long: {}\r\n\r\n{{}}", oversized_value);
    let mut cursor = tokio::io::BufReader::new(data.as_bytes());
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        ProtocolError::HeaderParseError(msg) => {
            assert!(
                msg.contains("exceeds maximum length"),
                "unexpected error message: {}",
                msg
            );
        }
        other => panic!("Expected HeaderParseError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_wire_max_headers_accepted() {
    // Exactly MAX_DAP_HEADER_COUNT (32) headers should parse successfully.
    // We use 1 Content-Length + 31 ignored X-Foo lines = 32 headers total.
    let json = "{}";
    let mut header = format!("Content-Length: {}\r\n", json.len());
    for i in 0..31 {
        header.push_str(&format!("X-Foo-{}: bar\r\n", i));
    }
    header.push_str("\r\n");
    let data = format!("{}{}", header, json);
    let mut cursor = tokio::io::BufReader::new(data.as_bytes());
    let result = Message::read(&mut cursor).await;
    assert!(
        result.is_ok(),
        "32 headers should parse successfully but got: {:?}",
        result
    );
    assert!(result.unwrap().is_some(), "should return Some(message)");
}

#[tokio::test]
async fn test_wire_too_many_headers() {
    // 33 headers (one over the cap) must be rejected.
    let json = "{}";
    let mut header = format!("Content-Length: {}\r\n", json.len());
    for i in 0..32 {
        header.push_str(&format!("X-Foo-{}: bar\r\n", i));
    }
    header.push_str("\r\n");
    let data = format!("{}{}", header, json);
    let mut cursor = tokio::io::BufReader::new(data.as_bytes());
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        ProtocolError::HeaderParseError(msg) => {
            assert!(
                msg.contains("too many headers"),
                "unexpected error message: {}",
                msg
            );
        }
        other => panic!("Expected HeaderParseError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_wire_unterminated_content_length_header() {
    // A complete-but-unterminated `Content-Length: 5` (no `\r\n` and no body)
    // is accepted as a valid header line; the missing terminator and body
    // surface as an `IoError(unexpected EOF)` on the next read. Lock down
    // that behavior so we notice if it changes.
    let data = b"Content-Length: 5";
    let mut cursor = tokio::io::BufReader::new(&data[..]);
    let result = Message::read(&mut cursor).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        ProtocolError::IoError(msg) => {
            assert!(
                msg.contains("unexpected EOF"),
                "unexpected error message: {}",
                msg
            );
        }
        other => panic!("Expected IoError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_wire_truncated_header_no_colon() {
    // A truncated key-only line with no colon and no terminator should
    // surface a HeaderParseError via the missing-colon path.
    let data = b"Content-Len";
    let mut cursor = tokio::io::BufReader::new(&data[..]);
    let result = Message::read(&mut cursor).await;
    assert!(matches!(result, Err(ProtocolError::HeaderParseError(_))));
}

#[tokio::test]
async fn test_wire_content_length_case_insensitive() {
    // DAP base protocol headers are case-insensitive (inherited from RFC 822 / HTTP);
    // some real-world adapters ship lowercase or with extra whitespace around the key.
    let json = r#"{"seq":1,"type":"event","event":"initialized","body":{}}"#;

    // Lowercase key.
    let header = format!("content-length: {}\r\n\r\n", json.len());
    let data = format!("{}{}", header, json);
    let mut cursor = tokio::io::BufReader::new(data.as_bytes());
    let parsed = Message::read(&mut cursor)
        .await
        .expect("lowercase content-length header should parse")
        .expect("should return Some(message)");
    assert!(matches!(parsed, Message::Event(_)));

    // Trailing whitespace before the colon.
    let header = format!("Content-Length : {}\r\n\r\n", json.len());
    let data = format!("{}{}", header, json);
    let mut cursor = tokio::io::BufReader::new(data.as_bytes());
    let parsed = Message::read(&mut cursor)
        .await
        .expect("Content-Length with trailing whitespace before colon should parse")
        .expect("should return Some(message)");
    assert!(matches!(parsed, Message::Event(_)));
}

#[test]
fn test_parse_set_exception_breakpoints_response_no_body() {
    let msg = parse_message(json!({
        "seq": 5,
        "type": "response",
        "request_seq": 4,
        "success": true,
        "command": "setExceptionBreakpoints"
    }));

    let Message::Response(response) = msg else {
        panic!("Expected Response");
    };
    assert!(response.success);
    assert!(matches!(
        response.body,
        ResponseBody::SetExceptionBreakpoints(None)
    ));
}

#[test]
fn test_parse_set_exception_breakpoints_response_with_body() {
    let msg = parse_message(json!({
        "seq": 5,
        "type": "response",
        "request_seq": 4,
        "success": true,
        "command": "setExceptionBreakpoints",
        "body": {
            "breakpoints": [
                { "id": 1, "verified": true }
            ]
        }
    }));

    let Message::Response(response) = msg else {
        panic!("Expected Response");
    };
    let ResponseBody::SetExceptionBreakpoints(Some(body)) = &response.body else {
        panic!("Expected SetExceptionBreakpoints body");
    };
    let bps = body.breakpoints.as_ref().expect("Expected breakpoints");
    assert_eq!(bps.len(), 1);
    assert_eq!(bps[0].id, Some(BreakpointId(1)));
    assert!(bps[0].verified);
}

#[test]
fn test_parse_initialize_response_no_body() {
    let msg = parse_message(json!({
        "seq": 2,
        "type": "response",
        "request_seq": 1,
        "success": true,
        "command": "initialize"
    }));

    let Message::Response(response) = msg else {
        panic!("Expected Response");
    };
    assert!(matches!(response.body, ResponseBody::Initialize(None)));
}

#[test]
fn test_parse_disconnect_request_no_arguments() {
    let msg = parse_message(json!({
        "seq": 10,
        "type": "request",
        "command": "disconnect"
    }));

    let Message::Request(request) = msg else {
        panic!("Expected Request");
    };
    assert!(matches!(request.command, RequestCommand::Disconnect(None)));
}

#[test]
fn test_parse_configuration_done_request_no_arguments() {
    let msg = parse_message(json!({
        "seq": 5,
        "type": "request",
        "command": "configurationDone"
    }));

    let Message::Request(request) = msg else {
        panic!("Expected Request");
    };
    assert!(matches!(
        request.command,
        RequestCommand::ConfigurationDone(None)
    ));
}

#[test]
fn test_configuration_done_none_should_not_serialize_arguments_null() {
    // Simulates what the dapper proxy does: receives a configurationDone request
    // without arguments from VS Code, deserializes it to ConfigurationDone(None),
    // then re-serializes it to forward to the backend (e.g. JavaDAP).
    // The re-serialized JSON must NOT contain "arguments": null, as that is not
    // spec-compliant and might cause problems for the DAP servers.
    let incoming = json!({
        "seq": 5,
        "type": "request",
        "command": "configurationDone"
    });

    // Step 1: Deserialize (as the proxy would when receiving from VS Code)
    let msg: Message = serde_json::from_value(incoming).unwrap();
    let Message::Request(ref request) = msg else {
        panic!("Expected Request");
    };
    assert!(matches!(
        request.command,
        RequestCommand::ConfigurationDone(None)
    ));

    // Step 2: Re-serialize (as the proxy would when forwarding to the backend)
    let json_val = msg.to_value().unwrap();

    // The "arguments" key should either be absent or be an empty object,
    // but NEVER null.
    if let Some(args) = json_val.get("arguments") {
        assert!(
            !args.is_null(),
            "configurationDone with no arguments serialized as \"arguments\": null. \
             This is not DAP spec-compliant and might cause problems for DAP servers. \
             Serialized JSON: {json_val}"
        );
    }
}

// -- i64_from_value tests --

#[test]
fn i64_from_value_integer() {
    assert_eq!(i64_from_value(&json!(42)), Ok(42));
}

#[test]
fn i64_from_value_negative_integer() {
    assert_eq!(i64_from_value(&json!(-7)), Ok(-7));
}

#[test]
fn i64_from_value_string() {
    assert_eq!(i64_from_value(&json!("42")), Ok(42));
}

#[test]
fn i64_from_value_negative_string() {
    assert_eq!(i64_from_value(&json!("-7")), Ok(-7));
}

#[test]
fn i64_from_value_float_rejected() {
    assert!(i64_from_value(&json!(1.5)).is_err());
}

#[test]
fn i64_from_value_empty_string_rejected() {
    assert!(i64_from_value(&json!("")).is_err());
}

#[test]
fn i64_from_value_non_numeric_string_rejected() {
    assert!(i64_from_value(&json!("abc")).is_err());
}

#[test]
fn i64_from_value_bool_rejected() {
    assert!(i64_from_value(&json!(true)).is_err());
}

#[test]
fn i64_from_value_null_rejected() {
    assert!(i64_from_value(&json!(null)).is_err());
}
