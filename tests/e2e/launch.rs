// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::net::TcpListener;

use anyhow::Context;
use dapper_e2e_support::DapClient;
use dapper_e2e_support::generate_test_scope_id;
use serde_json::json;

fn send_launch_request(client: &mut DapClient) -> anyhow::Result<()> {
    let debuggee = std::env::var("DAPPER_TEST_DEBUGGEE")
        .context("DAPPER_TEST_DEBUGGEE must name the test debuggee")?;
    client.send(
        json!({
            "type": "request",
            "command": "launch",
            "arguments": {
                "noDebug": false,
                "program": debuggee,
                "args": [],
            },
            "seq": 2,
        })
        .to_string(),
    )?;

    client.wait_for_event("initialized")?;
    Ok(())
}

#[test]
fn launch_request() -> anyhow::Result<()> {
    let scope_id = generate_test_scope_id("launch-request");

    let mut client = DapClient::new(Some(scope_id))?;
    client.initialize()?;

    let dapper_event = client.wait_for_event("dapper")?;
    let body = dapper_event
        .body
        .context("dapper event should have a body")?;
    assert_eq!(body["success"].as_bool(), Some(true));
    assert_eq!(body["category"].as_str(), Some("controlPlaneStatus"));
    assert!(body.get("sessionId").is_some());

    send_launch_request(&mut client)?;
    client.kill()?;
    Ok(())
}

#[test]
fn launch_without_control_plane() -> anyhow::Result<()> {
    let scope_id = generate_test_scope_id("launch-without-control-plane");

    let port_blocker =
        TcpListener::bind("127.0.0.1:0").context("failed to bind to an available port")?;
    let control_plane_port = port_blocker.local_addr()?.port();

    let mut client =
        DapClient::new_with_control_plane_port(Some(control_plane_port), Some(scope_id))?;
    client.initialize()?;

    let dapper_event = client.wait_for_event("dapper")?;
    let body = dapper_event
        .body
        .context("dapper event should have a body")?;
    assert_eq!(body["success"].as_bool(), Some(false));
    assert_eq!(body["category"].as_str(), Some("controlPlaneStatus"));
    assert!(body.get("message").is_some());
    assert!(body.get("sessionId").is_some());

    send_launch_request(&mut client)?;
    client.kill()?;
    Ok(())
}
