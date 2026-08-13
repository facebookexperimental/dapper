// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::create_mcp_client_with_json;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn capabilities_plaintext_is_the_prose_summary() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("mcp-capabilities")?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;
    let tool_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Capabilities))
        .await?;

    let content = extract_text_content(&tool_result);
    assert!(
        tool_result.is_error != Some(true),
        "Capabilities should succeed, got is_error: {:?}, content: {}",
        tool_result.is_error,
        content
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&content).is_err(),
        "plaintext should be the prose summary, not the raw adapter blob, got: {content}"
    );
    assert!(
        content.contains("Supported capabilities:") || content.contains("No optional capabilities"),
        "plaintext should be the `format_capabilities` summary, not the \
         not-yet-available sentinel, got: {content}"
    );

    mcp_client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn capabilities_json_carries_the_blob_under_result() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("mcp-capabilities-json")?;

    let mcp_client = create_mcp_client_with_json(Some(scope_id), None).await?;
    let tool_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Capabilities))
        .await?;

    let content = extract_text_content(&tool_result);
    assert!(
        tool_result.is_error != Some(true),
        "Capabilities should succeed, got is_error: {:?}, content: {}",
        tool_result.is_error,
        content
    );

    let parsed: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        anyhow::anyhow!("--json capabilities should parse as JSON ({e}), got: {content}")
    })?;
    let blob = parsed["result"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected a capability object under `result`: {parsed}"))?;
    assert!(
        blob.keys().any(|key| key.starts_with("supports")),
        "expected adapter capability keys, got: {parsed}"
    );

    mcp_client.cancel().await?;
    Ok(())
}
