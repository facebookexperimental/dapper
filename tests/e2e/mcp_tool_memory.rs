// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client_with_toolset;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::parse_frame_id_from_stack_trace_response;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_mcp_server::BuiltinToolset;
use dapper_mcp_server::DebugTool;
use dapper_mcp_server::Toolset;
use regex::Regex;
use rmcp::model::CallToolRequestParams;
use rmcp::serde_json;

#[tokio::test]
async fn read_write_memory() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("mcp-memory")?;
    dap_client.try_consume_pending_responses()?;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let toolset: Toolset = BuiltinToolset::Full.into();
    let mcp_client = create_mcp_client_with_toolset(Some(scope_id), Some(toolset)).await?;

    // Need a frame to evaluate expressions in. lldb-dap interprets a
    // frameless `evaluate` request as a debugger COMMAND (so `(void*)main`
    // would be parsed as `(void*)main` the command, which doesn't exist).
    // With a frame_id, it's evaluated as a C++ expression.
    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;
    let thread_id = parse_thread_id_from_threads_response(&extract_text_content(&threads_result))?;

    let stack_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::StackTrace);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "thread_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(thread_id.0)),
                );
                map
            });
            params
        })
        .await?;
    let frame_id = parse_frame_id_from_stack_trace_response(&extract_text_content(&stack_result))?;

    // `&main` is a global symbol always in scope. The cast to `(void*)`
    // forces lldb to render the result as a pointer literal that we can
    // regex out below. main's address is in the executable's code segment
    // — readable, may or may not be writable depending on how the binary
    // is mapped, so we treat WriteMemory's adapter-level result as
    // "either ok or adapter-error" rather than asserting success.
    let eval_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Evaluate);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "expression".to_string(),
                    serde_json::Value::String("(void*)&main".to_string()),
                );
                map.insert(
                    "frame_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(frame_id.0)),
                );
                map
            });
            params
        })
        .await?;
    let eval_content = extract_text_content(&eval_result);
    assert!(
        eval_result.is_error != Some(true),
        "Evaluate `(void*)&main` should succeed, got: {eval_content}"
    );

    let addr_re = Regex::new(r"0x[0-9a-fA-F]+")?;
    let memory_address = addr_re
        .find(&eval_content)
        .ok_or_else(|| {
            anyhow::anyhow!("Could not find hex address in evaluate response: {eval_content}")
        })?
        .as_str()
        .to_string();

    // --- ReadMemory happy path: read code-segment bytes at &main ---
    let read_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::ReadMemory);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "memory_reference".to_string(),
                    serde_json::Value::String(memory_address.clone()),
                );
                map.insert("count".to_string(), serde_json::Value::Number(16.into()));
                map
            });
            params
        })
        .await?;
    let read_content = extract_text_content(&read_result);
    assert!(
        read_result.is_error != Some(true),
        "readMemory should succeed, got: {read_content}"
    );
    assert!(
        read_content.contains("Memory at"),
        "Response should contain hex dump header, got: {read_content}"
    );
    assert!(
        read_content.contains("16 bytes"),
        "Response should report 16 bytes read, got: {read_content}"
    );

    // --- WriteMemory: parser-success path (well-formed hex). Adapter may
    // accept or reject depending on whether &main is writable in this
    // build; both outcomes prove the parser+adapter wiring works. ---
    let write_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::WriteMemory);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "memory_reference".to_string(),
                    serde_json::Value::String(memory_address.clone()),
                );
                map.insert(
                    "data".to_string(),
                    serde_json::Value::String("48656C6C6F".to_string()),
                );
                map
            });
            params
        })
        .await?;
    let write_content = extract_text_content(&write_result);
    assert!(
        write_content.contains("wrote")
            || write_content.contains("Write completed")
            || write_content.contains("Error writing memory"),
        "writeMemory should produce a structured response (success OR adapter error), got: {write_content}"
    );

    // --- WriteMemory parser-success path with 0x-prefixed hex ---
    let write_0x_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::WriteMemory);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "memory_reference".to_string(),
                    serde_json::Value::String(memory_address.clone()),
                );
                map.insert(
                    "data".to_string(),
                    serde_json::Value::String("0x4142".to_string()),
                );
                map
            });
            params
        })
        .await?;
    let write_0x_content = extract_text_content(&write_0x_result);
    assert!(
        !write_0x_content.contains("invalid hex") && !write_0x_content.contains("hex string must"),
        "0x prefix should pass our hex parser, got: {write_0x_content}"
    );

    // --- WriteMemory parser error: garbage hex ---
    let write_invalid_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::WriteMemory);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "memory_reference".to_string(),
                    serde_json::Value::String(memory_address.clone()),
                );
                map.insert(
                    "data".to_string(),
                    serde_json::Value::String("ZZZZ".to_string()),
                );
                map
            });
            params
        })
        .await?;
    let write_invalid_content = extract_text_content(&write_invalid_result);
    assert!(
        write_invalid_result.is_error == Some(true),
        "writeMemory with invalid hex should report error, got: {write_invalid_content}"
    );
    assert!(
        write_invalid_content.contains("invalid hex"),
        "Error should mention invalid hex, got: {write_invalid_content}"
    );

    // --- WriteMemory parser error: odd-length hex ---
    let write_odd_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::WriteMemory);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "memory_reference".to_string(),
                    serde_json::Value::String(memory_address.clone()),
                );
                map.insert(
                    "data".to_string(),
                    serde_json::Value::String("123".to_string()),
                );
                map
            });
            params
        })
        .await?;
    let write_odd_content = extract_text_content(&write_odd_result);
    assert!(
        write_odd_result.is_error == Some(true),
        "writeMemory with odd-length hex should report error, got: {write_odd_content}"
    );
    assert!(
        write_odd_content.contains("even number"),
        "Error should mention even-number-of-digits, got: {write_odd_content}"
    );

    // --- ReadMemory parameter error: count <= 0 ---
    let read_zero_result = mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::ReadMemory);
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "memory_reference".to_string(),
                    serde_json::Value::String(memory_address),
                );
                map.insert("count".to_string(), serde_json::Value::Number(0.into()));
                map
            });
            params
        })
        .await?;
    let read_zero_content = extract_text_content(&read_zero_result);
    assert!(
        read_zero_result.is_error == Some(true),
        "readMemory with count=0 should report error, got: {read_zero_content}"
    );
    assert!(
        read_zero_content.contains("count must be > 0"),
        "Error should mention count constraint, got: {read_zero_content}"
    );

    mcp_client.cancel().await?;
    Ok(())
}
