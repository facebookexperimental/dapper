// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::get_thread_ids;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

#[tokio::test]
async fn debug_cli_step_over() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-step-over")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let step_result =
        run_debug_command(Some(scope_id), &["step", "over", &thread_id.to_string()]).await?;

    assert!(
        step_result.success,
        "step over command should succeed, stderr: {}",
        step_result.stderr
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_step_over_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-step-over-json")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let step_result = run_debug_command(
        Some(scope_id),
        &["step", "over", &thread_id.to_string(), "--json"],
    )
    .await?;
    assert!(step_result.success, "stderr: {}", step_result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&step_result.stdout)?;
    assert_eq!(
        parsed["result"],
        serde_json::json!({
            "navigationType": "step_over",
            "result": {"type": "commandExecuted"}
        })
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_step_in_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-step-in-json")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let step_result = run_debug_command(
        Some(scope_id),
        &["step", "in", &thread_id.to_string(), "--json"],
    )
    .await?;
    assert!(step_result.success, "stderr: {}", step_result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&step_result.stdout)?;
    assert_eq!(
        parsed["result"],
        serde_json::json!({
            "navigationType": "step_in",
            "result": {"type": "commandExecuted"}
        })
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_step_out_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-step-out-json")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let step_result = run_debug_command(
        Some(scope_id),
        &["step", "out", &thread_id.to_string(), "--json"],
    )
    .await?;
    assert!(step_result.success, "stderr: {}", step_result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&step_result.stdout)?;
    assert_eq!(
        parsed["result"],
        serde_json::json!({
            "navigationType": "step_out",
            "result": {"type": "commandExecuted"}
        })
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_continue() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("debug-cli-continue")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let continue_result =
        run_debug_command(Some(scope_id), &["continue", &thread_id.to_string()]).await?;

    assert!(
        continue_result.success,
        "continue command should succeed, stderr: {}",
        continue_result.stderr
    );

    Ok(())
}

#[tokio::test]
async fn debug_cli_continue_json() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("debug-cli-continue-json")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let continue_result = run_debug_command(
        Some(scope_id),
        &["continue", &thread_id.to_string(), "--json"],
    )
    .await?;
    assert!(
        continue_result.success,
        "stderr: {}",
        continue_result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&continue_result.stdout)?;

    // Validate the result portion
    let result = parsed
        .get("result")
        .expect("JSON output should contain a 'result' field");
    let exit_code = result["result"]["data"]["exitCode"].clone();
    assert!(exit_code.is_number());
    assert_eq!(
        *result,
        serde_json::json!({
            "navigationType": "continue",
            "result": {"type": "exited", "data": {"exitCode": exit_code}}
        })
    );

    Ok(())
}
