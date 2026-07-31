// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use dapper_e2e_support::DapClient;
use dapper_e2e_support::find_breakpoint_line_by_marker;
use dapper_e2e_support::get_frame_ids;
use dapper_e2e_support::get_scope_variables_references;
use dapper_e2e_support::get_thread_ids;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_e2e_support::test_source_path;
use dapper_session::ScopeId;

#[tokio::test]
async fn debug_cli_set_variable_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-set-var-json")?;

    // Advance from the adapter's initial stop to a shared fixture marker where
    // the mutable `total` local is in scope.
    advance_to_default_stop(&scope_id, &mut dap_client).await?;

    let thread_id = get_thread_ids(&scope_id).await?[0];
    let frame_id = get_frame_ids(&scope_id, thread_id).await?[0];
    let variables_reference = get_scope_variables_references(&scope_id, frame_id).await?[0];

    // Get variables to find a variable name we can set
    let variables_result = run_debug_command(
        Some(scope_id.clone()),
        &["variables", &variables_reference.to_string(), "--json"],
    )
    .await?;
    assert!(
        variables_result.success,
        "variables --json command failed, stderr: {}",
        variables_result.stderr
    );
    let variables_parsed: serde_json::Value = serde_json::from_str(&variables_result.stdout)?;
    let variables = variables_parsed["result"]
        .get("variables")
        .and_then(|v| v.as_array())
        .expect("variables result should contain 'variables' array");
    assert!(
        !variables.is_empty(),
        "variables should not be empty in a stopped frame"
    );
    // Both fixtures expose the same scalar at the shared marker.
    let target_var_name = "total";
    let var_name = variables
        .iter()
        .filter_map(|v| v["name"].as_str())
        .find(|n| *n == target_var_name)
        .with_context(|| {
            format!("expected variable '{target_var_name}' in scope, got: {variables:#?}")
        })?;

    // Set the variable to a new value
    let set_result = run_debug_command(
        Some(scope_id),
        &[
            "set-variable",
            &variables_reference.to_string(),
            var_name,
            "42",
            "--json",
        ],
    )
    .await?;

    assert!(set_result.success, "stderr: {}", set_result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&set_result.stdout)?;
    assert_eq!(
        parsed["result"]["name"].as_str(),
        Some(var_name),
        "set-variable result should contain the variable name"
    );
    assert!(
        parsed["result"]["body"]["value"].as_str().is_some(),
        "set-variable result body should contain 'value', got: {}",
        parsed["result"]["body"]
    );

    dap_client.kill()?;
    Ok(())
}

/// Advances a debug session from its initial stop to the `breakpoint
/// default_stop` marker line in the adapter's example source, where a
/// user-code scalar local is in scope.
///
/// Sends `setBreakpoints` followed by `continue`, then waits for the next
/// `stopped` event.
async fn advance_to_default_stop(
    scope_id: &ScopeId,
    dap_client: &mut DapClient,
) -> anyhow::Result<()> {
    // Seq numbers reserved for this helper. The launch sequence uses seqs 1, 2,
    // and 6 (initialize, launch, configurationDone); 100/101 are far above to
    // avoid colliding with anything the surrounding setup might send.
    const SET_BREAKPOINTS_SEQ: i64 = 100;
    const CONTINUE_SEQ: i64 = 101;

    let source_path = test_source_path()?;
    let bp_line = find_breakpoint_line_by_marker(&source_path, "breakpoint default_stop").await?;

    let set_bp_req = serde_json::json!({
        "type": "request",
        "seq": SET_BREAKPOINTS_SEQ,
        "command": "setBreakpoints",
        "arguments": {
            "source": {"path": source_path},
            "breakpoints": [{"line": bp_line}],
        },
    })
    .to_string();
    dap_client.send(set_bp_req)?;
    dap_client.read_response()?;

    let thread_id = get_thread_ids(scope_id).await?[0];
    let continue_req = serde_json::json!({
        "type": "request",
        "seq": CONTINUE_SEQ,
        "command": "continue",
        "arguments": {"threadId": thread_id},
    })
    .to_string();
    dap_client.send(continue_req)?;
    dap_client.read_response()?;
    dap_client.wait_for_event("stopped")?;

    Ok(())
}
