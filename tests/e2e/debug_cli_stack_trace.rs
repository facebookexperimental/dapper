// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use dapper_e2e_support::get_thread_ids;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;
use serde::Deserialize;

#[derive(Deserialize)]
struct StackFrameResult {
    #[serde(rename = "id")]
    _id: i64,
    #[serde(rename = "name")]
    _name: String,
    #[serde(rename = "line")]
    _line: i64,
    #[serde(rename = "column")]
    _column: i64,
    #[serde(rename = "endLine")]
    _end_line: Option<i64>,
    #[serde(rename = "endColumn")]
    _end_column: Option<i64>,
    #[serde(rename = "source")]
    _source: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(rename = "canRestart")]
    _can_restart: Option<bool>,
    #[serde(rename = "instructionPointerReference")]
    _instruction_pointer_reference: Option<String>,
    #[serde(rename = "moduleId")]
    _module_id: Option<serde_json::Value>,
    #[serde(rename = "presentationHint")]
    _presentation_hint: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StackTraceResult {
    #[serde(rename = "stackFrames")]
    stack_frames: Vec<StackFrameResult>,
    #[serde(rename = "startFrame")]
    _start_frame: i64,
    #[serde(rename = "hasMoreFrames")]
    _has_more_frames: bool,
    #[serde(rename = "threadId")]
    _thread_id: i64,
    #[serde(rename = "scopes")]
    _scopes: Option<serde_json::Map<String, serde_json::Value>>,
}

#[tokio::test]
async fn debug_cli_stack_trace_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-stack-trace-json")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];

    let result = run_debug_command(
        Some(scope_id),
        &["--json", "stack-trace", &thread_id.to_string()],
    )
    .await?;

    assert!(
        result.success,
        "stack-trace --json command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "stack-trace --json output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let stack_trace_result: StackTraceResult = serde_json::from_value(parsed["result"].clone())
        .context("JSON result should match the stack-trace response shape")?;
    assert!(
        !stack_trace_result.stack_frames.is_empty(),
        "stackFrames should not be empty for a stopped thread"
    );

    dap_client.kill()?;
    Ok(())
}
