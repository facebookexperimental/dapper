// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::McpClient;
use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::start_stopped_debug_session;
use dapper_mcp_server::DebugTool;
use dapper_session::SessionId;
use rmcp::model::CallToolRequestParams;
use rmcp::serde_json;

async fn call_threads(
    mcp: &McpClient,
    session_id: Option<&SessionId>,
) -> anyhow::Result<rmcp::model::CallToolResult> {
    mcp.call_tool({
        let mut params = CallToolRequestParams::new(DebugTool::Threads);
        if let Some(id) = session_id {
            params.arguments = Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "session_id".into(),
                    serde_json::Value::String(id.to_string()),
                );
                map
            });
        }
        params
    })
    .await
    .map_err(Into::into)
}

fn parse_session_id_from_response(content: &str) -> Option<SessionId> {
    content
        .lines()
        .find(|line| line.starts_with("Session: "))
        .and_then(|line| {
            line.strip_prefix("Session: ")?
                .split(" | ")
                .next()
                .map(|s| s.into())
        })
}

async fn list_session_ids(scope: &dapper_session::ScopeId) -> anyhow::Result<Vec<SessionId>> {
    let output = run_debug_command(Some(scope.clone()), &["sessions"]).await?;
    assert!(
        output.success,
        "dapper debug sessions failed: {}",
        output.stderr
    );

    let ids: Vec<SessionId> = output
        .stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            Some(SessionId::new(
                line.strip_prefix("Session ")?.strip_suffix(':')?,
            ))
        })
        .collect();
    Ok(ids)
}

#[tokio::test]
async fn mcp_remembers_explicit_session_id() -> anyhow::Result<()> {
    let scope = generate_test_scope_id("explicit-session");

    let _dap_a = start_stopped_debug_session(scope.clone())?;
    let _dap_b = start_stopped_debug_session(scope.clone())?;

    let session_ids = list_session_ids(&scope).await?;
    assert_eq!(session_ids.len(), 2, "expected 2 active sessions");
    let target_id = session_ids[0].clone();

    let mcp = create_mcp_client(Some(scope.clone())).await?;

    let result = call_threads(&mcp, Some(&target_id)).await?;
    let content = extract_text_content(&result);
    let resolved =
        parse_session_id_from_response(&content).expect("response should contain session context");
    assert_eq!(
        resolved, target_id,
        "explicit session_id should target the requested session"
    );

    let result = call_threads(&mcp, None).await?;
    let content = extract_text_content(&result);
    let resolved =
        parse_session_id_from_response(&content).expect("response should contain session context");
    assert_eq!(
        resolved, target_id,
        "without session_id should use remembered session"
    );

    mcp.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn mcp_remembered_session_not_displaced_by_newer() -> anyhow::Result<()> {
    let scope = generate_test_scope_id("session-sticky");

    let _dap_a = start_stopped_debug_session(scope.clone())?;

    let mcp = create_mcp_client(Some(scope.clone())).await?;

    let result = call_threads(&mcp, None).await?;
    let content = extract_text_content(&result);
    let initial_id =
        parse_session_id_from_response(&content).expect("response should contain session context");

    let _dap_b = start_stopped_debug_session(scope.clone())?;

    let session_ids = list_session_ids(&scope).await?;
    assert_eq!(session_ids.len(), 2, "expected 2 active sessions");
    assert_eq!(
        session_ids[0], initial_id,
        "expected initial session to be first"
    );

    let result = call_threads(&mcp, None).await?;
    let content = extract_text_content(&result);
    let resolved =
        parse_session_id_from_response(&content).expect("response should contain session context");
    assert_eq!(
        resolved, initial_id,
        "newer session should not displace remembered session"
    );

    mcp.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn mcp_falls_back_when_remembered_session_dies() -> anyhow::Result<()> {
    let scope = generate_test_scope_id("session-fallback");

    let mut dap_a = start_stopped_debug_session(scope.clone())?;

    let mcp = create_mcp_client(Some(scope.clone())).await?;

    let result = call_threads(&mcp, None).await?;
    let content = extract_text_content(&result);
    let initial_id =
        parse_session_id_from_response(&content).expect("response should contain session context");

    let mut dap_b = start_stopped_debug_session(scope.clone())?;

    let session_ids = list_session_ids(&scope).await?;
    let other_id = session_ids
        .iter()
        .find(|id| **id != initial_id)
        .expect("should find the other session");

    dap_a.kill()?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let result = call_threads(&mcp, None).await?;
    let content = extract_text_content(&result);
    assert!(
        result.is_error != Some(true),
        "threads should succeed after fallback, content: {}",
        content
    );
    let resolved =
        parse_session_id_from_response(&content).expect("response should contain session context");
    assert_eq!(
        resolved, *other_id,
        "should fall back to the surviving session"
    );

    dap_b.kill()?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let result = call_threads(&mcp, None).await?;
    assert!(
        result.is_error == Some(true),
        "threads should fail with no active sessions, content: {}",
        extract_text_content(&result)
    );

    mcp.cancel().await?;
    Ok(())
}
