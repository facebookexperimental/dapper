// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use clap::Parser;
use clap::Subcommand;
use dapper_config::DapperConfig;
use dapper_session::SessionId;
use dapper_tracing::normalize_reason;

use crate::invocation::Reentry;

#[derive(Parser)]
#[command(name = "dapper")]
#[command(about = "Debug Adapter Protocol (DAP) proxy, (de-)multiplexer, client, and MCP server")]
#[command(version)]
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

    /// Why this command is being run. Recorded in Dapper's telemetry.
    ///
    /// Automation and agents should pass it on every command except the help
    /// paths (`help`, `--help`, `-h`), which render and exit before logging
    /// starts. Give one short line of intent, e.g.
    /// `--reason "inspect deadlocked worker thread"`. A `--reason` after a
    /// help topic is read as part of the topic name, so that form fails, and
    /// `proxy process` forwards everything after the adapter command, so a
    /// trailing `--reason` silently reaches the adapter instead.
    #[arg(long, global = true, env = "DAPPER_REASON")]
    pub reason: Option<String>,

    /// Output in JSON format instead of plaintext.
    #[arg(long, global = true, env = "DAPPER_OUTPUT_JSON")]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

impl Cli {
    /// `--reason`, sanitized for logging.
    pub fn normalized_reason(&self) -> Option<String> {
        normalize_reason(self.reason.as_deref()?)
    }

    /// Load config once and apply the CLI `--json` override if present.
    pub fn resolve_config(&self) -> DapperConfig {
        let mut config = DapperConfig::load_or_default();
        if self.json {
            config.output_format = dapper_config::OutputFormat::Json;
        }
        config
    }
}

/// Must run after logging is initialized, so [`Commands::run`] owns the call
/// for both binaries.
fn log_reason(reason: Option<&str>) {
    if let Some(reason) = reason {
        tracing::info!(invocation_reason = %reason, "dapper_invocation_reason");
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
    /// Run a non-`Help` subcommand, returning the process exit code for
    /// the binary entry point to exit with. The `Help` variant is
    /// dispatched in the binary entry points *before* tracing/logging is
    /// set up, so help rendering never touches disk or emits log lines —
    /// see `dapper_cli/bin/main.rs` and `fb/dapper_fb_main/src/lib.rs`.
    pub async fn run(
        self,
        session_id: &SessionId,
        config: DapperConfig,
        reason: Option<&str>,
        reentry: Reentry,
    ) -> anyhow::Result<i32> {
        tracing::info!("Dapper session: {}", session_id);
        log_reason(reason);

        let result = match self {
            Commands::Debug(cmd) => cmd.run(config).await,
            Commands::Proxy(cmd) => cmd.run(session_id, config, reentry).await,
            Commands::Mcp(cmd) => cmd.run(config).await,
            Commands::Help { .. } => {
                unreachable!("Help is dispatched in the binary entry point before Commands::run")
            }
        };

        match result {
            Ok(()) => Ok(0),
            // A closed stdout (e.g. `dapper debug ... | head`) is not a
            // command failure: report the conventional exit code instead
            // of an error trace. Both binaries funnel through here.
            Err(err) if is_broken_pipe(&err) => Ok(32),
            Err(err) => {
                tracing::error!("Dapper top-level error: {:#}", err);
                Err(err)
            }
        }
    }
}

fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::BrokenPipe)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use clap::Parser;

    use super::*;

    /// Collects rendered event fields for assertion.
    #[derive(Clone, Default)]
    struct CapturedEvents(Arc<Mutex<Vec<String>>>);

    impl CapturedEvents {
        fn contains(&self, needles: &[&str]) -> bool {
            self.0
                .lock()
                .expect("capture mutex is never poisoned in tests")
                .iter()
                .any(|event| needles.iter().all(|needle| event.contains(needle)))
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedEvents {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Visitor<'a>(&'a mut String);
            impl tracing::field::Visit for Visitor<'_> {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    use std::fmt::Write;
                    let _ = write!(self.0, "{}={:?} ", field.name(), value);
                }
            }

            let mut rendered = String::new();
            event.record(&mut Visitor(&mut rendered));
            self.0
                .lock()
                .expect("capture mutex is never poisoned in tests")
                .push(rendered);
        }
    }

    // `set_default` is thread-local and the spawned work must share the thread.
    #[tokio::test(flavor = "current_thread")]
    async fn run_logs_the_invocation_reason() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let sessions = tempfile::tempdir().expect("create temp sessions dir");
        let sessions_dir = sessions.path().to_str().expect("temp path is utf-8");
        let captured = CapturedEvents::default();

        temp_env::async_with_vars(
            [
                ("DAPPER_SESSIONS_DIR", Some(sessions_dir)),
                ("DAPPER_REASON", None),
            ],
            async {
                let _guard = tracing::subscriber::set_default(
                    tracing_subscriber::registry().with(captured.clone()),
                );
                let cli = Cli::try_parse_from(["dapper", "debug", "threads"]).expect("parses");

                let _ = cli
                    .command
                    .run(
                        &SessionId::generate(),
                        DapperConfig::default(),
                        Some("check the reason reaches the logs"),
                        Reentry::Standalone,
                    )
                    .await;
            },
        )
        .await;

        // One needle: split needles also match the reason folded into the message.
        assert!(
            captured.contains(&["invocation_reason=check the reason reaches the logs"]),
            "Commands::run must log the reason as a structured field"
        );
    }

    /// Pins the `--json` override the proxy path now also relies on.
    #[test]
    fn resolve_config_applies_json_override() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let config_dir = dir.path().to_str().expect("temp path is utf-8");
        temp_env::with_vars(
            [
                ("DAPPER_CONFIG_DIR", Some(config_dir)),
                ("DAPPER_OUTPUT_JSON", None),
            ],
            || {
                assert_eq!(
                    Cli::parse_from(["dapper", "--json", "help"])
                        .resolve_config()
                        .output_format,
                    dapper_config::OutputFormat::Json,
                );
                assert_eq!(
                    Cli::parse_from(["dapper", "help"])
                        .resolve_config()
                        .output_format,
                    dapper_config::OutputFormat::Plaintext,
                );
            },
        );
    }

    #[test]
    fn reason_parses_at_every_subcommand_level() {
        temp_env::with_var_unset("DAPPER_REASON", || {
            for args in [
                ["dapper", "--reason", "why", "debug", "threads"],
                ["dapper", "debug", "--reason", "why", "threads"],
                ["dapper", "debug", "threads", "--reason", "why"],
            ] {
                let cli = Cli::try_parse_from(args).expect("--reason is accepted at every level");
                assert_eq!(cli.reason.as_deref(), Some("why"), "for {args:?}");
            }
        });
    }

    #[test]
    fn help_swallows_a_trailing_reason_but_not_a_leading_one() {
        temp_env::with_var_unset("DAPPER_REASON", || {
            let cli = Cli::try_parse_from(["dapper", "help", "agent", "--reason", "why"])
                .expect("the topic absorbs trailing flags");
            assert_eq!(cli.reason, None);
            let Commands::Help { topic } = cli.command else {
                panic!("expected the help subcommand");
            };
            assert_eq!(topic, ["agent", "--reason", "why"]);
            assert!(matches!(
                crate::help::handle(&topic, "dapper", &[]),
                Err(crate::help::HelpError::UnknownTopic { .. })
            ));

            let cli = Cli::try_parse_from(["dapper", "--reason", "why", "help", "agent"])
                .expect("a leading --reason parses ahead of the subcommand");
            assert_eq!(cli.reason.as_deref(), Some("why"));
        });
    }

    #[test]
    fn normalized_reason_delegates_to_the_shared_normalizer() {
        temp_env::with_var_unset("DAPPER_REASON", || {
            let cli = Cli::try_parse_from(["dapper", "--reason", " a\n b ", "help"]).unwrap();
            assert_eq!(cli.normalized_reason().as_deref(), Some("a b"));

            let cli = Cli::try_parse_from(["dapper", "--reason", "   ", "help"]).unwrap();
            assert_eq!(cli.normalized_reason(), None);
        });
    }

    #[test]
    fn reason_is_optional() {
        temp_env::with_var_unset("DAPPER_REASON", || {
            let cli = Cli::try_parse_from(["dapper", "debug", "threads"])
                .expect("omitting --reason must not break existing callers");
            assert_eq!(cli.reason, None);
        });
    }

    #[test]
    fn reason_falls_back_to_env_var() {
        temp_env::with_var("DAPPER_REASON", Some("env-reason"), || {
            assert_eq!(
                Cli::try_parse_from(["dapper", "debug", "threads"])
                    .unwrap()
                    .reason
                    .as_deref(),
                Some("env-reason"),
            );
            assert_eq!(
                Cli::try_parse_from(["dapper", "debug", "threads", "--reason", "cli-reason"])
                    .unwrap()
                    .reason
                    .as_deref(),
                Some("cli-reason"),
                "an explicit flag must win over the ambient env var",
            );
        });
    }
}
