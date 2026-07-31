// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::create_mcp_client_with_toolset;
use dapper_mcp_server::BuiltinToolset;
use dapper_mcp_server::Toolset;

#[tokio::test]
async fn list_tools() -> anyhow::Result<()> {
    let mcp_client = create_mcp_client(None).await?;

    let tools_result = mcp_client.list_tools(Default::default()).await?;

    assert!(!tools_result.tools.is_empty(), "No tools were returned");

    for tool in &tools_result.tools {
        assert!(!tool.name.is_empty(), "Tool has an empty name");

        assert!(
            tool.description
                .as_ref()
                .is_some_and(|desc| !desc.is_empty()),
            "Tool '{}' is missing a description",
            tool.name
        );
    }

    mcp_client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn list_tools_minimal_toolset() -> anyhow::Result<()> {
    let toolset: Toolset = BuiltinToolset::Minimal.into();
    let mut expected_tools = toolset.to_tool_names();
    let always_available = vec![
        "debug_sessions_command".to_string(),
        "debug_status_command".to_string(),
        "debug_config_command".to_string(),
    ];
    for tool in &always_available {
        if !expected_tools.contains(tool) {
            expected_tools.push(tool.clone());
        }
    }

    let mcp_client = create_mcp_client_with_toolset(None, Some(toolset)).await?;

    let tools_result = mcp_client.list_tools(Default::default()).await?;

    assert_eq!(
        tools_result.tools.len(),
        expected_tools.len(),
        "Minimal toolset should have {} tools (including always-available), found: {}",
        expected_tools.len(),
        tools_result.tools.len()
    );

    for expected_tool in &expected_tools {
        assert!(
            tools_result
                .tools
                .iter()
                .any(|t| t.name == expected_tool.as_str()),
            "Expected tool '{}' not found in minimal toolset",
            expected_tool
        );
    }

    mcp_client.cancel().await?;
    Ok(())
}
