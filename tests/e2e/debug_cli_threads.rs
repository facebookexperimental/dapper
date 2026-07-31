// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadResult {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadsResult {
    threads: Vec<ThreadResult>,
    #[serde(rename = "stackTrace")]
    _stack_trace: Option<serde_json::Map<String, serde_json::Value>>,
}

#[tokio::test]
async fn debug_cli_threads() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-threads")?;

    let result = run_debug_command(Some(scope_id), &["threads"]).await?;

    assert!(
        result.success,
        "threads command should succeed, stderr: {}",
        result.stderr
    );

    assert!(
        result.stdout.contains("Thread "),
        "stdout should list at least one thread, got: {}",
        result.stdout
    );

    let thread_id = parse_thread_id_from_threads_response(&result.stdout)?;
    assert!(
        thread_id.0 > 0,
        "parsed thread ID should be positive, got: {}",
        thread_id
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_threads_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-threads-json")?;

    let result = run_debug_command(Some(scope_id), &["--json", "threads"]).await?;

    assert!(
        result.success,
        "threads --json command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "threads --json output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let threads_result: ThreadsResult = serde_json::from_value(parsed["result"].clone())
        .context("JSON result should match the threads response shape")?;
    assert!(
        !threads_result.threads.is_empty(),
        "threads array should not be empty in a stopped session"
    );
    assert!(
        threads_result
            .threads
            .iter()
            .all(|thread| thread.id > 0 && !thread.name.is_empty()),
        "each thread should have a positive ID and non-empty name"
    );

    // Validate context envelope
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
