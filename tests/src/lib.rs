// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use anyhow::Result;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_mcp_server::DebugTool;
use dapper_mcp_server::Toolset;
use dapper_session::NavigationType;
use dapper_session::ScopeId;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::service::RunningService;
use rmcp::service::ServiceExt;
use rmcp::transport::ConfigureCommandExt;
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

pub mod dap_client;
mod harness;

pub use harness::AdapterLog;
pub use harness::DapClient;
pub use harness::DebugCliOutput;
pub use harness::dapper_command;
pub use harness::generate_test_scope_id;
pub use harness::run_debug_command;
pub use harness::run_debug_command_with_binary;
pub use harness::setup_stopped_fake_debug_session;

pub const REVERSE_DEBUG_GATING_MSG: &str =
    "does not advertise the DAP `supportsStepBack` capability";

/// An initialized MCP client connected to a Dapper child process.
pub type McpClient = RunningService<RoleClient, ()>;

/// Starts Dapper's MCP server with the default toolset.
pub async fn create_mcp_client(scope_id: Option<ScopeId>) -> Result<McpClient> {
    create_mcp_client_with_toolset(scope_id, None).await
}

/// Starts Dapper's MCP server with an optional scope and toolset.
pub async fn create_mcp_client_with_toolset(
    scope_id: Option<ScopeId>,
    toolset: Option<Toolset>,
) -> Result<McpClient> {
    let dapper_path = std::env::var("DAPPER_TEST_EXECUTABLE")
        .context("DAPPER_TEST_EXECUTABLE must name the Dapper binary")?;
    create_mcp_client_with_binary(&dapper_path, scope_id, toolset).await
}

/// Starts an MCP server using an explicit Dapper executable.
pub async fn create_mcp_client_with_binary(
    dapper_path: &str,
    scope_id: Option<ScopeId>,
    toolset: Option<Toolset>,
) -> Result<McpClient> {
    let dapper_path = dapper_path.to_owned();
    let service = ()
        .serve(TokioChildProcess::new(
            Command::new(dapper_path).configure(|command| {
                command.arg("mcp");
                if let Some(scope) = &scope_id {
                    command.arg("--scope-id").arg(scope.as_str());
                }
                if let Some(toolset) = toolset {
                    command.arg("--toolset").arg(toolset.name);
                }

                command.env(
                    "DAPPER_SESSIONS_DIR",
                    dapper_session::get_user_temp_dir().join("test_sessions"),
                );
                command.env("DAPPER_DISABLE_SCUBA", "1");
            }),
        )?)
        .await?;

    Ok(service)
}

/// Extracts all text blocks from an MCP tool result.
pub fn extract_text_content(tool_result: &CallToolResult) -> String {
    tool_result
        .content
        .iter()
        .filter_map(|content| match content {
            rmcp::model::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Parses the first thread ID from Dapper's rendered threads response.
pub fn parse_thread_id_from_threads_response(threads_content: &str) -> Result<ThreadId> {
    threads_content
        .lines()
        .find(|line| line.trim().starts_with("Thread "))
        .and_then(|line| {
            line.split(':')
                .next()?
                .trim()
                .strip_prefix("Thread ")?
                .parse::<ThreadId>()
                .ok()
        })
        .with_context(|| format!("could not parse thread ID from: {threads_content}"))
}

/// Calls the MCP navigation tool with a navigation type and thread ID.
pub async fn call_navigate_command(
    mcp_client: &McpClient,
    navigation_type: NavigationType,
    thread_id: ThreadId,
) -> Result<CallToolResult> {
    mcp_client
        .call_tool({
            let mut params = CallToolRequestParams::new(DebugTool::Navigate);
            params.arguments = Some({
                let mut arguments = serde_json::Map::new();
                arguments.insert(
                    "navigation_type".to_string(),
                    serde_json::Value::String(navigation_type.to_string()),
                );
                arguments.insert(
                    "thread_id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(thread_id.0)),
                );
                arguments
            });
            params
        })
        .await
        .context("failed to call the MCP navigate tool")
}

/// Returns all thread IDs reported by the debug CLI for a scope.
pub async fn get_thread_ids(scope_id: &ScopeId) -> Result<Vec<i64>> {
    let result = run_debug_command(Some(scope_id.clone()), &["threads", "--json"]).await?;
    anyhow::ensure!(
        result.success,
        "threads --json command failed, stderr: {}",
        result.stderr
    );
    let parsed: serde_json::Value = serde_json::from_str(&result.stdout)
        .with_context(|| format!("threads --json output is not valid JSON: {}", result.stdout))?;
    let threads = parsed["result"]["threads"]
        .as_array()
        .context("threads response should contain a 'threads' array")?;
    anyhow::ensure!(!threads.is_empty(), "threads array should not be empty");
    threads
        .iter()
        .map(|thread| {
            thread["id"]
                .as_i64()
                .context("thread should have a numeric 'id'")
        })
        .collect()
}
