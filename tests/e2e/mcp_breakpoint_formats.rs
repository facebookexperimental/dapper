// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::McpClient;
use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::find_breakpoint_line_by_marker;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_e2e_support::test_source_path;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;
use rmcp::serde_json;

async fn set_breakpoint_and_verify(
    mcp_client: &McpClient,
    source_path: &str,
    expected_line: i64,
    breakpoints_value: serde_json::Value,
    format_description: &str,
) {
    let result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::SetBreakpoints);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "source_path".to_string(),
                    serde_json::Value::String(source_path.to_string()),
                );
                map.insert("breakpoints".to_string(), breakpoints_value);
                map.insert("clear_existing".to_string(), serde_json::Value::Bool(true));
                map
            });
            params
        })
        .await
        .unwrap_or_else(|e| {
            panic!(
                "MCP call with {} format should not return protocol error, got: {}",
                format_description, e
            )
        });

    let content = extract_text_content(&result);
    assert!(
        result.is_error != Some(true),
        "Setting breakpoint with {} format should succeed, got is_error: {:?}, content: {}",
        format_description,
        result.is_error,
        content
    );

    let expected_verified = format!("Verified: Line {}", expected_line);
    assert!(
        content.contains(&expected_verified),
        "Breakpoint with {} format should be verified at line {}, got: {}",
        format_description,
        expected_line,
        content
    );
}

#[tokio::test]
async fn breakpoint_alternative_formats() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("breakpoint-formats")?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    let source_path = test_source_path()?;
    let line_1 = find_breakpoint_line_by_marker(&source_path, "breakpoint default_stop").await?;
    let line_2 = find_breakpoint_line_by_marker(&source_path, "breakpoint secondary_stop").await?;
    let line_3 = find_breakpoint_line_by_marker(&source_path, "breakpoint tertiary_stop").await?;
    let line_4 = find_breakpoint_line_by_marker(&source_path, "breakpoint quaternary_stop").await?;
    let source_path = source_path.to_string_lossy().into_owned();

    set_breakpoint_and_verify(
        &mcp_client,
        &source_path,
        line_1,
        serde_json::json!([{"line": line_1}]),
        "object",
    )
    .await;

    set_breakpoint_and_verify(
        &mcp_client,
        &source_path,
        line_2,
        serde_json::json!([format!("{{\"line\": {}}}", line_2)]),
        "stringified JSON",
    )
    .await;

    set_breakpoint_and_verify(
        &mcp_client,
        &source_path,
        line_3,
        serde_json::json!([line_3]),
        "bare integer",
    )
    .await;

    set_breakpoint_and_verify(
        &mcp_client,
        &source_path,
        line_4,
        serde_json::json!([line_4.to_string()]),
        "string integer",
    )
    .await;

    mcp_client.cancel().await?;
    dap_client.kill()?;
    Ok(())
}
