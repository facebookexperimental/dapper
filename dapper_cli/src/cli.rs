// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use clap::Parser;
use clap::Subcommand;
use dapper_config::DapperConfig;
use dapper_session::SessionId;

#[derive(Parser)]
#[command(name = "dapper")]
#[command(about = "Debug Adapter Protocol (DAP) proxy, (de-)multiplexer, client, and MCP server")]
#[command(version = "0.1.0")]
// Disable clap's auto-generated `help` subcommand so our `Help` variant
// owns the slot. `--help` / `-h` are unaffected and still print clap's
// terse synopsis. `dapper help` (without `--`) is the canonical
// LLM-optimized surface and is documented under `--help`'s subcommand
// list, so no `after_help` block is needed.
#[command(disable_help_subcommand = true)]
pub struct Cli {
    /// Name of the client invoking the CLI. Used to break down Dapper's telemetry.
    #[arg(long, env = "DAPPER_CALLER_TO_LOG")]
    pub caller_to_log: Option<String>,

    /// Output in JSON format instead of plaintext.
    #[arg(long, global = true, env = "DAPPER_OUTPUT_JSON")]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

impl Cli {
    /// Load config once and apply the CLI `--json` override if present.
    pub fn resolve_config(&self) -> DapperConfig {
        let mut config = DapperConfig::load_or_default();
        if self.json {
            config.output_format = dapper_config::OutputFormat::Json;
        }
        config
    }
}

#[derive(Subcommand, strum::IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum Commands {
    Debug(crate::commands::Debug),
    Proxy(crate::commands::Proxy),
    Mcp(crate::commands::Mcp),
    /// Show LLM-optimized documentation for Dapper or a specific topic.
    #[command(alias = "docs")]
    Help {
        /// Topic to display (e.g., `agent`, `sessions`, `debug threads`).
        /// With no topic, prints the overview.
        #[arg(trailing_var_arg = true)]
        topic: Vec<String>,
    },
}

impl Commands {
    /// Run a non-`Help` subcommand. The `Help` variant is dispatched
    /// in the binary entry points *before* tracing/logging is set up,
    /// so help rendering never touches disk or emits log lines — see
    /// `dapper_cli/bin/main.rs` and `fb/dapper_fb_main/src/lib.rs`.
    pub async fn run(self, session_id: &SessionId, config: DapperConfig) -> anyhow::Result<()> {
        tracing::info!("Dapper session: {}", session_id);

        match self {
            Commands::Debug(cmd) => cmd.run(config).await,
            Commands::Proxy(cmd) => cmd.run(session_id).await,
            Commands::Mcp(cmd) => cmd.run().await,
            Commands::Help { .. } => {
                unreachable!("Help is dispatched in the binary entry point before Commands::run")
            }
        }
        .inspect_err(|err| {
            tracing::error!("Dapper top-level error: {:#}", err);
        })?;

        Ok(())
    }
}
