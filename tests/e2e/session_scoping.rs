// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::start_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn session_scoping() -> anyhow::Result<()> {
    let scope_a = generate_test_scope_id("session-scoping-a");
    let scope_b = generate_test_scope_id("session-scoping-b");

    let _dap_client_a = start_stopped_debug_session(scope_a.clone())?;
    let _dap_client_b = start_stopped_debug_session(scope_b.clone())?;

    let mcp_client_a = create_mcp_client(Some(scope_a.clone())).await?;

    let threads_result_a = mcp_client_a
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    assert!(
        threads_result_a.is_error != Some(true),
        "scope_a threads should succeed, got is_error: {:?}, content: {}",
        threads_result_a.is_error,
        extract_text_content(&threads_result_a)
    );

    let mcp_client_b = create_mcp_client(Some(scope_b.clone())).await?;

    let threads_result_b = mcp_client_b
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    assert!(
        threads_result_b.is_error != Some(true),
        "scope_b threads should succeed, got is_error: {:?}, content: {}",
        threads_result_b.is_error,
        extract_text_content(&threads_result_b)
    );

    let stop_result_a = mcp_client_a
        .call_tool(CallToolRequestParams::new(DebugTool::Stop))
        .await?;

    assert!(
        stop_result_a.is_error != Some(true),
        "scope_a stop should succeed, got is_error: {:?}, content: {}",
        stop_result_a.is_error,
        extract_text_content(&stop_result_a)
    );

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let threads_after_stop_a = mcp_client_a
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    assert!(
        threads_after_stop_a.is_error == Some(true),
        "scope_a threads after stop should fail, got is_error: {:?}, content: {}",
        threads_after_stop_a.is_error,
        extract_text_content(&threads_after_stop_a)
    );

    let threads_still_alive_b = mcp_client_b
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    assert!(
        threads_still_alive_b.is_error != Some(true),
        "scope_b threads should still succeed after scope_a stopped, got is_error: {:?}, content: {}",
        threads_still_alive_b.is_error,
        extract_text_content(&threads_still_alive_b)
    );

    mcp_client_a.cancel().await?;
    mcp_client_b.cancel().await?;

    Ok(())
}
