// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::data_types::FrameId;
use dapper_e2e_support::assert_context_contains_session_info;
use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::parse_frame_id_from_stack_trace_response;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn scopes_request() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("mcp-scopes")?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    // First, get the list of available threads to find a valid thread ID
    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    // Extract the thread ID from the threads response using helper functions
    let threads_content = extract_text_content(&threads_result);
    let thread_id = parse_thread_id_from_threads_response(&threads_content)?;

    // Get stack trace to find a valid frame ID
    let stack_trace_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::StackTrace);
            params.arguments = Some({
                let mut map = rmcp::serde_json::Map::new();
                map.insert(
                    "thread_id".to_string(),
                    rmcp::serde_json::Value::Number(rmcp::serde_json::Number::from(thread_id.0)),
                );
                map
            });
            params
        })
        .await?;

    let stack_trace_content = extract_text_content(&stack_trace_result);

    // For Python, stack trace might be empty at entry point, so we handle that case
    let frame_id = if stack_trace_content.contains("No stack frames found") {
        // For Python at entry point, we can try frame ID 0 as a fallback
        FrameId(0)
    } else {
        parse_frame_id_from_stack_trace_response(&stack_trace_content)?
    };

    // Now call scopes command with the valid frame ID
    let tool_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Scopes);
            params.arguments = Some({
                let mut map = rmcp::serde_json::Map::new();
                map.insert(
                    "frame_id".to_string(),
                    rmcp::serde_json::Value::Number(rmcp::serde_json::Number::from(frame_id.0)),
                );
                map
            });
            params
        })
        .await?;

    // Extract text content for debugging purposes
    let content_text = extract_text_content(&tool_result);

    // Check that the tool call was successful (not an error)
    // Note: For Python at entry point, scopes might be empty but should still succeed
    assert!(
        tool_result.is_error != Some(true),
        "Tool response should indicate success for valid frame ID, got is_error: {:?}, content: {}",
        tool_result.is_error,
        content_text
    );

    assert_context_contains_session_info(&content_text)?;

    // Check that we got some scope information (not completely empty/none found)
    // Note: For Python, scopes might be minimal at entry but should contain some structure
    assert!(
        !content_text.is_empty()
            && !content_text.contains("No scopes found")
            && !content_text.contains("scopes: []"),
        "Should have found some scopes in stopped debug session, got content: {}",
        content_text
    );

    // Check that if we have a Locals scope, it should be expanded with variables
    if content_text.to_lowercase().contains("local") {
        let has_variables =
            content_text.contains("    ") || content_text.contains("(no variables)");
        assert!(
            has_variables,
            "Locals scope should be expanded with variables or show '(no variables)', got content: {}",
            content_text
        );
    }

    // Note: LLDB's scopes command is more permissive and doesn't return errors for invalid frame IDs
    // It returns a successful response (possibly empty) instead of an error
    // This is different from thread-based commands that do return errors for invalid thread IDs

    mcp_client.cancel().await?;
    Ok(())
}
