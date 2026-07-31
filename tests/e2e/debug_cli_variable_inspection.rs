// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::get_frame_ids;
use dapper_e2e_support::get_scope_variables_references;
use dapper_e2e_support::get_thread_ids;
use dapper_e2e_support::parse_frame_id_from_stack_trace_response;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::parse_variables_reference_from_scopes_response;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

#[tokio::test]
async fn debug_cli_variable_inspection() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-var-inspect")?;

    let threads_result = run_debug_command(Some(scope_id.clone()), &["threads"]).await?;
    assert!(
        threads_result.success,
        "threads command failed, stderr: {}",
        threads_result.stderr
    );
    let thread_id = parse_thread_id_from_threads_response(&threads_result.stdout)?;

    let stack_trace_result = run_debug_command(
        Some(scope_id.clone()),
        &["stack-trace", &thread_id.to_string()],
    )
    .await?;
    assert!(
        stack_trace_result.success,
        "stack-trace command failed, stderr: {}",
        stack_trace_result.stderr
    );
    assert!(
        !stack_trace_result.stdout.contains("No stack frames found"),
        "expected stack frames in stopped debug session, got: {}",
        stack_trace_result.stdout
    );
    let frame_id = parse_frame_id_from_stack_trace_response(&stack_trace_result.stdout)?;

    let scopes_result =
        run_debug_command(Some(scope_id.clone()), &["scopes", &frame_id.to_string()]).await?;
    assert!(
        scopes_result.success,
        "scopes command failed, stderr: {}",
        scopes_result.stderr
    );
    assert!(
        scopes_result.stdout.contains("Scope:"),
        "scopes output should contain scope entries, got: {}",
        scopes_result.stdout
    );
    let variables_reference =
        parse_variables_reference_from_scopes_response(&scopes_result.stdout)?;
    assert!(
        variables_reference.0 > 0,
        "variables reference should be positive, got: {}",
        variables_reference
    );

    let variables_result = run_debug_command(
        Some(scope_id),
        &["variables", &variables_reference.to_string()],
    )
    .await?;
    assert!(
        variables_result.success,
        "variables command failed, stderr: {}",
        variables_result.stderr
    );
    assert!(
        variables_result.stdout.contains("Variables for reference"),
        "variables output should contain variable listing header, got: {}",
        variables_result.stdout
    );

    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn debug_cli_variable_inspection_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-var-inspect-json")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];
    let frame_id = get_frame_ids(&scope_id, thread_id).await?[0];
    let variables_reference = get_scope_variables_references(&scope_id, frame_id).await?[0];

    let variables_result = run_debug_command(
        Some(scope_id),
        &["--json", "variables", &variables_reference.to_string()],
    )
    .await?;
    assert!(
        variables_result.success,
        "variables --json command failed, stderr: {}",
        variables_result.stderr
    );
    let variables_parsed: serde_json::Value = serde_json::from_str(&variables_result.stdout)
        .unwrap_or_else(|e| {
            panic!(
                "variables --json output should be valid JSON, got error: {e}, stdout: {}",
                variables_result.stdout
            )
        });

    let variables = variables_parsed["result"]
        .get("variables")
        .and_then(|v| v.as_array())
        .expect("variables result should contain 'variables' array");
    assert!(
        !variables.is_empty(),
        "variables should not be empty in a stopped frame"
    );

    let first_var = &variables[0];
    assert!(
        first_var.get("name").and_then(|n| n.as_str()).is_some(),
        "each variable should have a string 'name', got: {}",
        first_var
    );
    assert!(
        first_var.get("value").and_then(|v| v.as_str()).is_some(),
        "each variable should have a string 'value', got: {}",
        first_var
    );

    dap_client.kill()?;
    Ok(())
}
