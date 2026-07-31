// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;

/// Tests that the proxy correctly filters responses so that the client only
/// receives responses for requests it sent, not responses for MCP tool requests.
#[tokio::test]
async fn client_only_receives_own_responses() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("proxy-filtering")?;

    // Consume any pending responses from the launch/initialization sequence
    // Python and LLDB have different response timing, so we drain the queue first
    dap_client.try_consume_pending_responses()?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    // Test scenario:
    // 1. MCP sends a threads request (seq assigned by backend, not tracked by client)
    // 2. Client sends a threads request (seq 100, tracked by client)
    // 3. Client should only receive response for seq 100, not the MCP request's response

    // First, send a threads request from the MCP tool
    // The response to this should NOT be forwarded to the DAP client
    let mcp_tool_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    // Verify MCP request succeeded
    assert!(
        mcp_tool_result.is_error != Some(true),
        "MCP threads command should succeed"
    );

    // Now send a threads request from the DAP client with seq 100
    // This SHOULD get a response because the client sent it
    let threads_req = r#"{"type": "request", "command": "threads", "seq": 100}"#.to_string();
    dap_client.send(threads_req)?;

    // Read the response - it should be for the client's request (seq 100)
    // and NOT for the MCP request
    let response = dap_client.read_response()?;

    // Verify this is a response to the client's threads request
    assert_eq!(
        response.request_seq, 100,
        "Client should receive response for its own request (seq 100), but got seq {}",
        response.request_seq
    );

    assert_eq!(
        response.command, "threads",
        "Response command should be 'threads', got '{}'",
        response.command
    );

    assert!(response.success, "Response should indicate success");

    // Verify we got threads in the response
    if let Some(body) = &response.body {
        let threads = body.get("threads");
        assert!(threads.is_some(), "Response should contain threads array");

        // Check that threads is an array with at least one element
        if let Some(rmcp::serde_json::Value::Array(thread_list)) = threads {
            assert!(
                !thread_list.is_empty(),
                "Should have at least one thread in a stopped debug session"
            );
        } else {
            panic!("threads field should be an array");
        }
    } else {
        panic!("Response should have a body");
    }

    // Clean up
    mcp_client.cancel().await?;

    Ok(())
}
