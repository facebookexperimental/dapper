// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::data_types::ThreadId;
use dapper_e2e_support::call_navigate_command;
use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;
use rmcp::serde_json;

#[tokio::test]
async fn mcp_error_paths() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("mcp-error-paths")?;
    dap_client.try_consume_pending_responses()?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    let stack_trace_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::StackTrace);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "thread_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(999999)),
                );
                map
            });
            params
        })
        .await?;

    let stack_trace_content = extract_text_content(&stack_trace_result);
    assert!(
        stack_trace_result.is_error == Some(true)
            || stack_trace_content.to_lowercase().contains("error")
            || stack_trace_content.contains("No stack frames found"),
        "stack_trace with invalid thread_id should return an error or empty frames, got: {}",
        stack_trace_content
    );

    let scopes_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Scopes);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "frame_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(999999)),
                );
                map
            });
            params
        })
        .await?;

    let scopes_content = extract_text_content(&scopes_result);
    assert!(
        scopes_result.is_error == Some(true)
            || scopes_content.to_lowercase().contains("error")
            || scopes_content.contains("Scopes for frame"),
        "scopes with invalid frame_id should return an error or scope data, got: {}",
        scopes_content
    );

    let navigate_result = call_navigate_command(
        &mcp_client,
        dapper_session::NavigationType::StepOver,
        ThreadId(999999),
    )
    .await?;

    let navigate_content = extract_text_content(&navigate_result);
    assert!(
        navigate_result.is_error == Some(true) || navigate_content.to_lowercase().contains("error"),
        "navigate with invalid thread_id should return an error, got: {}",
        navigate_content
    );

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    let threads_content = extract_text_content(&threads_result);
    assert!(
        threads_result.is_error != Some(true),
        "MCP server should still be responsive after error calls, got is_error: {:?}, content: {}",
        threads_result.is_error,
        threads_content
    );
    assert!(
        !threads_content.is_empty(),
        "threads response should not be empty after error calls"
    );

    mcp_client.cancel().await?;
    Ok(())
}
