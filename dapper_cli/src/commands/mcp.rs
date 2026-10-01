// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use clap::Parser;
use clap::builder::TypedValueParser;
use dapper_config::DapperConfig;
use dapper_mcp_server::BuiltinToolset;
use dapper_mcp_server::DebugTool;
use dapper_mcp_server::McpServerEnv;
use dapper_mcp_server::Toolset;
use dapper_session::SessionStore;
use strum::VariantNames;

use crate::commands::SessionTarget;

fn debug_tool_parser() -> impl clap::builder::TypedValueParser {
    clap::builder::PossibleValuesParser::new(DebugTool::VARIANTS)
        .try_map(|s| s.parse::<DebugTool>())
}

/// Start the MCP server on stdin/stdout
#[derive(Parser)]
pub struct Mcp {
    #[command(flatten)]
    target: SessionTarget,
    /// Builtin toolset to use
    #[arg(long, value_enum, default_value_t)]
    toolset: BuiltinToolset,
    /// Explicitly enable specific tools (overrides toolset)
    #[arg(long = "enable-tool", value_name = "TOOL", value_parser = debug_tool_parser())]
    enable_tools: Vec<DebugTool>,
}

impl Mcp {
    pub async fn run(self, config: DapperConfig) -> anyhow::Result<()> {
        let toolset = if !self.enable_tools.is_empty() {
            tracing::info!(
                "Using tools from CLI --enable-tool flags: {:?}",
                self.enable_tools
            );
            Toolset::custom("custom".to_string(), self.enable_tools)
        } else {
            self.toolset.into()
        };

        let env = McpServerEnv {
            control_port: self.target.control_port,
            scope_id: self.target.scope_id,
            sessions: SessionStore::default_location()?,
            config,
        };
        dapper_mcp_server::serve(env, toolset).await
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn control_port_parses_and_rejects_zero() {
        let mcp = Mcp::try_parse_from(["mcp", "--control-port", "8080"]).unwrap();
        assert_eq!(mcp.target.control_port.map(|p| p.get()), Some(8080));
        assert!(
            Mcp::try_parse_from(["mcp", "--control-port", "0"]).is_err(),
            "port 0 must be rejected at parse time"
        );
    }
}
