// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

#[tokio::test]
async fn debug_cli_status() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-status")?;

    let result = run_debug_command(Some(scope_id), &["status"]).await?;

    assert!(
        result.success,
        "status command should succeed, stderr: {}",
        result.stderr
    );

    assert!(
        result.stdout.contains("execution status: stopped"),
        "stdout should contain execution state, got: {}",
        result.stdout
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_status_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-status-json")?;

    let result = run_debug_command(Some(scope_id), &["--json", "status"]).await?;

    assert!(
        result.success,
        "status --json command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "status --json output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let context = parsed
        .get("context")
        .expect("JSON output should contain a 'context' field");
    let status = context
        .get("executionState")
        .and_then(|es| es.get("status"))
        .and_then(|s| s.as_str())
        .expect("context should contain executionState.status");
    assert_eq!(
        status, "stopped",
        "execution state should be 'stopped' in a stopped session"
    );

    dap_client.kill()?;
    Ok(())
}
