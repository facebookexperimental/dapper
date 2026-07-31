// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::assert_context_contains_session_info;
use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn threads_request() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("mcp-threads")?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;
    let tool_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    // Extract text content for debugging purposes
    let content_text = extract_text_content(&tool_result);

    // Check that the tool call was successful (not an error)
    assert!(
        tool_result.is_error != Some(true),
        "Tool response should indicate success, got is_error: {:?}, content: {}",
        tool_result.is_error,
        content_text
    );

    assert_context_contains_session_info(&content_text)?;

    // Check that we actually got threads (not empty/none found)
    assert!(
        !content_text.is_empty()
            && !content_text.contains("No threads found")
            && !content_text.contains("threads: []"),
        "Should have found threads in stopped debug session, got content: {}",
        content_text
    );

    // Check that the first thread's stack trace is automatically expanded
    let stack_trace_regex =
        regex::Regex::new(r"Stack trace \(frames \d+ - \d+\) for thread \d+:").unwrap();
    assert!(
        stack_trace_regex.is_match(&content_text),
        "First thread's stack trace should be expanded, got content: {}",
        content_text
    );

    // Check for either stack frames or "No stack frames found" message
    let has_stack_info =
        content_text.contains("#0:") || content_text.contains("No stack frames found");
    assert!(
        has_stack_info,
        "Should have stack trace information for first thread, got content: {}",
        content_text
    );

    mcp_client.cancel().await?;
    Ok(())
}
