// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::protocol::Message;
use dapper_dap_protocol::protocol::Request;
use dapper_dap_protocol::protocol::Response;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::responses::ResponseBody;
use dapper_dap_protocol::responses::UnknownResponseBody;
use dapper_session::SessionId;
use serde_json::Value;
use serde_json::json;

/// Dapper's own field, carried alongside the adapter's launch/attach payload so
/// a client can tie the session it is driving back to a dapper session.
pub const SESSION_ID_FIELD: &str = "__dapper_session_id";

/// Stamp `session_id` onto a launch or attach payload, leaving every other
/// message untouched. A `None` session id disables stamping.
pub fn stamp(message: &mut Message, session_id: Option<&SessionId>) {
    match message {
        Message::Request(request) => stamp_request(request, session_id),
        Message::Response(response) => stamp_response(response, session_id),
        Message::Event(_) | Message::Custom(_) => {}
    }
}

fn stamp_request(request: &mut Request, session_id: Option<&SessionId>) {
    let Some(session_id) = session_id else {
        return;
    };
    let extra = match &mut request.command {
        RequestCommand::Launch(args) => &mut args.extra,
        RequestCommand::Attach(args) => &mut args.extra,
        _ => return,
    };
    extra.insert(SESSION_ID_FIELD.to_owned(), json!(session_id.as_str()));
}

fn stamp_response(response: &mut Response, session_id: Option<&SessionId>) {
    let Some(session_id) = session_id else {
        return;
    };
    match &mut response.body {
        ResponseBody::Launch => response.body = stamped_body("launch", session_id),
        ResponseBody::Attach => response.body = stamped_body("attach", session_id),
        // A launch/attach response that already carried a body parsed as
        // `Unknown`, since the spec gives those two commands no body at all.
        ResponseBody::Unknown(unknown)
            if matches!(unknown.command.as_str(), "launch" | "attach") =>
        {
            let UnknownResponseBody { command, body, .. } = unknown;
            match body.get_or_insert_with(|| json!({})) {
                Value::Object(body) => {
                    body.insert(SESSION_ID_FIELD.to_owned(), json!(session_id.as_str()));
                }
                _ => tracing::warn!(
                    command = %command,
                    "adapter answered with a non-object body; session id not stamped"
                ),
            }
        }
        _ => {}
    }
}

fn stamped_body(command: &str, session_id: &SessionId) -> ResponseBody {
    ResponseBody::Unknown(UnknownResponseBody {
        command: command.to_owned(),
        body: Some(json!({ SESSION_ID_FIELD: session_id.as_str() })),
        extra: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::data_types::Seq;
    use dapper_dap_protocol::requests::AttachRequestArguments;
    use dapper_dap_protocol::requests::LaunchRequestArguments;
    use dapper_dap_protocol::requests::TerminateArguments;

    use super::*;

    fn session_id() -> SessionId {
        SessionId::new("session-1")
    }

    fn request(command: RequestCommand) -> Request {
        Request {
            seq: Seq(1),
            command,
        }
    }

    fn response(body: ResponseBody) -> Response {
        Response {
            seq: Seq(1),
            request_seq: Seq(1),
            success: true,
            message: None,
            body,
        }
    }

    fn stamped_field(value: &Value) -> Option<&Value> {
        value.get(SESSION_ID_FIELD)
    }

    #[test]
    fn launch_request_carries_the_session_id() {
        let mut request = request(RequestCommand::Launch(LaunchRequestArguments::default()));

        stamp_request(&mut request, Some(&session_id()));

        let RequestCommand::Launch(args) = &request.command else {
            panic!("expected a launch request, got {:?}", request.command);
        };
        assert_eq!(args.extra.get(SESSION_ID_FIELD), Some(&json!("session-1")));
    }

    #[test]
    fn attach_request_carries_the_session_id() {
        let mut request = request(RequestCommand::Attach(AttachRequestArguments::default()));

        stamp_request(&mut request, Some(&session_id()));

        let RequestCommand::Attach(args) = &request.command else {
            panic!("expected an attach request, got {:?}", request.command);
        };
        assert_eq!(args.extra.get(SESSION_ID_FIELD), Some(&json!("session-1")));
    }

    #[test]
    fn other_requests_are_left_alone() {
        let command = RequestCommand::Terminate(Some(TerminateArguments::default()));
        let mut request = request(command.clone());

        stamp_request(&mut request, Some(&session_id()));

        assert_eq!(request.command, command);
    }

    #[test]
    fn launch_response_carries_the_session_id_in_its_body() {
        let mut response = response(ResponseBody::Launch);

        stamp_response(&mut response, Some(&session_id()));

        let ResponseBody::Unknown(body) = &response.body else {
            panic!("expected a stamped body, got {:?}", response.body);
        };
        assert_eq!(body.command, "launch", "the command must survive stamping");
        assert_eq!(
            body.body.as_ref().and_then(stamped_field),
            Some(&json!("session-1"))
        );
    }

    #[test]
    fn attach_response_carries_the_session_id_in_its_body() {
        let mut response = response(ResponseBody::Attach);

        stamp_response(&mut response, Some(&session_id()));

        let ResponseBody::Unknown(body) = &response.body else {
            panic!("expected a stamped body, got {:?}", response.body);
        };
        assert_eq!(body.command, "attach");
        assert_eq!(
            body.body.as_ref().and_then(stamped_field),
            Some(&json!("session-1"))
        );
    }

    #[test]
    fn an_adapters_own_launch_response_body_is_kept() {
        let mut response = response(ResponseBody::Unknown(UnknownResponseBody {
            command: "launch".to_owned(),
            body: Some(json!({ "processId": 42 })),
            extra: Default::default(),
        }));

        stamp_response(&mut response, Some(&session_id()));

        let ResponseBody::Unknown(body) = &response.body else {
            panic!("expected a stamped body, got {:?}", response.body);
        };
        let body = body.body.as_ref().expect("body should still be present");
        assert_eq!(body.get("processId"), Some(&json!(42)));
        assert_eq!(stamped_field(body), Some(&json!("session-1")));
    }

    #[test]
    fn a_non_object_launch_response_body_is_left_as_the_adapter_sent_it() {
        let body = ResponseBody::Unknown(UnknownResponseBody {
            command: "launch".to_owned(),
            body: Some(json!(["not an object"])),
            extra: Default::default(),
        });
        let mut response = response(body.clone());

        stamp_response(&mut response, Some(&session_id()));

        assert_eq!(
            response.body, body,
            "there is nowhere in a non-object body to put the field, so it is logged and skipped"
        );
    }

    #[test]
    fn stamping_is_skipped_without_a_session_id() {
        let command = RequestCommand::Launch(LaunchRequestArguments::default());
        let mut request = request(command.clone());
        let mut response = response(ResponseBody::Launch);

        stamp_request(&mut request, None);
        stamp_response(&mut response, None);

        assert_eq!(request.command, command);
        assert_eq!(response.body, ResponseBody::Launch);
    }

    #[test]
    fn other_responses_are_left_alone() {
        let mut response = response(ResponseBody::Terminate);

        stamp_response(&mut response, Some(&session_id()));

        assert_eq!(response.body, ResponseBody::Terminate);
    }
}
