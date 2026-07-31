// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use anyhow::Result;
use dapper_mcp_server::Toolset;
use dapper_session::ScopeId;
use rmcp::RoleClient;
use rmcp::service::RunningService;
use rmcp::service::ServiceExt;
use rmcp::transport::ConfigureCommandExt;
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

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
