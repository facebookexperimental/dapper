// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::find_breakpoint_line_by_marker;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_e2e_support::test_source_path;
use dapper_mcp_server::DebugTool;
use rmcp::model::CallToolRequestParams;
use rmcp::serde_json;

#[tokio::test]
async fn breakpoint_lifecycle() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("breakpoint-lifecycle")?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    let threads_content = extract_text_content(&threads_result);
    assert!(
        threads_result.is_error != Some(true),
        "Threads should succeed, got is_error: {:?}, content: {}",
        threads_result.is_error,
        threads_content
    );

    let source_path = test_source_path()?;
    let breakpoint_line_1 =
        find_breakpoint_line_by_marker(&source_path, "breakpoint default_stop").await?;
    let breakpoint_line_2 =
        find_breakpoint_line_by_marker(&source_path, "breakpoint secondary_stop").await?;
    let source_path = source_path.to_string_lossy().into_owned();

    let set_bp_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::SetBreakpoints);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "source_path".to_string(),
                    serde_json::Value::String(source_path.clone()),
                );
                map.insert(
                    "breakpoints".to_string(),
                    serde_json::json!([{"line": breakpoint_line_1}]),
                );
                map
            });
            params
        })
        .await?;

    let set_bp_content = extract_text_content(&set_bp_result);
    assert!(
        set_bp_result.is_error != Some(true),
        "Setting first breakpoint should succeed, got is_error: {:?}, content: {}",
        set_bp_result.is_error,
        set_bp_content
    );

    let append_bp_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::SetBreakpoints);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "source_path".to_string(),
                    serde_json::Value::String(source_path.clone()),
                );
                map.insert(
                    "breakpoints".to_string(),
                    serde_json::json!([{"line": breakpoint_line_2}]),
                );
                map.insert("clear_existing".to_string(), serde_json::Value::Bool(false));
                map
            });
            params
        })
        .await?;

    let append_bp_content = extract_text_content(&append_bp_result);
    assert!(
        append_bp_result.is_error != Some(true),
        "Appending breakpoint should succeed, got is_error: {:?}, content: {}",
        append_bp_result.is_error,
        append_bp_content
    );
    assert!(
        append_bp_content.contains("Appended"),
        "Response should mention 'Appended' when adding breakpoints with clear_existing=false, got: {}",
        append_bp_content
    );

    let clear_bp_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::SetBreakpoints);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "source_path".to_string(),
                    serde_json::Value::String(source_path.clone()),
                );
                map.insert("breakpoints".to_string(), serde_json::json!([]));
                map.insert("clear_existing".to_string(), serde_json::Value::Bool(true));
                map
            });
            params
        })
        .await?;

    let clear_bp_content = extract_text_content(&clear_bp_result);
    assert!(
        clear_bp_result.is_error != Some(true),
        "Clearing breakpoints should succeed, got is_error: {:?}, content: {}",
        clear_bp_result.is_error,
        clear_bp_content
    );

    mcp_client.cancel().await?;
    dap_client.kill()?;
    Ok(())
}
