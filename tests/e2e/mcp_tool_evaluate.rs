// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::RealAdapterProfile;
use dapper_e2e_support::create_mcp_client_with_json;
use dapper_e2e_support::create_mcp_client_with_toolset;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::parse_frame_id_from_stack_trace_response;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::real_adapter_profile;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::BuiltinToolset;
use dapper_mcp_server::DebugTool;
use dapper_mcp_server::Toolset;
use rmcp::model::CallToolRequestParams;

/// An expression the adapter can evaluate, paired with a substring its result
/// must contain.
fn eval_probe(profile: RealAdapterProfile) -> (&'static str, &'static str) {
    match profile {
        RealAdapterProfile::Lldb => ("help", "Debugger commands:"),
        RealAdapterProfile::Debugpy => ("1 + 1", "2"),
    }
}

fn evaluate_params(expression: &str) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new(DebugTool::Evaluate);
    params.arguments = Some(rmcp::serde_json::Map::from_iter([(
        "expression".to_string(),
        rmcp::serde_json::Value::String(expression.to_string()),
    )]));
    params
}

#[tokio::test]
async fn evaluate_expression() -> anyhow::Result<()> {
    let adapter_profile = real_adapter_profile()?;
    let (scope_id, mut dap_client) = setup_stopped_debug_session("mcp-evaluate")?;
    dap_client.try_consume_pending_responses()?;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let toolset: Toolset = BuiltinToolset::Full.into();
    let mcp_client = create_mcp_client_with_toolset(Some(scope_id), Some(toolset)).await?;

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    let threads_content = extract_text_content(&threads_result);
    assert!(
        !threads_content.is_empty(),
        "Threads response should not be empty"
    );

    let (expression, expected_substr) = eval_probe(adapter_profile);

    let eval_result = mcp_client.call_tool(evaluate_params(expression)).await?;

    let eval_content = extract_text_content(&eval_result);

    assert!(
        eval_result.is_error != Some(true),
        "Evaluate '{expression}' should succeed, got is_error: {:?}, content: {}",
        eval_result.is_error,
        eval_content
    );

    assert!(
        eval_content.contains(expected_substr),
        "Evaluate '{expression}' should contain '{expected_substr}', got content: {}",
        eval_content
    );

    let error_eval_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Evaluate);
            params.arguments = Some({
                let mut map = rmcp::serde_json::Map::new();
                map.insert(
                    "expression".to_string(),
                    rmcp::serde_json::Value::String("undefined_var_xyz".to_string()),
                );
                map
            });
            params
        })
        .await?;

    let error_eval_content = extract_text_content(&error_eval_result);

    let indicates_error = error_eval_result.is_error == Some(true)
        || error_eval_content.to_lowercase().contains("error")
        || error_eval_content.to_lowercase().contains("not found")
        || error_eval_content.to_lowercase().contains("unknown")
        || error_eval_content.to_lowercase().contains("undefined");

    assert!(
        indicates_error,
        "Evaluate 'undefined_var_xyz' should indicate an error, got is_error: {:?}, content: {}",
        error_eval_result.is_error, error_eval_content
    );

    mcp_client.cancel().await?;
    Ok(())
}

/// `evaluate` and `stop` both return a bare `String`, so JSON has to supply the
/// `{"result": ...}` envelope around it. `stop` runs last: it ends the session.
#[tokio::test]
async fn evaluate_and_stop_json_output() -> anyhow::Result<()> {
    let adapter_profile = real_adapter_profile()?;
    let (scope_id, mut dap_client) = setup_stopped_debug_session("mcp-evaluate-json")?;
    dap_client.try_consume_pending_responses()?;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let toolset: Toolset = BuiltinToolset::Full.into();
    let mcp_client = create_mcp_client_with_json(Some(scope_id), Some(toolset)).await?;

    let (expression, expected_substr) = eval_probe(adapter_profile);

    let eval_result = mcp_client.call_tool(evaluate_params(expression)).await?;

    let eval_content = extract_text_content(&eval_result);
    assert!(
        eval_result.is_error != Some(true),
        "Evaluate '{expression}' should succeed, got is_error: {:?}, content: {}",
        eval_result.is_error,
        eval_content
    );

    let parsed: serde_json::Value = serde_json::from_str(&eval_content).map_err(|e| {
        anyhow::anyhow!("--json output should parse as JSON ({e}), got: {eval_content}")
    })?;
    let rendered = parsed["result"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("eval result must be a JSON string, got: {parsed}"))?;
    assert!(
        rendered.contains(expected_substr),
        "Evaluate '{expression}' should contain '{expected_substr}' under `result`, got: {parsed}"
    );

    let stop_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Stop))
        .await?;
    let stop_content = extract_text_content(&stop_result);
    assert!(
        stop_result.is_error != Some(true),
        "Stop should succeed, got is_error: {:?}, content: {stop_content}",
        stop_result.is_error
    );
    let stop_parsed: serde_json::Value = serde_json::from_str(&stop_content).map_err(|e| {
        anyhow::anyhow!("--json stop output should parse as JSON ({e}), got: {stop_content}")
    })?;
    assert!(
        stop_parsed["result"].is_string(),
        "stop's acknowledgement must ride in `result`, got: {stop_parsed}"
    );

    mcp_client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn evaluate_expression_with_frame_id() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("mcp-evaluate-frame")?;
    dap_client.try_consume_pending_responses()?;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let toolset: Toolset = BuiltinToolset::Full.into();
    let mcp_client = create_mcp_client_with_toolset(Some(scope_id), Some(toolset)).await?;

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;

    let threads_content = extract_text_content(&threads_result);
    let thread_id = parse_thread_id_from_threads_response(&threads_content)?;

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
    let frame_id = parse_frame_id_from_stack_trace_response(&stack_trace_content)?;

    let eval_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Evaluate);
            params.arguments = Some({
                let mut map = rmcp::serde_json::Map::new();
                map.insert(
                    "expression".to_string(),
                    rmcp::serde_json::Value::String("1 + 1".to_string()),
                );
                map.insert(
                    "frame_id".to_string(),
                    rmcp::serde_json::Value::Number(rmcp::serde_json::Number::from(frame_id.0)),
                );
                map
            });
            params
        })
        .await?;

    let eval_content = extract_text_content(&eval_result);

    assert!(
        eval_result.is_error != Some(true),
        "Evaluate with valid frame_id should succeed, got is_error: {:?}, content: {}",
        eval_result.is_error,
        eval_content
    );

    assert!(
        eval_content.contains("2"),
        "Evaluate '1 + 1' with frame_id should contain '2', got content: {}",
        eval_content
    );

    mcp_client.cancel().await?;
    Ok(())
}
