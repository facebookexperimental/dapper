// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::io::Write;

use anyhow::Context;
use dapper_e2e_support::DapClient;
use dapper_e2e_support::adapter_command;
use dapper_e2e_support::generate_test_scope_id;
use dapper_session::config::DebugSessionConfig;
use dapper_session::config::SpawnConfig;
use dapper_session::config::StdioSpawnConfig;
use serde_json::json;

/// Note: This test does NOT include debug_request in the config, so the proxy
/// runs in normal mode expecting an external client (the test) to drive the
/// initialization sequence.
#[test]
fn proxy_from_config_launches_adapter() -> anyhow::Result<()> {
    let scope_id = generate_test_scope_id("proxy-from-config");

    let adapter = adapter_command()?;
    let debuggee = std::env::var("DAPPER_TEST_DEBUGGEE")
        .context("DAPPER_TEST_DEBUGGEE must name the test debuggee")?;

    let config = DebugSessionConfig {
        spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
            cmd: adapter.executable,
            args: adapter.arguments,
            new_session: false,
        }),
        debug_request: None,
        breakpoints: vec![],
        metadata: Default::default(),
        initialize_args: None,
        init_timeout_secs: None,
        install_default_exception_breakpoints: false,
        child_sessions: None,
    };

    let mut config_file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(&mut config_file, &config)?;
    config_file.flush()?;

    let mut client = DapClient::new_from_config(config_file.path(), Some(scope_id))?;
    client.initialize()?;

    let dapper_event = client.wait_for_event("dapper")?;
    let body = dapper_event
        .body
        .context("dapper event should have a body")?;
    assert_eq!(body["success"].as_bool(), Some(true));
    assert_eq!(body["category"].as_str(), Some("controlPlaneStatus"));
    assert!(body.get("sessionId").is_some());

    let launch_request = json!({
        "type": "request",
        "command": "launch",
        "arguments": {
            "noDebug": false,
            "program": debuggee,
            "args": [],
        },
        "seq": 2
    });
    client.send(launch_request.to_string())?;

    client.wait_for_event("initialized")?;

    client.kill()?;
    Ok(())
}
