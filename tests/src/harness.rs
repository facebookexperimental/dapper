// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Context;
use anyhow::Result;
use dapper_proxy_server::ProgressEvent;
use dapper_session::ScopeId;
use dapper_session::config::DebugSessionConfig;
use debugserver_types::Event;
use tempfile::NamedTempFile;
use tokio::process::Child;
use tokio::process::Command as TokioCommand;

use crate::dap_client::DapClient as GeneralDapClient;

/// Temp file used to capture adapter diagnostics for panicking tests.
pub struct AdapterLog {
    file: NamedTempFile,
}

/// Executable and arguments used to spawn the selected DAP adapter.
pub struct AdapterCommand {
    pub executable: String,
    pub arguments: Vec<String>,
}

impl AdapterLog {
    pub fn new() -> Result<Self> {
        let file = NamedTempFile::new().context("Failed to create adapter log file")?;
        Ok(Self { file })
    }

    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

impl Drop for AdapterLog {
    fn drop(&mut self) {
        if !thread::panicking() {
            return;
        }
        match std::fs::read_to_string(self.file.path()) {
            Ok(contents) if contents.is_empty() => {
                eprintln!(
                    "--- adapter log empty (adapter wrote nothing; non-lldb-dap adapters never do) ---"
                );
            }
            Ok(contents) => {
                eprintln!(
                    "--- adapter log start ---\n{}\n--- adapter log end ---",
                    contents.trim_end()
                );
            }
            Err(error) => {
                eprintln!(
                    "--- adapter log unreadable ({}): {} ---",
                    self.file.path().display(),
                    error
                );
            }
        }
    }
}

/// A DAP client connected to a Dapper proxy subprocess.
pub struct DapClient {
    inner_client: GeneralDapClient,
    _adapter_log: AdapterLog,
}

/// Captured output from a Dapper debug CLI invocation.
pub struct DebugCliOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

impl DapClient {
    pub fn new(scope_id: Option<ScopeId>) -> Result<Self> {
        let dapper_path = dapper_executable()?;
        Self::new_with_binary(&dapper_path, None, scope_id)
    }

    pub fn new_with_adapter_args(scope_id: Option<ScopeId>, adapter_args: &[&str]) -> Result<Self> {
        let dapper_path = dapper_executable()?;
        Self::new_with_binary_and_adapter_args(&dapper_path, None, scope_id, adapter_args)
    }

    pub fn new_with_control_plane_port(
        control_port: Option<u16>,
        scope_id: Option<ScopeId>,
    ) -> Result<Self> {
        let dapper_path = dapper_executable()?;
        Self::new_with_binary(&dapper_path, control_port, scope_id)
    }

    pub fn new_with_binary(
        dapper_path: &str,
        control_port: Option<u16>,
        scope_id: Option<ScopeId>,
    ) -> Result<Self> {
        Self::new_with_binary_and_adapter_args(dapper_path, control_port, scope_id, &[])
    }

    pub fn new_with_binary_and_adapter_args(
        dapper_path: &str,
        control_port: Option<u16>,
        scope_id: Option<ScopeId>,
        adapter_args: &[&str],
    ) -> Result<Self> {
        let adapter = adapter_command()?;

        let mut command = dapper_command(dapper_path, "proxy");
        if let Some(port) = control_port {
            command.arg("--control-port").arg(port.to_string());
        }
        if let Some(scope) = scope_id {
            command.arg("--scope-id").arg(scope.as_str());
        }
        command.arg("process").arg(adapter.executable);
        command.args(adapter.arguments);
        command.args(adapter_args);

        let adapter_log = AdapterLog::new()?;
        command.env("LLDBDAP_LOG", adapter_log.path());

        let inner_client = GeneralDapClient::new_with_command(command)?;

        Ok(Self {
            inner_client,
            _adapter_log: adapter_log,
        })
    }

    pub fn new_from_config(
        config_path: impl AsRef<Path>,
        scope_id: Option<ScopeId>,
    ) -> Result<Self> {
        let dapper_path = dapper_executable()?;

        let mut command = dapper_command(&dapper_path, "proxy");
        if let Some(scope) = scope_id {
            command.arg("--scope-id").arg(scope.as_str());
        }
        command.arg("from-config").arg(config_path.as_ref());

        let adapter_log = AdapterLog::new()?;
        command.env("LLDBDAP_LOG", adapter_log.path());

        let inner_client = GeneralDapClient::new_with_command(command)?;

        Ok(Self {
            inner_client,
            _adapter_log: adapter_log,
        })
    }

    pub fn initialize(&mut self) -> Result<()> {
        let initialize = r#"{"type": "request", "command": "initialize", "arguments": {"adapterID": "dapper-test", "pathFormat": "path", "linesStartAt1": true, "columnsStartAt1": true}, "seq": 1}"#.to_string();
        self.inner_client.send(initialize)?;
        self.inner_client.read_response()?;
        Ok(())
    }

    pub fn read_event(&mut self) -> Result<Event> {
        self.inner_client.read_event()
    }

    pub fn read_response(&mut self) -> Result<debugserver_types::Response> {
        self.inner_client.read_response()
    }

    pub fn send(&mut self, message: String) -> Result<()> {
        self.inner_client.send(message)
    }

    pub fn launch(&mut self, launch_arguments: serde_json::Value) -> Result<()> {
        let launch_request = format!(
            r#"{{"type": "request", "command": "launch", "arguments": {}, "seq": 2}}"#,
            launch_arguments
        );
        self.inner_client.send(launch_request)?;
        self.wait_for_event("initialized")?;

        let configuration_done =
            r#"{"type": "request", "command": "configurationDone", "seq": 6}"#.to_string();
        self.inner_client.send(configuration_done)?;
        self.inner_client.read_response()?;

        Ok(())
    }

    pub fn try_consume_pending_responses(&mut self) -> Result<()> {
        while self
            .inner_client
            .read_response_timeout(Duration::from_millis(100))
            .is_ok()
        {}
        Ok(())
    }

    pub fn wait_for_event(&mut self, event_name: &str) -> Result<Event> {
        self.wait_for_event_timeout(event_name, Duration::from_secs(60))
    }

    pub fn wait_for_event_timeout(&mut self, event_name: &str, timeout: Duration) -> Result<Event> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timed out after {:?} waiting for event '{}'",
                    timeout,
                    event_name
                );
            }
            match self.read_event() {
                Ok(event) if event.event == event_name => return Ok(event),
                Ok(_) => {}
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    pub fn kill(&mut self) -> Result<()> {
        self.inner_client.kill()
    }
}

/// Returns the adapter command supplied by the E2E runner or Buck target.
pub fn adapter_command() -> Result<AdapterCommand> {
    let executable = std::env::var("DAPPER_TEST_ADAPTER_EXECUTABLE")
        .context("DAPPER_TEST_ADAPTER_EXECUTABLE must name the DAP adapter")?;
    let arguments = match std::env::var("DAPPER_TEST_ADAPTER_ARGUMENTS") {
        Ok(arguments) => serde_json::from_str(&arguments)
            .context("DAPPER_TEST_ADAPTER_ARGUMENTS must be a JSON string array")?,
        Err(std::env::VarError::NotPresent) => Vec::new(),
        Err(error) => {
            return Err(error).context("DAPPER_TEST_ADAPTER_ARGUMENTS must be valid Unicode");
        }
    };

    Ok(AdapterCommand {
        executable,
        arguments,
    })
}

/// Creates a Dapper command with the test session directory and logging policy.
pub fn dapper_command(dapper_path: &str, subcommand: &str) -> Command {
    let mut command = Command::new(dapper_path);
    command.arg(subcommand);
    command.env(
        "DAPPER_SESSIONS_DIR",
        dapper_session::get_user_temp_dir().join("test_sessions"),
    );
    command.env("DAPPER_DISABLE_SCUBA", "1");
    command
}

/// Generates a unique scope ID for an E2E test session.
pub fn generate_test_scope_id(test_name: &str) -> ScopeId {
    format!(
        "test-{}-{}",
        test_name,
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the system clock should be after the Unix epoch")
            .as_nanos()
    )
    .into()
}

/// Starts a stopped debug session backed by the fake DAP adapter.
pub fn setup_stopped_fake_debug_session(
    test_name: &str,
    adapter_args: &[&str],
) -> Result<(ScopeId, DapClient)> {
    let scope_id = generate_test_scope_id(test_name);

    let mut dap_client = DapClient::new_with_adapter_args(Some(scope_id.clone()), adapter_args)?;
    dap_client.initialize()?;
    dap_client.launch(serde_json::json!({}))?;
    dap_client.wait_for_event("stopped")?;

    Ok((scope_id, dap_client))
}

/// Runs a Dapper debug CLI command against an optional test scope.
pub async fn run_debug_command(scope_id: Option<ScopeId>, args: &[&str]) -> Result<DebugCliOutput> {
    let dapper_path = dapper_executable()?;
    run_debug_command_with_binary(&dapper_path, scope_id, args).await
}

/// Runs a debug CLI command using an explicit Dapper executable.
pub async fn run_debug_command_with_binary(
    dapper_path: &str,
    scope_id: Option<ScopeId>,
    args: &[&str],
) -> Result<DebugCliOutput> {
    let mut command = TokioCommand::new(dapper_path);
    command.arg("debug");
    if let Some(scope) = scope_id {
        command.arg("--scope-id").arg(scope.as_str());
    }
    command.args(args);
    command.env(
        "DAPPER_SESSIONS_DIR",
        dapper_session::get_user_temp_dir().join("test_sessions"),
    );
    command.env("DAPPER_DISABLE_SCUBA", "1");
    command.kill_on_drop(true);

    let output = command
        .output()
        .await
        .context("failed to run Dapper debug command")?;

    Ok(DebugCliOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    })
}

/// Parses a progress event from a Dapper stdout line.
pub fn parse_progress_event(line: &str) -> Option<ProgressEvent> {
    line.strip_prefix("[DAPPER_SESSION] ")
        .and_then(|json| serde_json::from_str(json).ok())
}

/// Spawns a headless Dapper proxy from a generated configuration file.
pub async fn spawn_proxy_from_config(
    config: &DebugSessionConfig,
    scope_id: &ScopeId,
) -> Result<(Child, NamedTempFile, AdapterLog)> {
    let mut config_file = NamedTempFile::new()?;
    serde_json::to_writer(&mut config_file, config)?;
    config_file.flush()?;

    let dapper_path = dapper_executable()?;
    let adapter_log = AdapterLog::new()?;

    let child = TokioCommand::new(&dapper_path)
        .arg("proxy")
        .arg("--scope-id")
        .arg(scope_id.as_str())
        .arg("from-config")
        .arg(config_file.path())
        .env(
            "DAPPER_SESSIONS_DIR",
            dapper_session::get_user_temp_dir().join("test_sessions"),
        )
        .env("DAPPER_DISABLE_SCUBA", "1")
        .env("LLDBDAP_LOG", adapter_log.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn Dapper proxy")?;

    Ok((child, config_file, adapter_log))
}

fn dapper_executable() -> Result<String> {
    std::env::var("DAPPER_TEST_EXECUTABLE")
        .context("DAPPER_TEST_EXECUTABLE must name the Dapper binary")
}
