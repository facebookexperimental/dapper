// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::REVERSE_DEBUG_GATING_MSG;
use dapper_e2e_support::call_navigate_command;
use dapper_e2e_support::create_mcp_client;
use dapper_e2e_support::extract_text_content;
use dapper_e2e_support::parse_thread_id_from_threads_response;
use dapper_e2e_support::setup_stopped_fake_debug_session;
use dapper_mcp_server::DebugTool;
use dapper_session::NavigationType;
use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn navigate_reverse_succeeds_when_capability_advertised() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_fake_debug_session(
        "mcp-reverse-nav-success",
        &["--supports-step-back", "true"],
    )?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;
    let threads_content = extract_text_content(&threads_result);
    let thread_id = parse_thread_id_from_threads_response(&threads_content)?;

    let step_back_result =
        call_navigate_command(&mcp_client, NavigationType::StepBack, thread_id).await?;
    let step_back_content = extract_text_content(&step_back_result);
    assert!(
        step_back_result.is_error != Some(true),
        "step_back should succeed when supportsStepBack is true, got is_error={:?}, content={}",
        step_back_result.is_error,
        step_back_content,
    );

    let reverse_continue_result =
        call_navigate_command(&mcp_client, NavigationType::ReverseContinue, thread_id).await?;
    let reverse_continue_content = extract_text_content(&reverse_continue_result);
    assert!(
        reverse_continue_result.is_error != Some(true),
        "reverse_continue should succeed when supportsStepBack is true, got is_error={:?}, content={}",
        reverse_continue_result.is_error,
        reverse_continue_content,
    );

    mcp_client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn navigate_reverse_gated_when_capability_missing() -> anyhow::Result<()> {
    // Spawn fake without step-back support; if the proxy ever forwarded a
    // reverse request, --fail-on-reverse would terminate the fake (exit 2)
    // and the follow-up debug_threads_command would fail with a broken pipe.
    let (scope_id, _dap_client) = setup_stopped_fake_debug_session(
        "mcp-reverse-nav-gated",
        &["--supports-step-back", "false", "--fail-on-reverse"],
    )?;

    let mcp_client = create_mcp_client(Some(scope_id)).await?;

    let threads_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;
    let threads_content = extract_text_content(&threads_result);
    let thread_id = parse_thread_id_from_threads_response(&threads_content)?;

    let step_back_result =
        call_navigate_command(&mcp_client, NavigationType::StepBack, thread_id).await?;
    let step_back_content = extract_text_content(&step_back_result);
    assert!(
        step_back_result.is_error == Some(true),
        "step_back should be gated when supportsStepBack is false, got is_error={:?}, content={}",
        step_back_result.is_error,
        step_back_content,
    );
    assert!(
        step_back_content.contains(REVERSE_DEBUG_GATING_MSG),
        "expected gating error message, got: {}",
        step_back_content,
    );

    let reverse_continue_result =
        call_navigate_command(&mcp_client, NavigationType::ReverseContinue, thread_id).await?;
    let reverse_continue_content = extract_text_content(&reverse_continue_result);
    assert!(
        reverse_continue_result.is_error == Some(true),
        "reverse_continue should be gated when supportsStepBack is false, got is_error={:?}, content={}",
        reverse_continue_result.is_error,
        reverse_continue_content,
    );
    assert!(
        reverse_continue_content.contains(REVERSE_DEBUG_GATING_MSG),
        "expected gating error message, got: {}",
        reverse_continue_content,
    );

    // Liveness check: issue a harmless follow-up command. If the fake exited
    // (i.e. a reverse request reached it despite --fail-on-reverse, which
    // triggers eprintln + exit(2)), the proxy pipe would be broken and
    // threads would fail. Success here proves the gate fired upstream and
    // the fake never saw the reverse request.
    let liveness_result = mcp_client
        .call_tool(CallToolRequestParams::new(DebugTool::Threads))
        .await?;
    let liveness_content = extract_text_content(&liveness_result);
    assert!(
        liveness_result.is_error != Some(true),
        "follow-up threads call should succeed if the fake adapter is still alive (proves gating worked), got is_error={:?}, content={}",
        liveness_result.is_error,
        liveness_content,
    );

    mcp_client.cancel().await?;
    Ok(())
}
