// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Result;
use clap::Parser;
use dapper_cli::cli::Cli;
use dapper_cli::cli::Commands;
use dapper_cli::help;
use dapper_cli::invocation::Reentry;
use dapper_session::SessionId;

#[tokio::main]
async fn main() -> Result<()> {
    let reentry = Reentry::Standalone;
    let program = reentry.program_name(std::env::args_os().next().as_deref());
    let args = help::rewrite_skill_to_help(&program, std::env::args().skip(1).collect());

    let cli = Cli::parse_from(args);

    // Help is a pure-stdout markdown render — short-circuit before
    // logging/session/config side effects so it never touches disk or
    // emits log lines. Unknown-topic errors exit at this boundary
    // rather than from inside the dispatcher so tokio's runtime gets
    // to shut down cleanly.
    if let Commands::Help { topic } = &cli.command {
        if let Err(e) = help::handle(topic, &reentry, &program, &[]) {
            e.print();
            std::process::exit(e.exit_code());
        }
        return Ok(());
    }

    let session_id = SessionId::generate();
    dapper_tracing::init_logging(dapper_tracing::default_layers()?)?;

    let config = cli.resolve_config();
    let reason = cli.normalized_reason();

    let exit_code = cli
        .command
        .run(&session_id, config, reason.as_deref(), reentry)
        .await?;

    // stdin is blocking, so we need to exit explicitly
    // TODO: find a better way to handle blocking stdin
    std::process::exit(exit_code);
}
