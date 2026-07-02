// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#![warn(clippy::all)]

use std::env;
use std::fmt;
use std::fs::File;
use std::fs::{self};
use std::io::BufWriter;
use std::io::Write;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::process;

use anyhow::Context;
use chrono::DateTime;
use chrono::Local;
// `dirs` is only referenced by the non-test `get_sessions_dir`, so the unittest
// target compiles `cfg(test)` code where it appears unused. Reference it in test
// builds too so RUSTUNUSEDDEPS does not flag it as an unused dependency.
#[cfg(test)]
use dirs as _;
use serde::Deserialize;
use serde::Serialize;
use serde_with::serde_as;
use strum::Display;

pub mod child_session;
pub mod config;

mod port;
pub use port::Port;

mod session_id;
pub use session_id::SessionId;

mod scope_id;
pub use scope_id::ScopeId;

pub fn get_user_temp_dir() -> PathBuf {
    let base = env::temp_dir();
    #[cfg(unix)]
    {
        let username = env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        base.join(format!("dapper-{}", username))
    }
    #[cfg(not(unix))]
    {
        base.join("dapper")
    }
}

/// Resolve a `(host, port)` pair to a `SocketAddr` via the standard
/// `ToSocketAddrs` resolution on the tuple. This correctly handles IPv4, IPv6
/// (including literals like `::1`), and hostnames, without any manual
/// `host:port` string formatting — which would mishandle bare IPv6 literals.
pub fn resolve_socket_addr(host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    // Propagate the underlying resolver error directly (no `with_context`): some
    // callers — notably the CLI address parser and its tests — rely on the raw
    // "failed to lookup address information" message surfacing through `Display`
    // (clap renders value-parser errors via `Display`, dropping the source chain).
    (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("No socket address resolved for {host}:{port}"))
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum RequestType {
    Launch,
    Attach,
}

/// Session information for a dapper proxy instance
#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub pid: u32,
    pub control_plane_port: Option<Port>,
    pub started_at: i64,
    pub command_line_args: Vec<String>,
    pub current_working_directory: Option<PathBuf>,
    pub scope_id: Option<ScopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_type: Option<RequestType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debuggee_process_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debugger_args: Option<serde_json::Value>,
    /// The session id of the parent proxy that spawned this session as a child
    /// (in response to a `startDebugging` reverse request). `None` for root
    /// sessions started directly by a user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
}

impl SessionInfo {
    pub fn generate(
        session_id: SessionId,
        control_plane_port: Option<Port>,
        scope_id: Option<ScopeId>,
        request_type: Option<RequestType>,
        debugger_args: Option<serde_json::Value>,
    ) -> Self {
        let pid = process::id();
        let started_at = Local::now().timestamp();
        let command_line_args = env::args().collect();
        let current_working_directory = env::current_dir().ok();

        let session_type = debugger_args
            .as_ref()
            .and_then(|args| args.get("type"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let program_path = debugger_args
            .as_ref()
            .and_then(|args| args.get("program"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let debuggee_process_id = debugger_args
            .as_ref()
            .and_then(|args| args.get("processId"))
            .and_then(|v| v.as_i64());

        SessionInfo {
            session_id,
            pid,
            control_plane_port,
            started_at,
            command_line_args,
            current_working_directory,
            scope_id,
            request_type,
            session_type,
            program_path,
            debuggee_process_id,
            debugger_args,
            parent_session_id: None,
        }
    }

    /// Set the parent session id (the proxy that spawned this one as a child).
    /// `None` for root sessions.
    pub fn with_parent_session_id(mut self, parent_session_id: Option<SessionId>) -> Self {
        self.parent_session_id = parent_session_id;
        self
    }

    pub fn write_to_file(&self) -> anyhow::Result<PathBuf> {
        let file_path = self.get_file_path()?;

        // Ensure the directory exists before writing
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create session dir: {}", parent.display()))?;
        }

        // Write to a temporary file first, then atomically rename into place.
        // This prevents concurrent readers (iter_sessions) from seeing an
        // empty or partially-written file and deleting it.
        let tmp_path = file_path.with_extension("json.tmp");
        let file = File::create(&tmp_path)
            .with_context(|| format!("Failed to create session file: {}", tmp_path.display()))?;

        let mut writer = BufWriter::new(file);
        let write_result =
            serde_json::to_writer(&mut writer, self).context("Failed to serialize session info");
        let flush_result =
            write_result.and_then(|()| writer.flush().context("Failed to flush session file"));

        if let Err(e) = flush_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }

        if let Err(e) = fs::rename(&tmp_path, &file_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(e).with_context(|| {
                format!(
                    "Failed to rename {} -> {}",
                    tmp_path.display(),
                    file_path.display()
                )
            });
        }

        Ok(file_path)
    }

    pub fn delete_file(&self) -> anyhow::Result<()> {
        let file_path = self.get_file_path()?;

        match fs::remove_file(&file_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(anyhow::anyhow!("Failed to delete session file: {}", e)),
        }
    }

    #[cfg(not(test))]
    fn get_sessions_dir() -> anyhow::Result<PathBuf> {
        if let Ok(env_dir) = env::var("DAPPER_SESSIONS_DIR") {
            return Ok(PathBuf::from(env_dir));
        }

        let sessions_dir = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Failed to get data directory"))?
            .join("dapper")
            .join("sessions");
        Ok(sessions_dir)
    }

    #[cfg(test)]
    fn get_sessions_dir() -> anyhow::Result<PathBuf> {
        Ok(get_user_temp_dir().join("test_sessions"))
    }

    fn get_file_path(&self) -> anyhow::Result<PathBuf> {
        let sessions_dir = Self::get_sessions_dir()?;

        let filename = format!(
            "dapper_proxy_{}_{}_{}.json",
            self.started_at, self.pid, self.session_id
        );

        Ok(sessions_dir.join(filename))
    }

    pub fn is_process_alive(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            std::path::Path::new(&format!("/proc/{}", self.pid)).exists()
        }

        #[cfg(target_os = "windows")]
        {
            use std::process::Command;
            let output = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", self.pid), "/NH"])
                .output();
            match output {
                Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&self.pid.to_string()),
                Err(_) => true, // conservatively assume alive
            }
        }

        #[cfg(target_os = "macos")]
        {
            // Signal 0 checks process existence without sending a signal.
            let output = std::process::Command::new("kill")
                .args(["-0", &self.pid.to_string()])
                .output();
            match output {
                Ok(o) => o.status.success(),
                Err(_) => true, // conservatively assume alive
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            true
        }
    }

    pub fn is_port_reachable(&self) -> bool {
        match self.control_plane_port {
            None => false,
            Some(port) => {
                let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port.get());
                TcpListener::bind(addr).is_err()
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_process_alive() && self.is_port_reachable()
    }

    pub fn iter_sessions() -> anyhow::Result<impl Iterator<Item = SessionInfo>> {
        let sessions_dir = Self::get_sessions_dir()?;

        let mut paths: Vec<PathBuf> = fs::read_dir(&sessions_dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("json") {
                            Some(path)
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|e| {
                tracing::warn!(
                    "Failed to read sessions directory {}: {}. Returning empty sessions iterator.",
                    sessions_dir.display(),
                    e
                );
                Vec::new()
            });

        paths.sort();

        let iter = paths.into_iter().filter_map(|path| {
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(e) => {
                    tracing::warn!("Failed to open session file {}: {}", path.display(), e);
                    return None;
                }
            };

            match serde_json::from_reader(file) {
                Ok(session_info) => Some(session_info),
                Err(e) => {
                    tracing::warn!("Failed to parse session file {}: {}", path.display(), e);
                    match fs::remove_file(&path) {
                        Ok(()) => {
                            tracing::info!("Deleted invalid session file: {}", path.display());
                        }
                        Err(e) => {
                            tracing::warn!("Failed to delete session file: {}", e);
                        }
                    }
                    None
                }
            }
        });

        Ok(iter)
    }

    pub fn iter_active_sessions(
        scope_id: Option<ScopeId>,
    ) -> anyhow::Result<impl Iterator<Item = SessionInfo>> {
        Ok(Self::iter_sessions()?.filter_map(move |session| {
            if let Some(ref filter_scope) = scope_id
                && session.scope_id.as_ref() != Some(filter_scope)
            {
                return None;
            }

            if !session.is_active() {
                if let Err(e) = session.delete_file() {
                    tracing::warn!(
                        "Failed to delete stale session file for pid {}: {}",
                        session.pid,
                        e
                    );
                }
                return None;
            }
            Some(session)
        }))
    }

    pub fn find_active_session_with_id(
        scope_id: Option<ScopeId>,
        session_id: &SessionId,
    ) -> anyhow::Result<Option<SessionInfo>> {
        Ok(Self::iter_active_sessions(scope_id)?.find(|s| s.session_id == *session_id))
    }
}

impl fmt::Display for SessionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Session {}:", self.session_id)?;

        writeln!(f, "  {:<13} {}", "PID:", self.pid)?;

        let port_str = self
            .control_plane_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        writeln!(f, "  {:<13} {}", "Control Port:", port_str)?;

        writeln!(
            f,
            "  {:<13} {}",
            "Scope ID:",
            self.scope_id.as_ref().map_or("-", |s| s.as_str())
        )?;

        if let Some(request_type) = self.request_type {
            writeln!(f, "  {:<13} {}", "Request Type:", request_type)?;
        }

        if let Some(ref session_type) = self.session_type {
            writeln!(f, "  {:<13} {}", "Session Type:", session_type)?;
        }

        if let Some(ref program_path) = self.program_path {
            writeln!(f, "  {:<13} {}", "Program:", program_path)?;
        }

        if let Some(debuggee_pid) = self.debuggee_process_id {
            writeln!(f, "  {:<13} {}", "Debuggee PID:", debuggee_pid)?;
        }

        if let Some(ref parent_session_id) = self.parent_session_id {
            writeln!(f, "  {:<13} {}", "Parent Session:", parent_session_id)?;
        }

        let datetime = DateTime::from_timestamp(self.started_at, 0)
            .map(|dt| dt.with_timezone(&Local))
            .unwrap_or_else(Local::now);
        writeln!(
            f,
            "  {:<13} {}",
            "Started At:",
            datetime.format("%Y-%m-%d %H:%M:%S")
        )?;

        let dir_str = self
            .current_working_directory
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        writeln!(f, "  {:<13} {}", "Directory:", dir_str)?;

        let cmd_str = if !self.command_line_args.is_empty() {
            self.command_line_args.join(" ")
        } else {
            "-".to_string()
        };
        writeln!(f, "  {:<13} {}", "Command:", cmd_str)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_info_creation() {
        let session_info = SessionInfo::generate(
            "test-session-id".into(),
            Port::try_new(12345),
            Some("test-scope".into()),
            Some(RequestType::Launch),
            None,
        );

        assert_eq!(session_info.session_id, SessionId::new("test-session-id"));
        assert!(session_info.control_plane_port.is_some());
        assert_eq!(session_info.control_plane_port.unwrap().get(), 12345);
        assert_eq!(session_info.scope_id, Some(ScopeId::new("test-scope")));
        assert_eq!(session_info.request_type, Some(RequestType::Launch));
        assert!(session_info.pid > 0);
        assert!(session_info.started_at > 0);
        assert!(!session_info.command_line_args.is_empty());
    }

    #[test]
    fn test_session_file_write_and_delete() {
        let session_info = SessionInfo::generate(
            "test-session-id".into(),
            Port::try_new(12345),
            Some("test-scope".into()),
            Some(RequestType::Attach),
            None,
        );

        let file_path = session_info.write_to_file().unwrap();
        assert!(file_path.exists());
        session_info.delete_file().unwrap();
        assert!(!file_path.exists());
        session_info.delete_file().unwrap();
    }

    #[test]
    fn test_current_process_is_alive() {
        let session = SessionInfo::generate(
            "test-session-id".into(),
            Port::try_new(12345),
            None,
            Some(RequestType::Launch),
            None,
        );
        assert!(session.is_process_alive());
    }

    #[test]
    fn test_is_port_reachable_with_bound_port() {
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).unwrap();

        let bound_port = listener.local_addr().unwrap().port();
        let session = SessionInfo::generate(
            "test-session-id".into(),
            Port::try_new(bound_port),
            None,
            Some(RequestType::Attach),
            None,
        );

        assert!(session.is_port_reachable());
    }

    #[test]
    fn test_iter_sessions() {
        let test_scope: ScopeId = format!("test-scope-{}", uuid::Uuid::new_v4()).into();

        let session1 = SessionInfo::generate(
            "session1-id".into(),
            Port::try_new(11111),
            Some(test_scope.clone()),
            Some(RequestType::Launch),
            None,
        );
        session1.write_to_file().unwrap();

        let session2 = SessionInfo::generate(
            "session2-id".into(),
            Port::try_new(22222),
            Some(test_scope.clone()),
            Some(RequestType::Attach),
            None,
        );
        session2.write_to_file().unwrap();

        let sessions: Vec<SessionInfo> = SessionInfo::iter_sessions()
            .unwrap()
            .filter(|s| s.scope_id.as_ref() == Some(&test_scope))
            .collect();

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].control_plane_port.unwrap().get(), 11111);
        assert_eq!(sessions[1].control_plane_port.unwrap().get(), 22222);

        session1.delete_file().unwrap();
        session2.delete_file().unwrap();
    }

    #[test]
    fn test_session_info_parent_session_id_default_none() {
        // generate() produces a root session with no parent linkage.
        let mut session = SessionInfo::generate(
            "child-session-id".into(),
            Port::try_new(12345),
            Some("test-scope".into()),
            Some(RequestType::Launch),
            None,
        );
        assert!(session.parent_session_id.is_none());

        // When None, `parent_session_id` is omitted from serialization via
        // `skip_serializing_if = "Option::is_none"`. Clear `command_line_args`
        // first: under `buck test` it carries the test name, which embeds the
        // substring "parent_session_id" and would otherwise defeat the
        // key-absence check below.
        session.command_line_args.clear();
        let json = serde_json::to_string(&session).unwrap();
        assert!(
            !json.contains("parent_session_id"),
            "None parent_session_id should be omitted from JSON: {json}"
        );

        // It round-trips back to None.
        let reparsed: SessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.parent_session_id, None);

        // Backward compatibility: session JSON written before this field
        // existed (no `parent_session_id` key) still deserializes, defaulting
        // to None via `#[serde(default)]`.
        let legacy_json = r#"{
            "session_id": "legacy",
            "pid": 1,
            "control_plane_port": null,
            "started_at": 0,
            "command_line_args": [],
            "current_working_directory": null,
            "scope_id": null
        }"#;
        let legacy: SessionInfo = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.parent_session_id, None);
    }

    #[test]
    fn test_session_info_with_parent_session_id() {
        let parent = SessionId::new("parent-session-id");
        let session = SessionInfo::generate(
            "child-session-id".into(),
            Port::try_new(12345),
            Some("test-scope".into()),
            Some(RequestType::Attach),
            None,
        )
        .with_parent_session_id(Some(parent.clone()));
        assert_eq!(session.parent_session_id, Some(parent.clone()));

        // Present parent_session_id round-trips through serialization.
        let json = serde_json::to_string(&session).unwrap();
        let reparsed: SessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.parent_session_id, Some(parent));
    }

    #[test]
    fn test_resolve_socket_addr_literals() {
        // IPv4 and IPv6 literals resolve without DNS, so this is deterministic.
        let v4 = resolve_socket_addr("127.0.0.1", 8080).unwrap();
        assert_eq!(v4, "127.0.0.1:8080".parse().unwrap());

        let v6 = resolve_socket_addr("::1", 8080).unwrap();
        assert_eq!(v6, "[::1]:8080".parse().unwrap());
    }
}
