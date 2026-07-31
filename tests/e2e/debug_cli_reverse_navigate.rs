// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::REVERSE_DEBUG_GATING_MSG;
use dapper_e2e_support::get_thread_ids;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_fake_debug_session;

#[tokio::test]
async fn debug_cli_step_back() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) =
        setup_stopped_fake_debug_session("debug-cli-step-back", &["--supports-step-back", "true"])?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let step_result =
        run_debug_command(Some(scope_id), &["step", "back", &thread_id.to_string()]).await?;

    assert!(
        step_result.success,
        "step back command should succeed, stderr: {}",
        step_result.stderr
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_step_back_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_fake_debug_session(
        "debug-cli-step-back-json",
        &["--supports-step-back", "true"],
    )?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let step_result = run_debug_command(
        Some(scope_id),
        &["step", "back", &thread_id.to_string(), "--json"],
    )
    .await?;
    assert!(step_result.success, "stderr: {}", step_result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&step_result.stdout)?;
    assert_eq!(
        parsed["result"],
        serde_json::json!({
            "navigationType": "step_back",
            "result": {"type": "commandExecuted"}
        })
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_reverse_continue() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_fake_debug_session(
        "debug-cli-reverse-continue",
        &["--supports-step-back", "true"],
    )?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let result = run_debug_command(
        Some(scope_id),
        &["reverse-continue", &thread_id.to_string()],
    )
    .await?;

    assert!(
        result.success,
        "reverse-continue command should succeed, stderr: {}",
        result.stderr
    );

    Ok(())
}

#[tokio::test]
async fn debug_cli_reverse_continue_json() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_fake_debug_session(
        "debug-cli-reverse-continue-json",
        &["--supports-step-back", "true"],
    )?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let result = run_debug_command(
        Some(scope_id),
        &["reverse-continue", &thread_id.to_string(), "--json"],
    )
    .await?;
    assert!(result.success, "stderr: {}", result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout)?;
    let result_obj = parsed
        .get("result")
        .expect("JSON output should contain a 'result' field");
    assert_eq!(
        result_obj.get("navigationType"),
        Some(&serde_json::json!("reverse_continue"))
    );
    // reverse-continue enters the wait branch in ProxyClient::navigate; the
    // fake emits a stopped event after responding, so the result type is
    // "stopped". Don't assert on inner `data` body fields — that's
    // dapper_dap_protocol's territory and the shape may shift independently.
    let inner = result_obj
        .get("result")
        .expect("JSON output should contain result.result");
    assert_eq!(inner.get("type"), Some(&serde_json::json!("stopped")));

    Ok(())
}

#[tokio::test]
async fn debug_cli_step_back_gated() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_fake_debug_session(
        "debug-cli-step-back-gated",
        &["--supports-step-back", "false", "--fail-on-reverse"],
    )?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let step_result = run_debug_command(
        Some(scope_id.clone()),
        &["step", "back", &thread_id.to_string()],
    )
    .await?;

    assert!(
        !step_result.success,
        "step back should be gated when supportsStepBack is false, stderr: {}",
        step_result.stderr
    );
    assert!(
        step_result.stderr.contains(REVERSE_DEBUG_GATING_MSG),
        "expected gating error message, stderr: {}",
        step_result.stderr
    );

    // Liveness check: if the proxy ever forwarded the reverse request, the
    // fake's --fail-on-reverse would have called exit(2), the proxy pipe
    // would be broken, and threads --json would fail. Success here proves
    // the gate fired upstream and the fake never saw the reverse request.
    let liveness = run_debug_command(Some(scope_id), &["threads", "--json"]).await?;
    assert!(
        liveness.success,
        "follow-up threads --json should succeed if the fake adapter is still alive (proves gating worked), stdout: {}, stderr: {}",
        liveness.stdout, liveness.stderr
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_reverse_continue_gated() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_fake_debug_session(
        "debug-cli-reverse-continue-gated",
        &["--supports-step-back", "false", "--fail-on-reverse"],
    )?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let result = run_debug_command(
        Some(scope_id.clone()),
        &["reverse-continue", &thread_id.to_string()],
    )
    .await?;

    assert!(
        !result.success,
        "reverse-continue should be gated when supportsStepBack is false, stderr: {}",
        result.stderr
    );
    assert!(
        result.stderr.contains(REVERSE_DEBUG_GATING_MSG),
        "expected gating error message, stderr: {}",
        result.stderr
    );

    let liveness = run_debug_command(Some(scope_id), &["threads", "--json"]).await?;
    assert!(
        liveness.success,
        "follow-up threads --json should succeed if the fake adapter is still alive (proves gating worked), stdout: {}, stderr: {}",
        liveness.stdout, liveness.stderr
    );

    dap_client.kill()?;
    Ok(())
}
