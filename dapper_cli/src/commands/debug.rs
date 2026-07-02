// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::io::Write;
use std::str::FromStr;

use anyhow::Context;
use clap::Parser;
use clap::Subcommand;
use dapper_config::DapperConfig;
use dapper_config::OutputFormat;
use dapper_control_api::ControlPlaneResult;
use dapper_control_api::DapperControlPlane;
use dapper_control_api::DapperControlPlaneClient;
use dapper_control_api::NavigationType;
use dapper_control_api::SessionsResult;
use dapper_control_api::render;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StepType {
    /// Step into function calls.
    In,
    /// Step over the current source line.
    Over,
    /// Step out of the current frame.
    Out,
    /// Reverse one source line (requires adapter support for reverse debugging).
    Back,
}

impl From<StepType> for NavigationType {
    fn from(step_type: StepType) -> Self {
        match step_type {
            StepType::In => NavigationType::StepIn,
            StepType::Over => NavigationType::StepOver,
            StepType::Out => NavigationType::StepOut,
            StepType::Back => NavigationType::StepBack,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(untagged)]
enum BreakpointArg {
    Line(i64),
    Spec {
        line: i64,
        #[serde(default)]
        condition: Option<String>,
        #[serde(default, alias = "logMessage")]
        log_message: Option<String>,
    },
}

impl FromStr for BreakpointArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(line) = s.parse::<i64>() {
            return Ok(BreakpointArg::Line(line));
        }
        serde_json::from_str(s).map_err(|e| format!("invalid breakpoint spec: {e}"))
    }
}

impl From<BreakpointArg> for SourceBreakpoint {
    fn from(arg: BreakpointArg) -> Self {
        match arg {
            BreakpointArg::Line(line) => SourceBreakpoint {
                line,
                ..Default::default()
            },
            BreakpointArg::Spec {
                line,
                condition,
                log_message,
            } => SourceBreakpoint {
                line,
                condition,
                log_message,
                ..Default::default()
            },
        }
    }
}

#[derive(Subcommand)]
enum DebugCommands {
    /// Get session status and context (execution state, stop reason, breakpoints)
    Status {},
    /// Print the debug session's launch/attach request and dapper config
    Config {},
    /// Stop the dapper proxy server, shutting down the debug session
    Stop {},
    /// Evaluate an expression or command in the debugger REPL
    Eval {
        /// Command to evaluate in the REPL
        command: String,
        /// Stack frame ID in which to evaluate the expression
        #[arg(long)]
        frame_id: Option<FrameId>,
    },
    /// List all threads in the debugged process
    Threads {},
    /// Print the call stack for a given thread id
    StackTrace {
        /// Thread ID to get stack trace for
        thread_id: ThreadId,
        /// The index of the first frame to return (0-based)
        #[arg(long)]
        start_frame: Option<i64>,
        /// Maximum number of stack frames to return (0 for all, uses config default if not specified)
        #[arg(long)]
        levels: Option<i64>,
    },
    /// List variable scopes for a given stack frame id
    Scopes {
        /// Frame ID to get scopes for
        frame_id: FrameId,
    },
    /// Retrieves all child variables for the given variable reference
    Variables {
        /// Variables reference to get variables for
        variables_reference: VariablesReference,
    },
    /// Step execution (in, over, out, or back)
    Step {
        /// Type of step to perform
        #[arg(value_enum)]
        step_type: StepType,
        /// Thread ID to execute the step on
        thread_id: ThreadId,
        /// If this flag is true, all other suspended threads are not resumed.
        /// Requires adapter capability `supportsSingleThreadExecutionRequests`.
        #[arg(long)]
        single_thread: Option<bool>,
    },
    /// Set the variable with the given name to a new value
    SetVariable {
        /// Variables reference containing the variable to set
        variables_reference: VariablesReference,
        /// Name of the variable to set
        name: String,
        /// New value for the variable
        value: String,
    },
    /// Resume execution until a breakpoint or program exit.
    Continue {
        /// Specifies the active thread. If the debug adapter supports single thread
        /// execution (see `supportsSingleThreadExecutionRequests`) and the argument
        /// singleThread is true, only the thread with this ID is resumed.
        thread_id: ThreadId,
        /// If this flag is true, execution is resumed only for the thread with given thread_id.
        /// Requires adapter capability `supportsSingleThreadExecutionRequests`.
        #[arg(long)]
        single_thread: Option<bool>,
    },
    /// Resume reverse execution until a breakpoint or the start of recording
    /// (requires adapter support for reverse debugging).
    ReverseContinue {
        /// Specifies the active thread. If the debug adapter supports single thread
        /// execution (see `supportsSingleThreadExecutionRequests`) and the
        /// singleThread argument is true, only the thread with this ID is resumed.
        thread_id: ThreadId,
        /// If this flag is true, backward execution is resumed only for the thread with given thread_id.
        /// Requires adapter capability `supportsSingleThreadExecutionRequests`.
        #[arg(long)]
        single_thread: Option<bool>,
    },
    /// Suspend the debuggee process
    Pause {
        /// Thread ID to pause execution
        thread_id: ThreadId,
    },
    /// Set source-line breakpoints in a file
    SetBreakpoints {
        /// Source file path to set breakpoints in
        source_path: String,
        /// Breakpoints: plain line numbers or JSON specs, e.g. -b 10 -b '{"line":20,"condition":"x>5"}'
        #[arg(short, long, required = true)]
        breakpoints: Vec<BreakpointArg>,
        /// Clear existing breakpoints in the file before adding new ones
        #[arg(long, default_value_t = false)]
        clear_existing: bool,
    },
    /// Set exception breakpoint filters at the debug adapter.
    ///
    /// Use `dapper debug capabilities` to discover supported filter ids
    /// (e.g. "raised", "uncaught", "cpp_throw"). The `--filter` flag is
    /// repeatable; omit it together with `--clear-existing` to disable
    /// all installed exception breakpoints (`dapper debug
    /// set-exception-breakpoints --clear-existing`).
    SetExceptionBreakpoints {
        /// Filter id(s) to enable. Repeat the flag for each filter, e.g.
        /// `--filter raised --filter uncaught`. Discover supported ids
        /// via `dapper debug capabilities`.
        #[arg(long = "filter")]
        filters: Vec<String>,
        /// Clear existing exception filters before enabling these. Pass
        /// alone (without any `--filter`) to disable all exception
        /// breakpoints.
        #[arg(long, default_value_t = false)]
        clear_existing: bool,
    },
    /// List all active debug sessions
    Sessions {},
    /// Print the JSON capabilities reported by the debug adapter (from the
    /// `initialize` response). Includes `exceptionBreakpointFilters` if the
    /// adapter advertises any. Stdout is always raw JSON (`null` when the
    /// initialize response has not yet arrived, in which case an explanatory
    /// notice is also printed to stderr); pipe through `jq` for pretty-
    /// printing. Always exits 0 — `--output-format` does not apply because
    /// the adapter blob has no canonical plaintext rendering at this layer.
    Capabilities {},
    /// Send a raw DAP (Debug Adapter Protocol) request
    ///
    /// Examples:
    ///   # List all threads (debugger must be stopped)
    ///   dapper debug dap threads
    ///
    ///   # Pause execution
    ///   dapper debug dap pause --arguments '{"threadId": 0}'
    ///
    ///   # Get stack trace for thread 1
    ///   dapper debug dap stackTrace --arguments '{"threadId": 1}'
    ///
    ///   # Set a breakpoint
    ///   dapper debug dap setBreakpoints \
    ///     --arguments '{"source": {"path": "/path/to/file.py"}, "breakpoints": [{"line": 10}]}'
    ///
    ///   # Continue and wait for stopped event
    ///   dapper debug dap continue --arguments '{"threadId": 1}' --wait-for-event
    ///
    ///   # To pin a specific session, use the parent flags before the subcommand:
    ///   dapper debug --control-port=PORT --scope-id=SCOPE dap threads
    #[command(verbatim_doc_comment)]
    Dap {
        /// The DAP command name (e.g., "threads", "pause", "stackTrace")
        command: String,
        /// JSON arguments for the command (optional, e.g., '{"threadId": 1}')
        #[arg(long)]
        arguments: Option<String>,
        /// Wait for stopped/exited events after request (for pause, continue, step commands)
        #[arg(long, default_value_t = false)]
        wait_for_event: bool,
        /// Timeout in seconds for event wait
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
}

/// Connect to the control plane and send a command for debugging
#[derive(Parser)]
pub struct Debug {
    /// Control plane port to connect to.
    /// If omitted, auto-discovers the unique active session — or errors with the
    /// candidate list when more than one is active. Pass --control-port (always
    /// deterministic) or a tighter --scope-id / DAPPER_SCOPE_ID to disambiguate.
    #[arg(long)]
    control_port: Option<Port>,
    /// Scope identifier to target a specific session.
    /// Filters auto-discovery and the `sessions` listing. May also be set via DAPPER_SCOPE_ID.
    #[arg(long, env = "DAPPER_SCOPE_ID")]
    scope_id: Option<ScopeId>,

    #[command(subcommand)]
    command: DebugCommands,
}

impl Debug {
    pub async fn run(self, config: DapperConfig) -> anyhow::Result<()> {
        let client = DapperControlPlaneClient::new(self.control_port, self.scope_id.clone());

        match self.command {
            DebugCommands::Status {} => {
                let result = client.status().await.context("Error getting status")?;
                let config = DapperConfig {
                    context: dapper_config::ContextConfig::all_enabled(),
                    ..config
                };
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Config {} => {
                let session = if let Some(port) = self.control_port {
                    SessionInfo::iter_active_sessions(self.scope_id.clone())
                        .context("Error listing sessions")?
                        .find(|s| s.control_plane_port.map(|p| p.get()) == Some(port.get()))
                        .ok_or_else(|| anyhow::anyhow!("no session found on port {}", port.get()))?
                } else {
                    dapper_control_api::resolve_unique_session(
                        SessionInfo::iter_active_sessions(self.scope_id.clone())
                            .context("Error listing sessions")?
                            .collect(),
                        &self.scope_id,
                        None,
                    )?
                };
                let output = serde_json::json!({
                    "debugger_args": session.debugger_args,
                    "dapper_config": config,
                });
                safe_println(format_args!(
                    "{}",
                    serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string())
                ));
            }
            DebugCommands::Stop {} => {
                client.stop().await?;
            }
            DebugCommands::Eval { command, frame_id } => {
                let result = client
                    .eval_repl(&command, frame_id)
                    .await
                    .context("Error evaluating command")?;
                let result = ControlPlaneResult {
                    result,
                    context: None,
                };
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Threads {} => {
                let result = client.threads().await.context("Error getting threads")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::StackTrace {
                thread_id,
                levels,
                start_frame,
            } => {
                let result = client
                    .stack_trace(thread_id, start_frame, levels)
                    .await
                    .context("Error getting stack trace")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Scopes { frame_id } => {
                let result = client
                    .scopes(frame_id)
                    .await
                    .context("Error getting scopes")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Variables {
                variables_reference,
            } => {
                let result = client
                    .variables(variables_reference)
                    .await
                    .context("Error getting variables")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Step {
                step_type,
                thread_id,
                single_thread,
            } => {
                let step_label = format!("{:?}", step_type);
                let result = client
                    .navigate(NavigationType::from(step_type), thread_id, single_thread)
                    .await
                    .with_context(|| format!("Error executing step {}", step_label))?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::SetVariable {
                variables_reference,
                name,
                value,
            } => {
                let result = client
                    .set_variable(variables_reference, &name, &value)
                    .await
                    .context("Error setting variable")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Continue {
                thread_id,
                single_thread,
            } => {
                let result = client
                    .navigate(NavigationType::Continue, thread_id, single_thread)
                    .await
                    .context("Error executing continue")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::ReverseContinue {
                thread_id,
                single_thread,
            } => {
                let result = client
                    .navigate(NavigationType::ReverseContinue, thread_id, single_thread)
                    .await
                    .context("Error executing reverse-continue")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Pause { thread_id } => {
                let result = client
                    .navigate(NavigationType::Pause, thread_id, None)
                    .await
                    .context("Error executing pause")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::SetBreakpoints {
                source_path,
                breakpoints,
                clear_existing,
            } => {
                let specs: Vec<SourceBreakpoint> =
                    breakpoints.into_iter().map(Into::into).collect();
                let result = client
                    .set_breakpoints(&source_path, clear_existing, &specs)
                    .await
                    .context("Error setting breakpoints")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::SetExceptionBreakpoints {
                filters,
                clear_existing,
            } => {
                // Strict empty-input validation, matching the MCP tool.
                // The library is permissive (silent no-op), but a user
                // typing the CLI almost certainly meant something other
                // than "do nothing", so reject with an actionable error.
                if filters.is_empty() && !clear_existing {
                    anyhow::bail!("must pass at least one --filter or --clear-existing");
                }
                let result = client
                    .set_exception_breakpoints(&filters, clear_existing)
                    .await
                    .context("Error setting exception breakpoints")?;
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Sessions {} => {
                let sessions: Vec<SessionInfo> =
                    SessionInfo::iter_active_sessions(self.scope_id.clone())
                        .context("Error listing sessions")?
                        .collect();

                let sessions_result = SessionsResult {
                    sessions,
                    scope_id: self.scope_id.clone(),
                };
                let result = ControlPlaneResult {
                    result: sessions_result,
                    context: None,
                };
                safe_println(format_args!("{}", render(&result, &config)?));
            }
            DebugCommands::Capabilities {} => {
                let caps = client
                    .capabilities()
                    .await
                    .context("Error fetching capabilities")?;
                match caps {
                    Some(json) => safe_println(format_args!("{}", json)),
                    None => {
                        eprintln!(
                            "Adapter capabilities not yet available (initialize response not received)."
                        );
                        safe_println(format_args!("null"));
                    }
                }
            }
            DebugCommands::Dap {
                command,
                arguments,
                wait_for_event,
                timeout,
            } => {
                let args: Option<serde_json::Value> = arguments
                    .map(|json_str| {
                        serde_json::from_str(&json_str)
                            .map_err(|e| anyhow::anyhow!("Invalid JSON arguments: {}", e))
                    })
                    .transpose()?;
                let result = client
                    .send_dap_request(&command, args, wait_for_event, timeout)
                    .await
                    .with_context(|| format!("Error executing DAP request '{}'", command))?;
                let output = match config.output_format {
                    OutputFormat::Json => result.render_json(),
                    OutputFormat::Plaintext => result.render(),
                };
                safe_println(format_args!("{}", output));
            }
        }
        Ok(())
    }
}

fn safe_println(args: std::fmt::Arguments<'_>) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let result = handle.write_fmt(args).and_then(|_| handle.write_all(b"\n"));
    if let Err(e) = result {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(32);
        }
        panic!("failed printing to stdout: {e}");
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parse_scope_id_from_cli_arg() {
        let debug =
            Debug::try_parse_from(["debug", "--scope-id", "test-scope", "threads"]).unwrap();
        assert_eq!(debug.scope_id, Some(ScopeId::new("test-scope")));
    }

    #[test]
    fn parse_scope_id_from_env_var() {
        temp_env::with_var("DAPPER_SCOPE_ID", Some("env-scope"), || {
            let debug = Debug::try_parse_from(["debug", "threads"]).unwrap();
            assert_eq!(debug.scope_id, Some(ScopeId::new("env-scope")));
        });
    }

    #[test]
    fn cli_arg_takes_precedence_over_env_var() {
        temp_env::with_var("DAPPER_SCOPE_ID", Some("env-scope"), || {
            let debug =
                Debug::try_parse_from(["debug", "--scope-id", "cli-scope", "threads"]).unwrap();
            assert_eq!(debug.scope_id, Some(ScopeId::new("cli-scope")));
        });
    }

    #[test]
    fn defaults_when_neither_arg_nor_env() {
        temp_env::with_var_unset("DAPPER_SCOPE_ID", || {
            let debug = Debug::try_parse_from(["debug", "threads"]).unwrap();
            assert_eq!(debug.scope_id, None);
            assert_eq!(debug.control_port, None);
        });
    }

    #[test]
    fn parse_capabilities_subcommand() {
        let debug = Debug::try_parse_from(["debug", "capabilities"]).unwrap();
        assert!(matches!(debug.command, DebugCommands::Capabilities {}));
    }

    #[test]
    fn capabilities_subcommand_rejects_extra_args() {
        assert!(Debug::try_parse_from(["debug", "capabilities", "foo"]).is_err());
    }

    #[test]
    fn parse_config_subcommand() {
        let debug = Debug::try_parse_from(["debug", "config"]).unwrap();
        assert!(matches!(debug.command, DebugCommands::Config {}));
    }

    #[test]
    fn config_subcommand_rejects_extra_args() {
        assert!(Debug::try_parse_from(["debug", "config", "foo"]).is_err());
    }

    #[test]
    fn config_subcommand_with_scope_id() {
        let debug = Debug::try_parse_from(["debug", "--scope-id", "my-scope", "config"]).unwrap();
        assert!(matches!(debug.command, DebugCommands::Config {}));
        assert_eq!(debug.scope_id, Some(ScopeId::new("my-scope")));
    }

    #[test]
    fn parse_set_exception_breakpoints_with_filters() {
        let debug = Debug::try_parse_from([
            "debug",
            "set-exception-breakpoints",
            "--filter",
            "raised",
            "--filter",
            "uncaught",
        ])
        .unwrap();
        let DebugCommands::SetExceptionBreakpoints {
            filters,
            clear_existing,
        } = debug.command
        else {
            panic!("expected SetExceptionBreakpoints variant");
        };
        assert_eq!(filters, vec!["raised".to_string(), "uncaught".to_string()]);
        assert!(!clear_existing);
    }

    #[test]
    fn parse_set_exception_breakpoints_clear_existing_alone() {
        // Empty --filter list with --clear-existing alone is the
        // documented "clear all" path — must parse cleanly even though
        // --filter has no `required = true`.
        let debug =
            Debug::try_parse_from(["debug", "set-exception-breakpoints", "--clear-existing"])
                .unwrap();
        let DebugCommands::SetExceptionBreakpoints {
            filters,
            clear_existing,
        } = debug.command
        else {
            panic!("expected SetExceptionBreakpoints variant");
        };
        assert!(filters.is_empty());
        assert!(clear_existing);
    }

    #[test]
    fn parse_set_exception_breakpoints_clap_alone_does_not_reject_bare_invocation() {
        // The clap parser must accept `set-exception-breakpoints` with
        // no flags (since `--filter` isn't `required = true` and
        // `--clear-existing` defaults to false) so the documented
        // "clear all" idiom (`--clear-existing` alone) still parses.
        // The actual rejection of the empty + !clear case happens at
        // runtime in the dispatch arm — not exercised here because
        // calling it would require spinning up the control plane.
        let debug = Debug::try_parse_from(["debug", "set-exception-breakpoints"]).unwrap();
        let DebugCommands::SetExceptionBreakpoints {
            filters,
            clear_existing,
        } = debug.command
        else {
            panic!("expected SetExceptionBreakpoints variant");
        };
        assert!(filters.is_empty());
        assert!(!clear_existing);
    }
}
