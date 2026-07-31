// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionEntry {
    #[serde(rename = "sessionId")]
    _session_id: String,
    #[serde(rename = "pid")]
    _pid: i64,
    #[serde(rename = "controlPlanePort")]
    _control_plane_port: Option<i64>,
    #[serde(rename = "startedAt")]
    _started_at: i64,
    #[serde(rename = "commandLineArgs")]
    _command_line_args: Vec<String>,
    #[serde(rename = "currentWorkingDirectory")]
    _current_working_directory: Option<String>,
    #[serde(rename = "scopeId")]
    _scope_id: Option<String>,
    #[serde(rename = "requestType")]
    _request_type: Option<String>,
    #[serde(rename = "sessionType")]
    _session_type: Option<String>,
    #[serde(rename = "programPath")]
    _program_path: Option<String>,
    #[serde(rename = "debuggeeProcessId")]
    _debuggee_process_id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionsResult {
    sessions: Vec<SessionEntry>,
    #[serde(rename = "scopeId")]
    _scope_id: Option<String>,
}

#[tokio::test]
async fn debug_cli_sessions_lists_active() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-sessions")?;

    let result = run_debug_command(Some(scope_id.clone()), &["sessions"]).await?;

    assert!(
        result.success,
        "sessions command should succeed, stderr: {}",
        result.stderr
    );

    assert!(
        result.stdout.contains("active session"),
        "sessions output should mention active session(s), got: {}",
        result.stdout
    );

    assert!(
        result.stdout.contains(scope_id.as_str()),
        "sessions output should contain the scope ID '{}', got: {}",
        scope_id.as_str(),
        result.stdout
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_sessions_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-sessions-json")?;

    let result = run_debug_command(Some(scope_id.clone()), &["sessions", "--json"]).await?;

    assert!(
        result.success,
        "sessions --json command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "sessions --json output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let sessions_result: SessionsResult = serde_json::from_value(parsed["result"].clone())
        .context("JSON result should match the sessions response shape")?;
    assert!(
        !sessions_result.sessions.is_empty(),
        "sessions array should not be empty with an active debug session"
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_sessions_scope_filter() -> anyhow::Result<()> {
    let (_scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-sessions-filter")?;

    let nonexistent_scope = generate_test_scope_id("nonexistent");
    let result = run_debug_command(Some(nonexistent_scope), &["sessions"]).await?;

    assert!(
        result.success,
        "sessions command with non-matching scope should still succeed, stderr: {}",
        result.stderr
    );

    assert!(
        result.stdout.contains("No active sessions found"),
        "sessions output should indicate no sessions for non-matching scope, got: {}",
        result.stdout
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_sessions_scope_filter_json() -> anyhow::Result<()> {
    let (_scope_id, mut dap_client) =
        setup_stopped_debug_session("debug-cli-sessions-filter-json")?;

    let nonexistent_scope = generate_test_scope_id("nonexistent");
    let result =
        run_debug_command(Some(nonexistent_scope.clone()), &["sessions", "--json"]).await?;

    assert!(
        result.success,
        "sessions --json command with non-matching scope should still succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "sessions --json output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    assert_eq!(
        parsed["result"],
        serde_json::json!({ "sessions": [], "scopeId": nonexistent_scope.as_str() }),
        "non-matching scope should return empty sessions"
    );

    dap_client.kill()?;
    Ok(())
}
