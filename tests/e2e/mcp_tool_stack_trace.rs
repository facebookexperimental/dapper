// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::assert_context_contains_session_info;
use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn stack_trace_request() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("mcp-stack-trace")?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    // First, get the list of available threads to find a valid thread ID
    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    // Extract the thread ID from the threads response using helper functions
    let threads_content = extract_text_content(&threads_result);
    let thread_id = parse_thread_id_from_threads_response(&threads_content)?;

    // Now call stack trace with the valid thread ID
    let tool_result = mcp_client
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

    // Extract text content for debugging purposes
    let content_text = extract_text_content(&tool_result);

    // Check that the tool call was successful (not an error)
    assert!(
        tool_result.is_error != Some(true),
        "Tool response should indicate success for valid thread ID, got is_error: {:?}, content: {}",
        tool_result.is_error,
        content_text
    );

    assert_context_contains_session_info(&content_text)?;

    // Check that we actually got stack frames (not empty/none found)
    assert!(!content_text.contains("No stack frames found"));
    assert!(!(content_text.contains("stackFrames") && content_text.contains("[]")));

    // Check that the topmost frame's scopes are automatically expanded
    assert!(
        content_text.contains("Scopes for frame"),
        "Topmost frame's scopes should be expanded, got content: {}",
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

    mcp_client.cancel().await?;
    Ok(())
}
