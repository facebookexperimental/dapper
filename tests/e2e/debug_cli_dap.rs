// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

#[tokio::test]
async fn debug_cli_dap_threads() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-dap-threads")?;

    let result = run_debug_command(Some(scope_id.clone()), &["dap", "threads"]).await?;

    assert!(
        result.success,
        "dap threads command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "dap threads output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let threads = parsed
        .get("threads")
        .and_then(|t| t.as_array())
        .expect("dap threads response should contain a 'threads' array");
    assert!(
        !threads.is_empty(),
        "threads array should not be empty in a stopped session"
    );

    let first_thread = &threads[0];
    assert!(
        first_thread.get("id").and_then(|id| id.as_i64()).is_some(),
        "each thread should have a numeric 'id', got: {}",
        first_thread
    );
    assert!(
        first_thread.get("name").and_then(|n| n.as_str()).is_some(),
        "each thread should have a string 'name', got: {}",
        first_thread
    );

    let json_result = run_debug_command(Some(scope_id), &["dap", "threads", "--json"]).await?;
    assert!(
        json_result.success,
        "dap threads --json command should succeed, stderr: {}",
        json_result.stderr
    );
    let json_stdout = json_result.stdout.trim();
    assert!(
        !json_stdout.contains('\n'),
        "dap threads --json output should be compact (single line), got: {}",
        json_stdout
    );
    let json_parsed: serde_json::Value = serde_json::from_str(json_stdout).unwrap_or_else(|e| {
        panic!(
            "dap threads --json output should be valid JSON, got error: {e}, stdout: {}",
            json_stdout
        )
    });
    assert!(
        json_parsed
            .get("threads")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty()),
        "dap threads --json output should carry a non-empty 'threads' array, got: {}",
        json_stdout
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_dap_stacktrace() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-dap-stacktrace")?;

    let threads_result = run_debug_command(Some(scope_id.clone()), &["dap", "threads"]).await?;
    assert!(
        threads_result.success,
        "intermediate dap threads command should succeed, stderr: {}",
        threads_result.stderr
    );
    let threads_parsed: serde_json::Value = serde_json::from_str(&threads_result.stdout)?;
    let thread_id = threads_parsed["threads"][0]["id"]
        .as_i64()
        .expect("thread id");

    let args_json = format!(r#"{{"threadId": {}}}"#, thread_id);

    let result = run_debug_command(
        Some(scope_id.clone()),
        &["dap", "stackTrace", "--arguments", &args_json],
    )
    .await?;

    assert!(
        result.success,
        "dap stackTrace command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "dap stackTrace output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let frames = parsed
        .get("stackFrames")
        .and_then(|f| f.as_array())
        .expect("stackTrace response should contain 'stackFrames' array");
    assert!(
        !frames.is_empty(),
        "stackFrames should not be empty for a stopped thread"
    );

    let first_frame = &frames[0];
    assert!(
        first_frame.get("id").and_then(|id| id.as_i64()).is_some(),
        "each frame should have a numeric 'id', got: {}",
        first_frame
    );
    assert!(
        first_frame.get("name").and_then(|n| n.as_str()).is_some(),
        "each frame should have a string 'name', got: {}",
        first_frame
    );

    let json_result = run_debug_command(
        Some(scope_id),
        &["dap", "stackTrace", "--arguments", &args_json, "--json"],
    )
    .await?;
    assert!(
        json_result.success,
        "dap stackTrace --json command should succeed, stderr: {}",
        json_result.stderr
    );
    let json_stdout = json_result.stdout.trim();
    assert!(
        !json_stdout.contains('\n'),
        "dap stackTrace --json output should be compact (single line), got: {}",
        json_stdout
    );
    let json_parsed: serde_json::Value = serde_json::from_str(json_stdout).unwrap_or_else(|e| {
        panic!(
            "dap stackTrace --json output should be valid JSON, got error: {e}, stdout: {}",
            json_stdout
        )
    });
    assert!(
        json_parsed
            .get("stackFrames")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty()),
        "dap stackTrace --json output should carry a non-empty 'stackFrames' array, got: {}",
        json_stdout
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_dap_scopes() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-dap-scopes")?;

    let threads_result = run_debug_command(Some(scope_id.clone()), &["dap", "threads"]).await?;
    assert!(
        threads_result.success,
        "intermediate dap threads command should succeed, stderr: {}",
        threads_result.stderr
    );
    let threads_parsed: serde_json::Value = serde_json::from_str(&threads_result.stdout)?;
    let thread_id = threads_parsed["threads"][0]["id"]
        .as_i64()
        .expect("thread id");

    let stack_result = run_debug_command(
        Some(scope_id.clone()),
        &[
            "dap",
            "stackTrace",
            "--arguments",
            &format!(r#"{{"threadId": {}}}"#, thread_id),
        ],
    )
    .await?;
    assert!(
        stack_result.success,
        "intermediate dap stackTrace command should succeed, stderr: {}",
        stack_result.stderr
    );
    let stack_parsed: serde_json::Value = serde_json::from_str(&stack_result.stdout)?;
    let frame_id = stack_parsed["stackFrames"][0]["id"]
        .as_i64()
        .expect("frame id");

    let args_json = format!(r#"{{"frameId": {}}}"#, frame_id);

    let result = run_debug_command(
        Some(scope_id.clone()),
        &["dap", "scopes", "--arguments", &args_json],
    )
    .await?;

    assert!(
        result.success,
        "dap scopes command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "dap scopes output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let scopes = parsed
        .get("scopes")
        .and_then(|s| s.as_array())
        .expect("scopes response should contain 'scopes' array");
    assert!(
        !scopes.is_empty(),
        "scopes should not be empty for a valid frame"
    );

    let first_scope = &scopes[0];
    assert!(
        first_scope.get("name").and_then(|n| n.as_str()).is_some(),
        "each scope should have a string 'name', got: {}",
        first_scope
    );
    assert!(
        first_scope
            .get("variablesReference")
            .and_then(|v| v.as_i64())
            .is_some(),
        "each scope should have a numeric 'variablesReference', got: {}",
        first_scope
    );

    let json_result = run_debug_command(
        Some(scope_id),
        &["dap", "scopes", "--arguments", &args_json, "--json"],
    )
    .await?;
    assert!(
        json_result.success,
        "dap scopes --json command should succeed, stderr: {}",
        json_result.stderr
    );
    let json_stdout = json_result.stdout.trim();
    assert!(
        !json_stdout.contains('\n'),
        "dap scopes --json output should be compact (single line), got: {}",
        json_stdout
    );
    let json_parsed: serde_json::Value = serde_json::from_str(json_stdout).unwrap_or_else(|e| {
        panic!(
            "dap scopes --json output should be valid JSON, got error: {e}, stdout: {}",
            json_stdout
        )
    });
    assert!(
        json_parsed
            .get("scopes")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty()),
        "dap scopes --json output should carry a non-empty 'scopes' array, got: {}",
        json_stdout
    );

    dap_client.kill()?;
    Ok(())
}
