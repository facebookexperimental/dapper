// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Debug Session Configuration Types
//!
//! These types define the configuration for starting debug sessions via the
//! `from-config` proxy mode.

use std::collections::HashMap;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use bon::Builder;
use dapper_access::Grant;
use dapper_access::Policy;
use dapper_dap_protocol::requests::AttachRequestArguments;
use dapper_dap_protocol::requests::LaunchRequestArguments;
use dapper_dap_protocol::requests::RequestCommand;
use serde::Deserialize;
use serde::Serialize;

// The declarative child-session config / rule-engine types live in their own
// module; re-exported here so existing `dapper_session::config::…` paths keep
// resolving.
pub use crate::child_session::*;

/// Configuration for starting a debug session via dapper proxy. Other crates
/// build it with [`DebugSessionConfig::builder`].
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DebugSessionConfig {
    /// How to spawn/connect to the DAP backend
    pub spawn_config: SpawnConfig,
    /// The debug request to send (launch or attach).
    /// When absent, the DAP client is expected to initialize the backend.
    /// When present, dapper's internal DAP client will run the initialization
    /// for headless operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_request: Option<DebugRequest>,
    /// Initial breakpoints to set
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default)]
    pub breakpoints: Vec<BreakpointSpec>,
    /// Optional metadata (session IDs, telemetry, etc.)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[builder(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Optional override for the DAP initialize request arguments.
    /// When provided, these arguments replace the defaults entirely, except
    /// `supportsStartDebuggingRequest` and `supportsRunInTerminalRequest`,
    /// which dapper always sets from what the headless driver can honor.
    /// When absent, dapper's built-in defaults are used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialize_args: Option<serde_json::Value>,
    /// Optional timeout in seconds for DAP initialization.
    /// Defaults to 5 minutes (300s). Set higher for large coredumps.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "initTimeoutSeconds"
    )]
    pub init_timeout_secs: Option<u64>,
    /// When true (default), install the exception breakpoint filters whose
    /// `default: true` flag is set in the adapter's `initialize` response
    /// (`Capabilities.exceptionBreakpointFilters`). This matches what most
    /// IDEs auto-enable in their checkbox UIs (e.g. `uncaught` for debugpy).
    /// Set to false to opt out and only install filters explicitly listed
    /// in `breakpoints` (if any).
    ///
    /// Intentionally always serialized (no `skip_serializing_if`) so a
    /// serialize -> deserialize round-trip preserves an explicit `false`,
    /// distinguishing "user opted out" from "user accepted the default" in
    /// stored configs.
    #[serde(default = "default_install_default_exception_breakpoints")]
    #[builder(default = default_install_default_exception_breakpoints())]
    pub install_default_exception_breakpoints: bool,
    /// Configuration governing whether and how this proxy spawns child debug
    /// sessions in response to the adapter's `startDebugging` reverse requests
    /// (headless mode). Absent means no child-session support is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_sessions: Option<ChildSessionConfig>,
    /// What callers of this session's control plane may do; absent, anything.
    /// Child sessions inherit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<AccessConfig>,
}

fn default_install_default_exception_breakpoints() -> bool {
    true
}

/// What callers of a session's control plane may do. A field this dapper does
/// not know could narrow the policy, so it is an error instead of ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessConfig {
    /// The presets and operations allowed; any call that needs another
    /// operation is refused.
    pub allow: Vec<Grant>,
}

impl AccessConfig {
    pub fn policy(&self) -> Policy {
        Policy::from_grants(self.allow.iter().copied())
    }
}

impl DebugSessionConfig {
    /// The policy this session's control plane enforces.
    pub fn policy(&self) -> Policy {
        self.access
            .as_ref()
            .map_or(Policy::UNRESTRICTED, AccessConfig::policy)
    }

    /// Load a debug session configuration from a JSON file.
    ///
    /// For stdio spawn configs, `new_session` is forced to `true` so that the
    /// spawned process runs in its own session and cannot steal the terminal's
    /// foreground process group (e.g. Ctrl+C reaches dapper, not the debuggee).
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open debug session config {}", path.display()))?;
        let mut config: Self =
            serde_json::from_reader(BufReader::new(file)).with_context(|| {
                format!(
                    "Failed to parse debug session config from {}",
                    path.display()
                )
            })?;
        if let SpawnConfig::Stdio(ref mut stdio) = config.spawn_config {
            stdio.new_session = true;
        }
        Ok(config)
    }
}

/// How to spawn or connect to the DAP backend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum SpawnConfig {
    /// Spawn a process and communicate via stdio
    Stdio(StdioSpawnConfig),
    /// Connect to a running DAP server via TCP
    Tcp(TcpSpawnConfig),
    #[cfg(unix)]
    /// Connect to a running DAP server via Unix Domain Socket
    Uds(UdsSpawnConfig),
}

/// Configuration for spawning a DAP server via stdio
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioSpawnConfig {
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// When true, the spawned process is placed in a new session (via
    /// `setsid`) so that neither it nor its descendants can steal the
    /// terminal's foreground process group. This is used by `from-config`
    /// mode where Ctrl+C must reach dapper rather than the debuggee.
    #[serde(default)]
    pub new_session: bool,
}

/// Configuration for connecting to a DAP server via TCP
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpSpawnConfig {
    /// Socket address of an already-running DAP server to connect to.
    pub addr: SocketAddr,
}

#[cfg(unix)]
/// Configuration for connecting to a DAP server via Unix Domain Socket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UdsSpawnConfig {
    /// Path to the Unix Domain Socket
    pub path: PathBuf,
}

/// DAP debug request (launch or attach)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "request")]
pub enum DebugRequest {
    Launch(LaunchRequestArguments),
    Attach(AttachRequestArguments),
}

impl From<DebugRequest> for RequestCommand {
    fn from(req: DebugRequest) -> Self {
        match req {
            DebugRequest::Launch(args) => RequestCommand::Launch(args),
            DebugRequest::Attach(args) => RequestCommand::Attach(args),
        }
    }
}

/// Breakpoint specification for session configuration.
/// This is distinct from the DAP protocol's `Breakpoint` type which represents
/// a resolved breakpoint in responses.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord
)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum BreakpointSpec {
    Function {
        #[serde(alias = "functionName")]
        name: String,
    },
    Source {
        path: String,
        line: usize,
    },
    /// Exception breakpoint filter. The filter ID must match one advertised
    /// by the debug adapter in the `initialize` response's
    /// `exceptionBreakpointFilters`. The optional `condition` is sent via
    /// `filterOptions` only if the adapter advertises both
    /// `supportsExceptionFilterOptions` and the filter's `supportsCondition`;
    /// otherwise it is dropped with a warning.
    Exception {
        filter: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
    },
}

impl BreakpointSpec {
    pub fn function(name: impl Into<String>) -> Self {
        Self::Function { name: name.into() }
    }

    pub fn source(path: impl Into<String>, line: usize) -> Self {
        Self::Source {
            path: path.into(),
            line,
        }
    }

    pub fn exception(filter: impl Into<String>, condition: Option<String>) -> Self {
        Self::Exception {
            filter: filter.into(),
            condition,
        }
    }
}

impl std::fmt::Display for BreakpointSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Function { name } => f.write_str(name),
            Self::Source { path, line } => write!(f, "{path}:{line}"),
            Self::Exception {
                filter,
                condition: None,
            } => write!(f, "exception:{filter}"),
            Self::Exception {
                filter,
                condition: Some(cond),
            } => write!(f, "exception:{filter} if {cond}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use dapper_access::Operation;
    use dapper_access::Preset;

    use super::*;

    #[test]
    fn test_debug_session_config_roundtrip() {
        let config = DebugSessionConfig {
            spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
                cmd: "/usr/bin/lldb-dap".to_string(),
                args: vec!["--arg1".to_string()],
                new_session: false,
            }),
            debug_request: Some(DebugRequest::Launch(
                serde_json::from_value(serde_json::json!({
                    "program": "/path/to/binary",
                    "args": ["arg1", "arg2"]
                }))
                .unwrap(),
            )),
            breakpoints: vec![
                BreakpointSpec::function("main"),
                BreakpointSpec::source("main.cpp", 42),
            ],
            metadata: HashMap::from([("sessionId".to_string(), serde_json::json!("test-session"))]),
            initialize_args: None,
            init_timeout_secs: None,
            install_default_exception_breakpoints: false,
            child_sessions: None,
            access: None,
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: DebugSessionConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.breakpoints.len(), 2);
        assert_eq!(parsed.metadata.get("sessionId").unwrap(), "test-session");
    }

    #[test]
    fn from_file_error_names_a_missing_config() {
        let path = Path::new("/nonexistent/dapper-session-config.json");
        let err = DebugSessionConfig::from_file(path).unwrap_err();
        assert!(
            format!("{err:#}").contains("/nonexistent/dapper-session-config.json"),
            "got: {err:#}"
        );
    }

    #[test]
    fn test_debug_session_config_without_debug_request() {
        // When debug_request is absent, the DAP client is expected to initialize the backend
        let json = r#"{
            "spawnConfig": {
                "type": "stdio",
                "cmd": "/usr/bin/lldb-dap"
            }
        }"#;

        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        assert!(config.debug_request.is_none());
        assert!(config.breakpoints.is_empty());
        assert!(config.metadata.is_empty());

        // Verify serialization omits optional/empty fields
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(!serialized.contains("debugRequest"));
        assert!(!serialized.contains("breakpoints"));
        assert!(!serialized.contains("metadata"));
    }

    #[test]
    fn test_spawn_config_serialization() {
        let stdio = SpawnConfig::Stdio(StdioSpawnConfig {
            cmd: "lldb-dap".to_string(),
            args: vec![],
            new_session: false,
        });
        let json = serde_json::to_string(&stdio).unwrap();
        assert!(json.contains(r#""type":"stdio"#));

        let tcp = SpawnConfig::Tcp(TcpSpawnConfig {
            addr: "127.0.0.1:4711".parse().unwrap(),
        });
        let json = serde_json::to_string(&tcp).unwrap();
        assert!(json.contains(r#""type":"tcp"#));
        assert!(json.contains(r#""addr":"127.0.0.1:4711""#));

        #[cfg(unix)]
        {
            let uds = SpawnConfig::Uds(UdsSpawnConfig {
                path: PathBuf::from("/tmp/debug.sock"),
            });
            let json = serde_json::to_string(&uds).unwrap();
            assert!(json.contains(r#""type":"uds"#));
            assert!(json.contains(r#""path":"/tmp/debug.sock""#));
        }
    }

    #[test]
    fn test_tcp_spawn_config_ignores_legacy_cmd_args() {
        // Configs written when TcpSpawnConfig still required cmd/args must keep
        // parsing after their removal: the keys are simply ignored.
        let json = r#"{
            "spawnConfig": {
                "type": "tcp",
                "cmd": "/usr/bin/lldb-dap",
                "args": ["--port", "4711"],
                "addr": "127.0.0.1:4711"
            }
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        match config.spawn_config {
            SpawnConfig::Tcp(tcp) => {
                assert_eq!(tcp.addr, "127.0.0.1:4711".parse().unwrap());
            }
            other => panic!("expected TCP spawn config, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_uds_spawn_config_deserialization() {
        let json = r#"{
            "spawnConfig": {
                "type": "uds",
                "path": "/var/run/debug.sock"
            }
        }"#;

        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        match config.spawn_config {
            SpawnConfig::Uds(cfg) => {
                assert_eq!(cfg.path, PathBuf::from("/var/run/debug.sock"));
            }
            _ => panic!("Expected UDS spawn config"),
        }
    }

    #[test]
    fn test_debug_request_serialization() {
        let launch = DebugRequest::Launch(
            serde_json::from_value(serde_json::json!({"program": "/bin/test"})).unwrap(),
        );
        let json = serde_json::to_string(&launch).unwrap();
        assert!(json.contains(r#""request":"launch"#));
        let roundtrip: DebugRequest = serde_json::from_str(&json).unwrap();
        match roundtrip {
            DebugRequest::Launch(args) => {
                assert_eq!(args.extra.get("program").unwrap(), "/bin/test");
                assert!(args.extra.get("request").is_none());
            }
            _ => panic!("Expected Launch variant"),
        }

        let attach =
            DebugRequest::Attach(serde_json::from_value(serde_json::json!({"pid": 1234})).unwrap());
        let json = serde_json::to_string(&attach).unwrap();
        assert!(json.contains(r#""request":"attach"#));
        let roundtrip: DebugRequest = serde_json::from_str(&json).unwrap();
        match roundtrip {
            DebugRequest::Attach(args) => {
                assert_eq!(args.extra.get("pid").unwrap(), 1234);
                assert!(args.extra.get("request").is_none());
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_breakpoint_spec_display() {
        assert_eq!(BreakpointSpec::function("main").to_string(), "main");
        assert_eq!(
            BreakpointSpec::source("test.cpp", 42).to_string(),
            "test.cpp:42"
        );
        assert_eq!(
            BreakpointSpec::exception("raised", None).to_string(),
            "exception:raised"
        );
        assert_eq!(
            BreakpointSpec::exception("raised", Some("x > 5".to_string())).to_string(),
            "exception:raised if x > 5"
        );
    }

    #[test]
    fn test_breakpoint_spec_exception_serde_roundtrip() {
        let plain = BreakpointSpec::exception("uncaught", None);
        let json = serde_json::to_string(&plain).unwrap();
        assert_eq!(json, r#"{"type":"exception","filter":"uncaught"}"#);
        let parsed: BreakpointSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, plain);

        let conditional = BreakpointSpec::exception("raised", Some("x>5".to_string()));
        let json = serde_json::to_string(&conditional).unwrap();
        assert_eq!(
            json,
            r#"{"type":"exception","filter":"raised","condition":"x>5"}"#
        );
        let parsed: BreakpointSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, conditional);
    }

    #[test]
    fn test_install_default_exception_breakpoints_defaults_to_true_on_missing() {
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" }
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        assert!(config.install_default_exception_breakpoints);
    }

    #[test]
    fn test_install_default_exception_breakpoints_explicit_false() {
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
            "installDefaultExceptionBreakpoints": false
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        assert!(!config.install_default_exception_breakpoints);

        // Field is always serialized (no skip-if), so the explicit false
        // round-trips through serialize -> deserialize without ambiguity.
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(
            serialized.contains(r#""installDefaultExceptionBreakpoints":false"#),
            "expected explicit false in serialized JSON: {serialized}"
        );
        let reparsed: DebugSessionConfig = serde_json::from_str(&serialized).unwrap();
        assert!(!reparsed.install_default_exception_breakpoints);
    }

    #[test]
    fn test_init_timeout_secs_deserialization() {
        // When specified, it should be parsed
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
            "initTimeoutSecs": 600
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.init_timeout_secs, Some(600));

        // When omitted, it should default to None
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" }
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.init_timeout_secs, None);

        // Serialization should omit None
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(!serialized.contains("initTimeoutSecs"));

        // Serialization should include Some
        let config_with_timeout = DebugSessionConfig {
            init_timeout_secs: Some(120),
            ..config
        };
        let serialized = serde_json::to_string(&config_with_timeout).unwrap();
        assert!(serialized.contains("\"initTimeoutSecs\":120"));

        // Legacy spelling "initTimeoutSeconds" should also deserialize
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
            "initTimeoutSeconds": 900
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.init_timeout_secs, Some(900));
    }

    fn parse(json: serde_json::Value) -> serde_json::Result<DebugSessionConfig> {
        serde_json::from_value(json)
    }

    #[test]
    fn a_config_without_access_allows_everything() {
        let config = parse(serde_json::json!({
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" }
        }))
        .expect("a config without access should parse");
        assert_eq!(config.access, None);
        assert_eq!(config.policy(), Policy::UNRESTRICTED);
        assert!(
            !serde_json::to_string(&config)
                .expect("a config should serialize")
                .contains("access"),
            "an absent policy should stay absent when the config is written for a child"
        );
    }

    #[test]
    fn a_config_allows_what_its_access_grants() {
        let config = parse(serde_json::json!({
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
            "access": { "allow": ["standard", "read_memory"] }
        }))
        .expect("known grants should parse");
        assert_eq!(
            config.policy().allowed(),
            Preset::Standard.operations() | Operation::ReadMemory
        );
        let written = serde_json::to_value(&config).expect("a config should serialize");
        assert_eq!(
            written["access"],
            serde_json::json!({ "allow": ["standard", "read_memory"] }),
            "a child session's config should carry the same allow list"
        );
    }

    #[test]
    fn an_unreadable_access_fails_the_config() {
        for access in [
            serde_json::json!({ "allow": ["everything"] }),
            serde_json::json!({ "allow": ["minimal"], "deny": ["read_values"] }),
            serde_json::json!({}),
        ] {
            assert!(
                parse(serde_json::json!({
                    "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
                    "access": access.clone()
                }))
                .is_err(),
                "{access} should fail the config instead of being ignored"
            );
        }
    }

    #[test]
    fn an_access_grants_what_its_operations_require() {
        let config = parse(serde_json::json!({
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
            "access": { "allow": ["structure", "evaluate"] }
        }))
        .expect("known grants should parse");
        assert_eq!(
            config.policy().allowed(),
            Operation::Inspect | Operation::Evaluate | Operation::ReadValues,
            "evaluate comes with read_values, which it reveals"
        );
    }

    #[test]
    fn builder_defaults_match_serde_defaults() {
        let spawn_config = SpawnConfig::Tcp(TcpSpawnConfig {
            addr: "127.0.0.1:4711".parse().expect("a valid socket address"),
        });
        let built = DebugSessionConfig::builder()
            .spawn_config(spawn_config.clone())
            .build();
        let parsed: DebugSessionConfig =
            serde_json::from_value(serde_json::json!({ "spawnConfig": spawn_config }))
                .expect("a config with only a backend should parse");
        assert_eq!(
            serde_json::to_value(&built).expect("a config should serialize"),
            serde_json::to_value(&parsed).expect("a config should serialize"),
            "a field left unset should get the same default from the builder as from serde"
        );
    }
}
