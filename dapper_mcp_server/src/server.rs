// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use dapper_session::Port;
use dapper_session::ScopeId;
use rmcp::ServiceExt;
use rmcp::transport::stdio;

use crate::handler::McpHandler;
use crate::toolsets::Toolset;

/// Serve MCP on stdin and stdout
pub async fn serve(
    control_port: Option<Port>,
    scope_id: Option<ScopeId>,
    toolset: Toolset,
) -> anyhow::Result<()> {
    tracing::info!(
        "Starting MCP server with toolset '{}' ({} tool(s))",
        toolset.name,
        toolset.tools.len()
    );

    let service = McpHandler::new(control_port, scope_id, &toolset)
        .serve(stdio())
        .await
        .context("Failed to start serving")?;

    tracing::info!("Server initialized and ready to handle requests");

    service.waiting().await?;
    Ok(())
}
