// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::parse_frame_id_from_stack_trace_response;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::parse_variables_reference_from_scopes_response;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;
use rmcp::serde_json;

#[tokio::test]
async fn variable_inspection() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("variable-inspection")?;
    dap_client.try_consume_pending_responses()?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    let threads_content = extract_text_content(&threads_result);
    assert!(
        threads_result.is_error != Some(true),
        "Threads request failed: {}",
        threads_content
    );
    let thread_id = parse_thread_id_from_threads_response(&threads_content)?;

    let stack_trace_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::StackTrace);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "thread_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(thread_id.0)),
                );
                map
            });
            params
        })
        .await?;

    let stack_trace_content = extract_text_content(&stack_trace_result);
    assert!(
        stack_trace_result.is_error != Some(true),
        "Stack trace request failed: {}",
        stack_trace_content
    );
    assert!(
        !stack_trace_content.contains("No stack frames found"),
        "Expected stack frames in stopped debug session, got: {}",
        stack_trace_content
    );
    let frame_id = parse_frame_id_from_stack_trace_response(&stack_trace_content)?;

    let scopes_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Scopes);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "frame_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(frame_id.0)),
                );
                map
            });
            params
        })
        .await?;

    let scopes_content = extract_text_content(&scopes_result);
    assert!(
        scopes_result.is_error != Some(true),
        "Scopes request failed: {}",
        scopes_content
    );
    assert!(
        !scopes_content.is_empty() && !scopes_content.contains("No scopes found"),
        "Expected scopes in stopped debug session, got: {}",
        scopes_content
    );
    assert!(
        scopes_content.contains("Scope:"),
        "Scopes response should contain scope entries, got: {}",
        scopes_content
    );

    let variables_reference = parse_variables_reference_from_scopes_response(&scopes_content)?;
    assert!(
        variables_reference.0 > 0,
        "Variables reference should be positive, got: {}",
        variables_reference
    );

    let variables_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Variables);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "variables_reference".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(variables_reference.0)),
                );
                map
            });
            params
        })
        .await?;

    let variables_content = extract_text_content(&variables_result);
    assert!(
        variables_result.is_error != Some(true),
        "Variables request failed: {}",
        variables_content
    );
    assert!(
        variables_content.contains("Variables for reference"),
        "Variables response should contain variable listing header, got: {}",
        variables_content
    );
    assert!(
        !variables_content.is_empty(),
        "Variables response should not be empty"
    );

    mcp_client.cancel().await?;
    Ok(())
}
