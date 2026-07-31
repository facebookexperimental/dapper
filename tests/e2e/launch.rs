// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use dapper_e2e_support::DapClient;
use dapper_e2e_support::generate_test_scope_id;
use serde_json::json;

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

    let debuggee = std::env::var("DAPPER_TEST_DEBUGGEE")
        .context("DAPPER_TEST_DEBUGGEE must name the Python debuggee")?;
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
    client.kill()?;
    Ok(())
}
