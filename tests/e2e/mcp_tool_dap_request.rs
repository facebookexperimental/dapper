// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::DapClient;
use dapper_e2e_support::McpClient;
use dapper_e2e_support::create_mcp_client_with_toolset;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use dapper_mcp_server::Toolset;
use rmcp::model::CallToolRequestParams;

fn raw_toolset() -> Toolset {
    Toolset {
        name: "raw".to_string(),
        tools: vec![DebugTool::DapRequest, DebugTool::Stop],
    }
}

async fn setup_raw_mcp_session(test_name: &str) -> anyhow::Result<(DapClient, McpClient)> {
    let (scope_id, dap_client) = setup_stopped_debug_session(test_name)?;

    let mcp_client = create_mcp_client_with_toolset(Some(scope_id), Some(raw_toolset())).await?;

    Ok((dap_client, mcp_client))
}

#[tokio::test]
async fn mcp_dap_request_threads() -> anyhow::Result<()> {
    let (mut dap_client, mcp_client) = setup_raw_mcp_session("mcp-dap-threads").await?;

    let tool_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::DapRequest);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "command".to_string(),
                    serde_json::Value::String("threads".to_string()),
                );
                map
            });
            params
        })
        .await?;

    assert!(
        tool_result.is_error != Some(true),
        "debug_dap_request should succeed, got: {}",
        extract_text_content(&tool_result)
    );

    let content = extract_text_content(&tool_result);
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|e| {
        panic!(
            "dap request output should be valid JSON, got error: {e}, content: {}",
            content
        )
    });

    let threads = parsed
        .get("threads")
        .and_then(|t| t.as_array())
        .expect("threads response should contain a 'threads' array");
    assert!(
        !threads.is_empty(),
        "threads array should not be empty in a stopped session"
    );

    mcp_client.cancel().await?;
    dap_client.kill()?;
    Ok(())
}

#[tokio::test]
async fn mcp_dap_request_stacktrace() -> anyhow::Result<()> {
    let (mut dap_client, mcp_client) = setup_raw_mcp_session("mcp-dap-stacktrace").await?;

    let threads_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::DapRequest);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "command".to_string(),
                    serde_json::Value::String("threads".to_string()),
                );
                map
            });
            params
        })
        .await?;

    assert!(
        threads_result.is_error != Some(true),
        "intermediate threads call should succeed, got: {}",
        extract_text_content(&threads_result)
    );

    let threads_content = extract_text_content(&threads_result);
    let threads_parsed: serde_json::Value = serde_json::from_str(&threads_content)?;
    let thread_id = threads_parsed["threads"][0]["id"]
        .as_i64()
        .expect("thread id");

    let tool_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::DapRequest);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "command".to_string(),
                    serde_json::Value::String("stackTrace".to_string()),
                );
                map.insert(
                    "arguments".to_string(),
                    serde_json::json!({"threadId": thread_id}),
                );
                map
            });
            params
        })
        .await?;

    assert!(
        tool_result.is_error != Some(true),
        "debug_dap_request stackTrace should succeed, got: {}",
        extract_text_content(&tool_result)
    );

    let content = extract_text_content(&tool_result);
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|e| {
        panic!(
            "stackTrace output should be valid JSON, got error: {e}, content: {}",
            content
        )
    });

    let frames = parsed
        .get("stackFrames")
        .and_then(|f| f.as_array())
        .expect("stackTrace response should contain 'stackFrames' array");
    assert!(
        !frames.is_empty(),
        "stackFrames should not be empty for a stopped thread"
    );

    mcp_client.cancel().await?;
    dap_client.kill()?;
    Ok(())
}
