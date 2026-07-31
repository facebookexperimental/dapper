// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::normalize_dap_response;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

fn assert_normalized_eq(a: &serde_json::Value, b: &serde_json::Value, msg: &str) {
    let norm_a = normalize_dap_response(a);
    let norm_b = normalize_dap_response(b);
    assert_eq!(
        norm_a, norm_b,
        "{}\n\nnormalized a: {:#}\nnormalized b: {:#}\n\noriginal a: {:#}\noriginal b: {:#}",
        msg, norm_a, norm_b, a, b
    );
}

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

    // Verify --json produces equivalent but compact output
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
    // Normalize proactively: while threads responses currently have no non-idempotent
    // fields, server-side cascading expansion could add stack traces with sourceReference.
    assert_normalized_eq(
        &parsed,
        &json_parsed,
        "dap threads --json parsed value should equal default parsed value (after normalizing non-idempotent fields)",
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

    // Verify --json produces equivalent but compact output
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
    assert_normalized_eq(
        &parsed,
        &json_parsed,
        "dap stackTrace --json parsed value should equal default parsed value (after normalizing non-idempotent fields)",
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

    // Verify --json produces equivalent but compact output
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
    assert_normalized_eq(
        &parsed,
        &json_parsed,
        "dap scopes --json parsed value should equal default parsed value (after normalizing non-idempotent fields)",
    );

    dap_client.kill()?;
    Ok(())
}

#[test]
fn normalize_dap_response_zeroes_non_idempotent_fields() {
    use serde_json::json;

    let value = json!({
        "threads": [
            {"id": 12345, "name": "main"}
        ],
        "stackFrames": [
            {
                "id": 100,
                "name": "foo",
                "line": 42,
                "column": 1,
                "source": {
                    "name": "main.py",
                    "path": "/tmp/main.py",
                    "sourceReference": 7
                },
                "instructionPointerReference": "0xdeadbeef"
            }
        ],
        "scopes": [
            {
                "name": "Locals",
                "variablesReference": 33554433,
                "expensive": false
            }
        ],
        "breakpoints": [
            {"id": 5, "verified": true, "line": 10}
        ],
        "variables": [
            {
                "name": "x",
                "value": "42",
                "variablesReference": 100,
                "memoryReference": "0x1234",
                "declarationLocationReference": 50,
                "valueLocationReference": 51
            }
        ]
    });

    let value = normalize_dap_response(&value);

    // Thread IDs should be preserved (id + name only, no line/column)
    assert_eq!(value["threads"][0]["id"], 12345);
    assert_eq!(value["threads"][0]["name"], "main");

    // StackFrame id should be zeroed (has name + line + column)
    assert_eq!(value["stackFrames"][0]["id"], 0);
    // But name/line/column are preserved
    assert_eq!(value["stackFrames"][0]["name"], "foo");
    assert_eq!(value["stackFrames"][0]["line"], 42);
    assert_eq!(value["stackFrames"][0]["column"], 1);
    // sourceReference zeroed
    assert_eq!(value["stackFrames"][0]["source"]["sourceReference"], 0);
    // instructionPointerReference blanked
    assert_eq!(value["stackFrames"][0]["instructionPointerReference"], "");

    // Scope variablesReference zeroed
    assert_eq!(value["scopes"][0]["variablesReference"], 0);
    assert_eq!(value["scopes"][0]["name"], "Locals");
    assert_eq!(value["scopes"][0]["expensive"], false);

    // Breakpoint id zeroed (has verified)
    assert_eq!(value["breakpoints"][0]["id"], 0);
    assert_eq!(value["breakpoints"][0]["verified"], true);
    assert_eq!(value["breakpoints"][0]["line"], 10);

    // Variable reference fields zeroed/blanked
    assert_eq!(value["variables"][0]["variablesReference"], 0);
    assert_eq!(value["variables"][0]["memoryReference"], "");
    assert_eq!(value["variables"][0]["declarationLocationReference"], 0);
    assert_eq!(value["variables"][0]["valueLocationReference"], 0);
    // Variable name and value preserved
    assert_eq!(value["variables"][0]["name"], "x");
    assert_eq!(value["variables"][0]["value"], "42");
}
