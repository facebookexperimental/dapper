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
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionStore;
use strum::VariantNames;

fn debug_tool_parser() -> impl clap::builder::TypedValueParser {
    clap::builder::PossibleValuesParser::new(DebugTool::VARIANTS)
        .try_map(|s| s.parse::<DebugTool>())
}

/// Start the MCP server on stdin/stdout
#[derive(Parser)]
pub struct Mcp {
    /// Control plane port to connect to.
    /// If omitted, auto-discovers the unique active session — or errors with the
    /// candidate list when more than one is active. Pass --control-port (always
    /// deterministic) or a tighter --scope-id / DAPPER_SCOPE_ID to disambiguate.
    /// Tools also accept a `session_id` argument for per-call targeting.
    #[arg(long)]
    control_port: Option<Port>,
    /// Scope identifier to target a specific session.
    /// Filters auto-discovery. May also be set via DAPPER_SCOPE_ID.
    #[arg(long, env = "DAPPER_SCOPE_ID")]
    scope_id: Option<ScopeId>,
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
            control_port: self.control_port,
            scope_id: self.scope_id,
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
    fn parse_scope_id_from_cli_arg() {
        let mcp = Mcp::try_parse_from(["mcp", "--scope-id", "test-scope"]).unwrap();
        assert_eq!(mcp.scope_id, Some(ScopeId::new("test-scope")));
    }

    #[test]
    fn parse_scope_id_from_env_var() {
        temp_env::with_var("DAPPER_SCOPE_ID", Some("env-scope"), || {
            let mcp = Mcp::try_parse_from(["mcp"]).unwrap();
            assert_eq!(mcp.scope_id, Some(ScopeId::new("env-scope")));
        });
    }

    #[test]
    fn cli_arg_takes_precedence_over_env_var() {
        temp_env::with_var("DAPPER_SCOPE_ID", Some("env-scope"), || {
            let mcp = Mcp::try_parse_from(["mcp", "--scope-id", "cli-scope"]).unwrap();
            assert_eq!(mcp.scope_id, Some(ScopeId::new("cli-scope")));
        });
    }

    #[test]
    fn control_port_parses_and_rejects_zero() {
        let mcp = Mcp::try_parse_from(["mcp", "--control-port", "8080"]).unwrap();
        assert_eq!(mcp.control_port.map(|p| p.get()), Some(8080));
        assert!(
            Mcp::try_parse_from(["mcp", "--control-port", "0"]).is_err(),
            "port 0 must be rejected at parse time"
        );
    }

    #[test]
    fn defaults_when_neither_arg_nor_env() {
        temp_env::with_var_unset("DAPPER_SCOPE_ID", || {
            let mcp = Mcp::try_parse_from(["mcp"]).unwrap();
            assert_eq!(mcp.scope_id, None);
            assert_eq!(mcp.control_port, None);
        });
    }
}
