// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;
use rmcp::serde_json;

#[tokio::test]
async fn error_recovery_after_stop() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("error-recovery")?;
    dap_client.try_consume_pending_responses()?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    assert!(
        threads_result.is_error != Some(true),
        "Threads command should succeed before stop. Got is_error: {:?}, content: {}",
        threads_result.is_error,
        extract_text_content(&threads_result)
    );

    let stop_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Stop))
        .await?;

    assert!(
        stop_result.is_error != Some(true),
        "Stop command should succeed. Got is_error: {:?}, content: {}",
        stop_result.is_error,
        extract_text_content(&stop_result)
    );

    assert_eq!(
        extract_text_content(&stop_result),
        "Dapper proxy server stopped.",
        "stop's plaintext body should render without an envelope"
    );

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let threads_after_stop = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    assert!(
        threads_after_stop.is_error == Some(true),
        "Threads command after stop should return an error. Got is_error: {:?}, content: {}",
        threads_after_stop.is_error,
        extract_text_content(&threads_after_stop)
    );

    let stack_trace_after_stop = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::StackTrace);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "thread_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(1)),
                );
                map
            });
            params
        })
        .await?;

    assert!(
        stack_trace_after_stop.is_error == Some(true),
        "Stack trace command after stop should return an error. Got is_error: {:?}, content: {}",
        stack_trace_after_stop.is_error,
        extract_text_content(&stack_trace_after_stop)
    );

    mcp_client.cancel().await?;
    Ok(())
}
