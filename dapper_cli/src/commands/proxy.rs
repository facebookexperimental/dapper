// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use clap::Subcommand;
use dapper_config::DapperConfig;
use dapper_control_server as control_plane_server;
use dapper_dap_protocol::protocol::Request;
use dapper_dap_protocol::requests::DisconnectArguments;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_proxy_server::Backend;
use dapper_proxy_server::ClientId;
use dapper_proxy_server::DuplexChannel;
use dapper_proxy_server::EventWriter;
use dapper_proxy_server::ProxyServer;
use dapper_proxy_server::SessionInitializer;
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::SessionStore;
use dapper_session::config::DebugSessionConfig;
use dapper_session::config::SpawnConfig;
use dapper_session::config::StdioSpawnConfig;
use dapper_session::config::TcpSpawnConfig;
#[cfg(unix)]
use dapper_session::config::UdsSpawnConfig;

fn parse_socket_addr(addr_str: &str) -> anyhow::Result<SocketAddr> {
    // Fast path: a literal socket address (covers bracketed IPv6 like `[::1]:8080`).
    if let Ok(addr) = addr_str.parse::<SocketAddr>() {
        return Ok(addr);
    }
    // Otherwise split `<host>:<port>` and resolve via the shared helper, which
    // runs ToSocketAddrs on the (host, port) tuple — no manual host:port string
    // formatting (which would mishandle IPv6 literals).
    let (host, port_str) = addr_str.rsplit_once(':').ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to parse address `{}`. Expected: <host>:<port>.",
            addr_str
        )
    })?;
    let port: u16 = port_str
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid port in address `{}`", addr_str))?;
    // Strip brackets from a bracketed IPv6 literal. An unbracketed host still
    // containing `:` is an ambiguous bare IPv6 literal (`rsplit_once` mis-splits
    // it), so reject it — IPv6 must be bracketed.
    let host = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(stripped) => stripped,
        None if host.contains(':') => {
            anyhow::bail!(
                "Failed to parse address `{}`. Bracket IPv6 literals, e.g. `[::1]:8080`.",
                addr_str
            );
        }
        None => host,
    };
    dapper_session::resolve_socket_addr(host, port)
}

#[derive(Subcommand, Debug)]
enum BackendMode {
    /// Start a debug adapter process and communicate via stdio
    Process {
        /// Command and arguments to start the debug adapter
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Connect to a debug adapter server over TCP
    Tcp {
        /// Socket address to connect to
        #[arg(value_parser = parse_socket_addr)]
        addr: SocketAddr,
    },
    /// Connect to a debug adapter server over Unix Domain Socket
    #[cfg(unix)]
    Uds {
        /// Path to the Unix Domain Socket
        path: PathBuf,
    },
    /// Read configuration from a file and start the appropriate backend
    FromConfig {
        /// Path to the configuration file (JSON)
        config_file: PathBuf,
        /// File descriptor to write progress events to as JSON lines.
        /// When set, events are written to this fd instead of stdout.
        #[cfg(unix)]
        #[arg(long)]
        events_fd: Option<i32>,
    },
}

impl BackendMode {
    #[cfg(unix)]
    fn events_fd(&self) -> Option<i32> {
        match self {
            BackendMode::FromConfig { events_fd, .. } => *events_fd,
            _ => None,
        }
    }

    /// Convert this BackendMode into a DebugSessionConfig.
    fn into_session_config(self) -> anyhow::Result<DebugSessionConfig> {
        match self {
            BackendMode::Process { cmd } => {
                let (first, rest) = cmd
                    .split_first()
                    .ok_or_else(|| anyhow::anyhow!("Process command cannot be empty"))?;
                Ok(DebugSessionConfig {
                    spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
                        cmd: first.clone(),
                        args: rest.to_vec(),
                        new_session: false,
                    }),
                    debug_request: None,
                    breakpoints: Vec::new(),
                    metadata: std::collections::HashMap::new(),
                    initialize_args: None,
                    init_timeout_secs: None,
                    install_default_exception_breakpoints: true,
                    child_sessions: None,
                })
            }
            BackendMode::Tcp { addr } => Ok(DebugSessionConfig {
                spawn_config: SpawnConfig::Tcp(TcpSpawnConfig {
                    cmd: PathBuf::new(),
                    args: Vec::new(),
                    addr,
                }),
                debug_request: None,
                breakpoints: Vec::new(),
                metadata: std::collections::HashMap::new(),
                initialize_args: None,
                init_timeout_secs: None,
                install_default_exception_breakpoints: true,
                child_sessions: None,
            }),
            #[cfg(unix)]
            BackendMode::Uds { path } => Ok(DebugSessionConfig {
                spawn_config: SpawnConfig::Uds(UdsSpawnConfig { path }),
                debug_request: None,
                breakpoints: Vec::new(),
                metadata: std::collections::HashMap::new(),
                initialize_args: None,
                init_timeout_secs: None,
                install_default_exception_breakpoints: true,
                child_sessions: None,
            }),
            BackendMode::FromConfig { config_file, .. } => {
                DebugSessionConfig::from_file(&config_file)
            }
        }
    }
}

/// Start the proxy for the specified backend DAP server
#[derive(Parser, Debug)]
pub struct Proxy {
    /// Port to start the control plane on
    #[arg(long, default_value_t = crate::DAPPER_CONTROL_PLANE_PORT)]
    control_port: u16,
    /// Port for the DAP client, such as IDE, to connect to via TCP (if not specified, uses stdio)
    #[arg(long)]
    client_port: Option<u16>,
    /// Scope identifier for this proxy session
    #[arg(long)]
    scope_id: Option<ScopeId>,
    /// The parent proxy's session id, set when this proxy is spawned as a child
    /// of another (headless `startDebugging`) session. Recorded in this
    /// session's `SessionInfo` so the parent/child relationship is discoverable.
    #[arg(long)]
    parent_session_id: Option<SessionId>,
    /// Backend configuration
    #[command(subcommand)]
    backend: BackendMode,
}

async fn create_backend(spawn_config: &SpawnConfig) -> anyhow::Result<Backend> {
    match spawn_config {
        SpawnConfig::Stdio(cfg) => {
            let mut cmd = vec![cfg.cmd.clone()];
            cmd.extend(cfg.args.clone());
            tracing::info!("Starting debug adapter process: {:?}", cmd);
            Backend::from_process(&cmd, cfg.new_session).await
        }
        SpawnConfig::Tcp(cfg) => {
            // TODO: Spawn the process first if cmd is specified, then connect
            tracing::info!("Connecting to debug adapter at {}", cfg.addr);
            Backend::from_tcp(&cfg.addr.ip().to_string(), cfg.addr.port()).await
        }
        #[cfg(unix)]
        SpawnConfig::Uds(cfg) => {
            tracing::info!("Connecting to debug adapter at Unix socket: {:?}", cfg.path);
            Backend::from_uds(&cfg.path).await
        }
    }
}

impl Proxy {
    pub async fn run(self, session_id: &SessionId, config: DapperConfig) -> anyhow::Result<()> {
        let control_port = Port::try_new(self.control_port);
        tracing::info!(self.control_port, "Starting dapper proxy");
        let sessions = match SessionStore::default_location() {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::warn!(
                    "Sessions directory unavailable ({e}); session file will not be written"
                );
                None
            }
        };

        // SAFETY-CRITICAL ORDERING: consume `--events-fd` here, BEFORE
        // `create_backend` spawns the debug adapter. `EventWriter::from_raw_fd`
        // re-sets `FD_CLOEXEC` on the inherited write end (the child supervisor
        // clears it in `pre_exec` so this proxy inherits it); if the adapter were
        // spawned first, that CLOEXEC-cleared fd would leak into the adapter (a
        // grandchild), and the supervisor's read end would never see EOF until
        // the whole descendant subtree exited — defeating per-child reaping. Keep
        // this above `create_backend`.
        #[cfg(unix)]
        let event_writer = match self.backend.events_fd() {
            Some(fd) => EventWriter::from_raw_fd(fd)?,
            None => EventWriter::stdout(),
        };
        #[cfg(not(unix))]
        let event_writer = EventWriter::stdout();

        let session_config = self.backend.into_session_config()?;
        tracing::debug!("Session config: {:?}", session_config);

        let backend = create_backend(&session_config.spawn_config).await?;

        // Create the proxy server
        let proxy_server = ProxyServer::new(
            backend,
            config,
            sessions.clone(),
            session_id.clone(),
            self.parent_session_id.clone(),
        );
        let control_client = proxy_server.create_client(ClientId::new("control-plane"));

        // Teardown hook for child sessions (set below for headless + Unix +
        // autoSpawn). Used by the control-plane `stop` and the shutdown cascade so
        // children are torn down before the parent.
        let mut child_teardown_hook: Option<control_plane_server::ChildTeardownHook> = None;

        // Create DAP client based on mode:
        // - Headless mode (debug_request present): use in-memory channel with SessionInitializer
        // - Normal mode: use TCP server or stdio for external IDE client
        let (dap_client, initializer_handle) = if session_config.debug_request.is_some() {
            // Headless mode: create in-memory channel pair
            tracing::info!("Running in headless mode (no external DAP client)");
            let (server_channel, client_channel) = DuplexChannel::in_memory(64 * 1024);

            // Spawn SessionInitializer with the client side of the channel
            let init_config = session_config.clone();
            // Wire the child-session supervisor (Unix-only; gated on autoSpawn).
            // On non-Unix or when autoSpawn is off, there is no channel and
            // `startDebugging` reverse requests fail closed.
            #[cfg(unix)]
            let child_spawn_tx = match crate::child_supervisor::setup_child_supervisor(
                &session_config,
                session_id,
                self.scope_id.clone(),
            ) {
                Some((tx, teardown)) => {
                    child_teardown_hook = Some(crate::child_supervisor::teardown_hook(teardown));
                    Some(tx)
                }
                None => None,
            };
            #[cfg(not(unix))]
            let child_spawn_tx: Option<
                tokio::sync::mpsc::Sender<dapper_proxy_server::ChildSpawnRequest>,
            > = None;
            let handle = tokio::spawn(async move {
                let mut initializer =
                    SessionInitializer::new(init_config).with_event_writer(event_writer);
                if let Some(timeout_secs) = session_config.init_timeout_secs {
                    initializer = initializer.with_timeout(Duration::from_secs(timeout_secs));
                }
                if let Some(tx) = child_spawn_tx {
                    initializer = initializer.with_child_spawn_tx(tx);
                }
                if let Err(e) = initializer.run(client_channel).await {
                    tracing::error!("DAP initialization failed: {}", e);
                    return Err(e);
                }
                Ok(())
            });

            (server_channel, Some(handle))
        } else if let Some(tcp_port) = self.client_port {
            tracing::info!("Starting TCP server for DAP client on port {}", tcp_port);
            (DuplexChannel::from_tcp_server(tcp_port).await?, None)
        } else {
            (DuplexChannel::from_stdio(), None)
        };

        let proxy_server_handle = tokio::spawn(proxy_server.run(dap_client));
        let proxy_server_abort = proxy_server_handle.abort_handle();

        let cleanup_client = control_client.clone();

        // Start the control plane server
        let control_plane_result = control_plane_server::start_control_plane(
            control_port,
            control_client,
            proxy_server_abort.clone(),
            session_id,
            self.scope_id.clone(),
            child_teardown_hook.clone(),
        )
        .await;

        let control_plane_task = match control_plane_result {
            Ok(server) => {
                tracing::info!("Control plane started on port: {}", server.port);
                Some(server.handle)
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to start control plane: {}. Continuing without control plane.",
                    e
                );
                None
            }
        };

        #[cfg(unix)]
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        let result = tokio::select! {
            result = proxy_server_handle => result,
            result = async {
                match initializer_handle {
                    Some(handle) => handle.await,
                    None => std::future::pending().await,
                }
            } => {
                match result {
                    Ok(Ok(())) => Ok(Ok(())),
                    Ok(Err(e)) => {
                        tracing::error!("DAP initialization failed, shutting down proxy: {}", e);
                        Self::graceful_shutdown(
                            &cleanup_client,
                            &proxy_server_abort,
                            child_teardown_hook.as_ref(),
                        )
                        .await;
                        Ok(Err(e))
                    }
                    Err(e) => {
                        tracing::error!("DAP initializer task panicked, shutting down proxy: {}", e);
                        Self::graceful_shutdown(
                            &cleanup_client,
                            &proxy_server_abort,
                            child_teardown_hook.as_ref(),
                        )
                        .await;
                        Err(e)
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received SIGINT, shutting down gracefully...");
                Self::graceful_shutdown(
                    &cleanup_client,
                    &proxy_server_abort,
                    child_teardown_hook.as_ref(),
                )
                .await;
                Ok(Ok(()))
            }
            _ = async {
                #[cfg(unix)]
                sigterm.recv().await;
                #[cfg(not(unix))]
                std::future::pending::<()>().await;
            } => {
                tracing::info!("Received SIGTERM, shutting down gracefully...");
                Self::graceful_shutdown(
                    &cleanup_client,
                    &proxy_server_abort,
                    child_teardown_hook.as_ref(),
                )
                .await;
                Ok(Ok(()))
            }
        };

        // Catch-all: ensure children are torn down on any exit path (e.g. the
        // proxy server task ending on its own, which doesn't go through
        // graceful_shutdown). Idempotent with the graceful/stop paths above.
        if let Some(teardown) = &child_teardown_hook {
            teardown().await;
        }

        // Cancel control plane when proxy completes
        if let Some(task) = control_plane_task {
            task.abort();
        }

        // Clean up session file if it was created
        if let (Some(store), Some(session)) = (
            &sessions,
            cleanup_client.debug_session_tracker().get_session_info(),
        ) {
            if let Err(e) = store.delete(&session) {
                tracing::warn!("{}", e);
            } else {
                tracing::info!("Session file cleaned up successfully");
            }
        }

        match result {
            Ok(Ok(())) => tracing::info!("Proxy server completed successfully"),
            Ok(Err(e)) => tracing::error!("Proxy server error: {}", e),
            Err(e) => tracing::error!("Proxy server task error: {}", e),
        }
        Ok(())
    }

    const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    /// Send a DAP disconnect to the backend debugger, then abort the proxy
    /// server task. Fields like `terminate_debuggee` and `suspend_debuggee`
    /// are omitted so the debug adapter applies its own default behavior.
    async fn graceful_shutdown(
        proxy_client: &dapper_proxy_server::ProxyClient,
        proxy_server_abort: &tokio::task::AbortHandle,
        child_teardown: Option<&control_plane_server::ChildTeardownHook>,
    ) {
        // Children-before-parent: tear down child sessions before disconnecting
        // the parent's own adapter.
        if let Some(teardown) = child_teardown {
            teardown().await;
        }
        let request = Request::new(RequestCommand::Disconnect(Some(
            DisconnectArguments::default(),
        )));
        if let Err(e) = proxy_client
            .send_message_with_timeout(request.into(), Self::SHUTDOWN_TIMEOUT)
            .await
        {
            tracing::warn!("DAP disconnect request failed during shutdown: {}", e);
        }
        proxy_server_abort.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::net::Ipv4Addr;
    use std::net::SocketAddr;

    use clap::Parser;

    use super::*;

    #[test]
    fn test_parse_valid_socket_addr() {
        assert_eq!(
            parse_socket_addr("127.0.0.1:8080").unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080)
        );
        assert_eq!(
            parse_socket_addr("255.255.255.255:22").unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), 22)
        );
    }

    #[test]
    fn test_parse_hostname_socket_addr() {
        // Test localhost resolution
        let addr = parse_socket_addr("localhost:8080").unwrap();
        assert!(addr.ip() == Ipv4Addr::LOCALHOST || addr.ip() == std::net::Ipv6Addr::LOCALHOST);
        assert_eq!(addr.port(), 8080);
    }

    #[test]
    fn test_parse_socket_addr_invalid_format() {
        assert!(parse_socket_addr("127.0.0.1").is_err()); // Missing port
        assert!(parse_socket_addr("invalid").is_err()); // Invalid name
        assert!(parse_socket_addr("8080").is_err()); // Missing host
        // ":8080" is resolved successfully on Windows via to_socket_addrs,
        // so we only assert it's invalid on Unix.
        #[cfg(unix)]
        assert!(parse_socket_addr(":8080").is_err()); // Has separator but missing host
        // A bare, unbracketed IPv6 literal is ambiguous once split on `:` and
        // must be rejected rather than silently mis-split (e.g. `::1` -> `[::]:1`).
        assert!(parse_socket_addr("::1").is_err());
        assert!(parse_socket_addr("fe80::1").is_err());
    }

    #[test]
    fn test_parse_bracketed_ipv6_socket_addr() {
        // Bracketed IPv6 literals with a port parse correctly (via the fast path).
        let addr = parse_socket_addr("[::1]:8080").unwrap();
        assert_eq!(addr.ip(), std::net::Ipv6Addr::LOCALHOST);
        assert_eq!(addr.port(), 8080);
    }

    #[test]
    fn test_parse_socket_addr_invalid_port() {
        assert!(parse_socket_addr("127.0.0.1:99999").is_err());
    }

    #[test]
    fn test_proxy_tcp_command_parsing() {
        let args = vec!["proxy", "--scope-id", "test-scope", "tcp", "127.0.0.1:8080"];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Tcp { addr } => {
                assert_eq!(
                    addr,
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080)
                );
            }
            _ => panic!("Expected TCP backend mode"),
        }
        assert_eq!(proxy.scope_id, Some(ScopeId::new("test-scope")));
    }

    #[test]
    fn test_proxy_tcp_command_invalid_address() {
        // On Windows, ":8080" is resolved successfully by to_socket_addrs,
        // so this test only validates the error path on Unix.
        #[cfg(unix)]
        {
            let args = vec!["proxy", "--scope-id", "test-scope", "tcp", ":8080"];
            let result = Proxy::try_parse_from(args);
            assert!(result.is_err());

            let error_msg = result.unwrap_err().to_string();
            assert!(error_msg.contains("failed to lookup address information"));
        }

        // Use a clearly invalid address that fails on all platforms.
        let args = vec![
            "proxy",
            "--scope-id",
            "test-scope",
            "tcp",
            "not_a_valid_addr",
        ];
        let result = Proxy::try_parse_from(args);
        assert!(result.is_err());
    }

    #[test]
    fn test_proxy_tcp_command_missing_address() {
        let args = vec!["proxy", "--scope-id", "test-scope", "tcp"];
        let result = Proxy::try_parse_from(args);
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("required"));
    }

    #[test]
    fn test_proxy_process_command_parsing() {
        let args = vec![
            "proxy",
            "--scope-id",
            "test-scope",
            "process",
            "lldb-dap",
            "arg1",
            "value1",
        ];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Process { cmd } => {
                assert_eq!(
                    cmd,
                    vec![
                        "lldb-dap".to_string(),
                        "arg1".to_string(),
                        "value1".to_string()
                    ]
                );
            }
            _ => panic!("Expected Process backend mode"),
        }
        assert_eq!(proxy.scope_id, Some(ScopeId::new("test-scope")));
    }

    #[cfg(unix)]
    #[test]
    fn test_proxy_uds_command_parsing() {
        let args = vec![
            "proxy",
            "--scope-id",
            "test-scope",
            "uds",
            "/tmp/debug.sock",
        ];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Uds { path } => {
                assert_eq!(path, PathBuf::from("/tmp/debug.sock"));
            }
            _ => panic!("Expected UDS backend mode"),
        }
        assert_eq!(proxy.scope_id, Some(ScopeId::new("test-scope")));
    }

    #[cfg(unix)]
    #[test]
    fn test_proxy_uds_command_missing_path() {
        let args = vec!["proxy", "--scope-id", "test-scope", "uds"];
        let result = Proxy::try_parse_from(args);
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("required"));
    }

    #[test]
    fn test_scope_id_optional() {
        // Test that scope_id can be omitted
        let args = vec!["proxy", "tcp", "127.0.0.1:8080"];
        let proxy = Proxy::try_parse_from(args).unwrap();
        assert_eq!(proxy.scope_id, None);

        match proxy.backend {
            BackendMode::Tcp { addr } => {
                assert_eq!(
                    addr,
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080)
                );
            }
            _ => panic!("Expected TCP backend mode"),
        }
    }

    #[test]
    fn test_proxy_process_with_double_dash_args() {
        // Test that arguments starting with -- are captured as part of cmd
        let args = vec![
            "proxy",
            "--scope-id",
            "test-scope",
            "process",
            "/path/to/debugger",
            "--interpreter=vscode",
            "--configDir=/some/path",
        ];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Process { cmd } => {
                assert_eq!(cmd.len(), 3);
                assert_eq!(cmd[0], "/path/to/debugger");
                assert_eq!(cmd[1], "--interpreter=vscode");
                assert_eq!(cmd[2], "--configDir=/some/path");
            }
            _ => panic!("Expected Process backend mode"),
        }
        assert_eq!(proxy.scope_id, Some(ScopeId::new("test-scope")));
    }

    #[test]
    fn test_proxy_process_with_single_dash_args() {
        // Test that arguments starting with - are captured as part of cmd
        let args = vec![
            "proxy",
            "--scope-id",
            "test-scope",
            "process",
            "my-debugger",
            "-v",
            "-port",
            "8080",
        ];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Process { cmd } => {
                assert_eq!(cmd.len(), 4);
                assert_eq!(cmd[0], "my-debugger");
                assert_eq!(cmd[1], "-v");
                assert_eq!(cmd[2], "-port");
                assert_eq!(cmd[3], "8080");
            }
            _ => panic!("Expected Process backend mode"),
        }
    }

    #[test]
    fn test_proxy_process_with_mixed_args() {
        // Test that a mix of regular and hyphenated args are all captured
        let args = vec![
            "proxy",
            "--scope-id",
            "test-scope",
            "process",
            "debugger.exe",
            "positional_arg",
            "--flag=value",
            "-s",
            "another_positional",
            "--another-flag",
        ];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Process { cmd } => {
                assert_eq!(cmd.len(), 6);
                assert_eq!(cmd[0], "debugger.exe");
                assert_eq!(cmd[1], "positional_arg");
                assert_eq!(cmd[2], "--flag=value");
                assert_eq!(cmd[3], "-s");
                assert_eq!(cmd[4], "another_positional");
                assert_eq!(cmd[5], "--another-flag");
            }
            _ => panic!("Expected Process backend mode"),
        }
    }

    #[test]
    fn test_proxy_process_vsdbg_real_world_command() {
        // Test the real-world vsdbg command that motivated this change
        let args = vec![
            "proxy",
            "--scope-id",
            "vscode-54196",
            "--control-port",
            "0",
            "process",
            "c:\\path\\to\\vsdbg.exe",
            "--interpreter=vscode",
            "--extConfigDir=C:\\Users\\user\\.cppvsdbg\\extensions",
        ];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Process { cmd } => {
                assert_eq!(cmd.len(), 3);
                assert_eq!(cmd[0], "c:\\path\\to\\vsdbg.exe");
                assert_eq!(cmd[1], "--interpreter=vscode");
                assert_eq!(
                    cmd[2],
                    "--extConfigDir=C:\\Users\\user\\.cppvsdbg\\extensions"
                );
            }
            _ => panic!("Expected Process backend mode"),
        }
        assert_eq!(proxy.scope_id, Some(ScopeId::new("vscode-54196")));
        assert_eq!(proxy.control_port, 0);
    }

    #[test]
    fn test_proxy_process_with_explicit_double_dash_separator() {
        // Test that explicit -- separator still works correctly
        let args = vec![
            "proxy",
            "--scope-id",
            "test-scope",
            "process",
            "--",
            "debugger",
            "--some-flag",
        ];
        let proxy = Proxy::try_parse_from(args).unwrap();

        match proxy.backend {
            BackendMode::Process { cmd } => {
                assert_eq!(cmd.len(), 2);
                assert_eq!(cmd[0], "debugger");
                assert_eq!(cmd[1], "--some-flag");
            }
            _ => panic!("Expected Process backend mode"),
        }
    }

    #[test]
    fn test_proxy_process_empty_cmd_fails() {
        // Test that process subcommand requires at least one argument
        let args = vec!["proxy", "--scope-id", "test-scope", "process"];
        let result = Proxy::try_parse_from(args);
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("required"));
    }
}
