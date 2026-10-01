// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use rmcp::ServiceExt;
use rmcp::transport::stdio;

use crate::handler::McpHandler;
use crate::handler::McpServerEnv;
use crate::toolsets::Toolset;

/// Serve MCP on stdin and stdout
pub async fn serve(env: McpServerEnv, toolset: Toolset) -> anyhow::Result<()> {
    let handler = McpHandler::new(env, &toolset);
    tracing::info!(
        "Starting MCP server with toolset '{}' ({} tool(s))",
        toolset.name,
        handler.tool_count()
    );

    let service = handler
        .serve(stdio())
        .await
        .context("Failed to start serving")?;

    tracing::info!("Server initialized and ready to handle requests");

    service.waiting().await?;
    Ok(())
}
