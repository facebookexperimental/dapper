// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::io::Write;
use std::str::FromStr;

use anyhow::Context;
use clap::ArgGroup;
use clap::Parser;
use clap::Subcommand;
use dapper_config::DapperConfig;
use dapper_config::OutputFormat;
use dapper_control_api::ControlPlaneResult;
use dapper_control_api::DapperControlPlane;
use dapper_control_api::DapperControlPlaneClient;
use dapper_control_api::RenderedResponse;
use dapper_control_api::render;
use dapper_dap_protocol::data_types::FrameId;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::data_types::ThreadId;
use dapper_dap_protocol::data_types::VariablesReference;
use dapper_session::NavigationType;
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionInfo;
use dapper_session::SessionStore;
use dapper_session::SessionsResult;

use crate::commands::SessionTarget;

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
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(line) = s.parse::<i64>() {
            return Ok(BreakpointArg::Line(line));
        }
        // Interpolate the serde error rather than attaching it as context:
        // clap renders value-parser errors via `Display`, which drops the
        // source chain.
        serde_json::from_str(s).map_err(|e| anyhow::anyhow!("invalid breakpoint spec: {e}"))
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
        #[arg(long)]
        clear_existing: bool,
    },
    /// Set exception breakpoint filters at the debug adapter.
    ///
    /// Use `dapper debug capabilities` to discover supported filter ids
    /// (e.g. "raised", "uncaught", "cpp_throw"). The `--filter` flag is
    /// repeatable; omit it together with `--clear-existing` to disable
    /// all installed exception breakpoints (`dapper debug
    /// set-exception-breakpoints --clear-existing`).
    #[command(group(
        ArgGroup::new("filters_or_clear")
            .args(["filters", "clear_existing"])
            .required(true)
            .multiple(true)
    ))]
    SetExceptionBreakpoints {
        /// Filter id(s) to enable. Repeat the flag for each filter, e.g.
        /// `--filter raised --filter uncaught`. Discover supported ids
        /// via `dapper debug capabilities`.
        #[arg(long = "filter")]
        filters: Vec<String>,
        /// Clear existing exception filters before enabling these. Pass
        /// alone (without any `--filter`) to disable all exception
        /// breakpoints.
        #[arg(long)]
        clear_existing: bool,
    },
    /// List all active debug sessions
    Sessions {},
    /// Print the JSON capabilities reported by the debug adapter (from the
    /// `initialize` response). Includes `exceptionBreakpointFilters` if the
    /// adapter advertises any. Stdout is raw JSON with or without `--json`;
    /// pipe it through `jq` to pretty-print. Before the initialize response
    /// arrives it prints `null`, with a notice on stderr, and still exits 0.
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
        #[arg(long)]
        wait_for_event: bool,
        /// Timeout in seconds for the request and, with `--wait-for-event`, for
        /// the event wait. Default: 60; 0 also means 60.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
}

/// Connect to the control plane and send a command for debugging
#[derive(Parser)]
pub struct Debug {
    #[command(flatten)]
    target: SessionTarget,

    #[command(subcommand)]
    command: DebugCommands,
}

impl Debug {
    pub async fn run(self, config: DapperConfig) -> anyhow::Result<()> {
        let client = match self.target.control_port {
            Some(port) => DapperControlPlaneClient::for_port(port),
            None => DapperControlPlaneClient::discover(
                SessionStore::default_location()?,
                self.target.scope_id.clone(),
            ),
        };

        match self.command {
            DebugCommands::Status {} => {
                let result = client.status().await.context("Error getting status")?;
                let config = DapperConfig {
                    context: dapper_config::ContextConfig::all_enabled(),
                    ..config
                };
                print_rendered(&result, &config)?;
            }
            DebugCommands::Config {} => {
                let session = find_config_session(
                    &SessionStore::default_location()?,
                    self.target.control_port,
                    self.target.scope_id,
                )?;
                let output = serde_json::json!({
                    "debugger_args": session.debugger_args,
                    "dapper_config": config,
                });
                try_println(format_args!("{:#}", output))?;
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
                print_rendered(&result, &config)?;
            }
            DebugCommands::Threads {} => {
                let result = client.threads().await.context("Error getting threads")?;
                print_rendered(&result, &config)?;
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
                print_rendered(&result, &config)?;
            }
            DebugCommands::Scopes { frame_id } => {
                let result = client
                    .scopes(frame_id)
                    .await
                    .context("Error getting scopes")?;
                print_rendered(&result, &config)?;
            }
            DebugCommands::Variables {
                variables_reference,
            } => {
                let result = client
                    .variables(variables_reference)
                    .await
                    .context("Error getting variables")?;
                print_rendered(&result, &config)?;
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
                print_rendered(&result, &config)?;
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
                print_rendered(&result, &config)?;
            }
            DebugCommands::Continue {
                thread_id,
                single_thread,
            } => {
                let result = client
                    .navigate(NavigationType::Continue, thread_id, single_thread)
                    .await
                    .context("Error executing continue")?;
                print_rendered(&result, &config)?;
            }
            DebugCommands::ReverseContinue {
                thread_id,
                single_thread,
            } => {
                let result = client
                    .navigate(NavigationType::ReverseContinue, thread_id, single_thread)
                    .await
                    .context("Error executing reverse-continue")?;
                print_rendered(&result, &config)?;
            }
            DebugCommands::Pause { thread_id } => {
                let result = client
                    .navigate(NavigationType::Pause, thread_id, None)
                    .await
                    .context("Error executing pause")?;
                print_rendered(&result, &config)?;
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
                print_rendered(&result, &config)?;
            }
            DebugCommands::SetExceptionBreakpoints {
                filters,
                clear_existing,
            } => {
                let result = client
                    .set_exception_breakpoints(&filters, clear_existing)
                    .await
                    .context("Error setting exception breakpoints")?;
                print_rendered(&result, &config)?;
            }
            DebugCommands::Sessions {} => {
                let sessions: Vec<SessionInfo> = SessionStore::default_location()?
                    .iter_active_sessions(self.target.scope_id.clone())
                    .collect();

                let sessions_result = SessionsResult {
                    sessions,
                    scope_id: self.target.scope_id,
                };
                let result = ControlPlaneResult {
                    result: sessions_result,
                    context: None,
                };
                print_rendered(&result, &config)?;
            }
            DebugCommands::Capabilities {} => {
                let caps = client
                    .capabilities()
                    .await
                    .context("Error fetching capabilities")?;
                if caps.result.0.is_none() {
                    eprintln!(
                        "Adapter capabilities not yet available (initialize response not received)."
                    );
                }
                // Serializes to the adapter's blob, or `null` before it arrives.
                try_println(format_args!("{}", serde_json::to_string(&caps.result)?))?;
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
                    OutputFormat::Plaintext => {
                        RenderedResponse::from_text(result.to_string()).spill_to_temp_and_render()
                    }
                };
                try_println(format_args!("{}", output))?;
            }
        }
        Ok(())
    }
}

fn find_config_session(
    store: &SessionStore,
    control_port: Option<Port>,
    scope_id: Option<ScopeId>,
) -> anyhow::Result<SessionInfo> {
    let Some(port) = control_port else {
        return dapper_control_api::resolve_unique_session(
            store.iter_active_sessions(scope_id.clone()).collect(),
            &scope_id,
            None,
        );
    };
    // Unscoped like `DapperControlPlaneClient::for_port`: an exported
    // `DAPPER_SCOPE_ID` must not hide the session `--control-port` names.
    store
        .iter_active_sessions(None)
        .find(|s| s.control_plane_port == Some(port))
        .with_context(|| format!("no session found on port {port}"))
}

/// Print a line to stdout, returning write errors instead of panicking the
/// way `println!` does; `Commands::run` maps a closed pipe to exit code 32.
fn try_println(args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_fmt(args)?;
    handle.write_all(b"\n")
}

fn print_rendered<T: std::fmt::Display + serde::Serialize>(
    result: &ControlPlaneResult<T>,
    config: &DapperConfig,
) -> anyhow::Result<()> {
    try_println(format_args!("{}", render(result, config)?))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use clap::Parser;
    use clap::error::ErrorKind;

    use super::*;

    #[test]
    fn parse_scope_id_from_cli_arg() {
        let debug =
            Debug::try_parse_from(["debug", "--scope-id", "test-scope", "threads"]).unwrap();
        assert_eq!(debug.target.scope_id, Some(ScopeId::new("test-scope")));
    }

    #[test]
    fn parse_scope_id_from_env_var() {
        temp_env::with_var("DAPPER_SCOPE_ID", Some("env-scope"), || {
            let debug = Debug::try_parse_from(["debug", "threads"]).unwrap();
            assert_eq!(debug.target.scope_id, Some(ScopeId::new("env-scope")));
        });
    }

    #[test]
    fn cli_arg_takes_precedence_over_env_var() {
        temp_env::with_var("DAPPER_SCOPE_ID", Some("env-scope"), || {
            let debug =
                Debug::try_parse_from(["debug", "--scope-id", "cli-scope", "threads"]).unwrap();
            assert_eq!(debug.target.scope_id, Some(ScopeId::new("cli-scope")));
        });
    }

    #[test]
    fn defaults_when_neither_arg_nor_env() {
        temp_env::with_var_unset("DAPPER_SCOPE_ID", || {
            let debug = Debug::try_parse_from(["debug", "threads"]).unwrap();
            assert_eq!(debug.target.scope_id, None);
            assert_eq!(debug.target.control_port, None);
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
        assert_eq!(debug.target.scope_id, Some(ScopeId::new("my-scope")));
    }

    #[test]
    fn config_session_on_control_port_ignores_scope() {
        let dir = tempfile::tempdir().expect("create temp sessions dir");
        let store = SessionStore::at(dir.path());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        let port = Port::try_new(listener.local_addr().expect("local addr").port())
            .expect("an ephemeral port is non-zero");
        let session = SessionInfo::generate(
            "other-scope".into(),
            Some(port),
            Some(ScopeId::new("other")),
            None,
            None,
        );
        store.save(&session).expect("seed the store");

        let found = find_config_session(&store, Some(port), Some(ScopeId::new("mine")))
            .expect("--control-port names the session whatever its scope");
        assert_eq!(found.session_id, session.session_id);

        assert!(
            find_config_session(&store, None, Some(ScopeId::new("mine"))).is_err(),
            "auto-discovery must still filter by scope"
        );
        drop(listener);
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
    fn parse_set_exception_breakpoints_filter_with_clear_existing() {
        let debug = Debug::try_parse_from([
            "debug",
            "set-exception-breakpoints",
            "--filter",
            "raised",
            "--clear-existing",
        ])
        .unwrap();
        let DebugCommands::SetExceptionBreakpoints {
            filters,
            clear_existing,
        } = debug.command
        else {
            panic!("expected SetExceptionBreakpoints variant");
        };
        assert_eq!(filters, ["raised"]);
        assert!(clear_existing);
    }

    #[test]
    fn set_exception_breakpoints_rejects_bare_invocation() {
        let Err(err) = Debug::try_parse_from(["debug", "set-exception-breakpoints"]) else {
            panic!("passing neither --filter nor --clear-existing must fail to parse");
        };
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn breakpoint_arg_parses_bare_line() {
        let arg: BreakpointArg = "10".parse().expect("bare line number");
        assert!(matches!(arg, BreakpointArg::Line(10)));
        assert_eq!(SourceBreakpoint::from(arg).line, 10);
    }

    #[test]
    fn breakpoint_arg_parses_json_spec() {
        let arg: BreakpointArg = r#"{"line":20,"condition":"x>5"}"#.parse().expect("json spec");
        let bp = SourceBreakpoint::from(arg);
        assert_eq!(bp.line, 20);
        assert_eq!(bp.condition.as_deref(), Some("x>5"));
        assert_eq!(bp.log_message, None);
    }

    #[test]
    fn breakpoint_arg_accepts_log_message_alias() {
        let arg: BreakpointArg = r#"{"line":30,"logMessage":"x={x}"}"#
            .parse()
            .expect("logMessage alias");
        assert_eq!(
            SourceBreakpoint::from(arg).log_message.as_deref(),
            Some("x={x}")
        );
    }

    #[test]
    fn breakpoint_arg_rejects_non_line_non_json() {
        let err = "not-a-breakpoint"
            .parse::<BreakpointArg>()
            .expect_err("neither a line number nor JSON");
        assert!(
            err.to_string().starts_with("invalid breakpoint spec: "),
            "clap renders this via Display, so the serde detail must be inline: {err}"
        );
    }

    #[test]
    fn breakpoint_arg_rejects_json_without_line() {
        assert!(
            r#"{"condition":"x>5"}"#.parse::<BreakpointArg>().is_err(),
            "a spec with no line has nowhere to set a breakpoint"
        );
    }
}
