// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Client-side session targeting shared by control-plane CLI front ends.

use std::str::FromStr;

use anyhow::Context;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionId;
use dapper_session::SessionInfo;
use dapper_session::SessionStore;
use dapper_session::SessionsResult;

use crate::DapperControlPlaneClient;
use crate::resolve_unique_session;

/// The session a CLI invocation targets, decided once from its flags.
///
/// Every operation below applies the same rules: a control port is exact and
/// unscoped, a session id is looked up within the optional scope, and
/// otherwise the unique active session in the scope is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionTarget {
    Port(Port),
    Session {
        id: SessionId,
        scope: Option<ScopeId>,
    },
    Discover {
        scope: Option<ScopeId>,
    },
}

impl SessionTarget {
    /// `--control-port` wins over `--session-id`, which wins over discovery.
    /// The scope does not apply to a control port.
    pub fn from_flags(
        control_port: Option<Port>,
        session_id: Option<SessionId>,
        scope_id: Option<ScopeId>,
    ) -> Self {
        match (control_port, session_id) {
            (Some(port), _) => Self::Port(port),
            (None, Some(id)) => Self::Session {
                id,
                scope: scope_id,
            },
            (None, None) => Self::Discover { scope: scope_id },
        }
    }

    /// A client for the targeted control plane. Discovery is lazy: the
    /// unique session is resolved on each request, not here.
    pub fn client(&self, store: &SessionStore) -> anyhow::Result<DapperControlPlaneClient> {
        match self {
            Self::Port(port) => Ok(DapperControlPlaneClient::for_port(*port)),
            Self::Session { id, .. } => {
                let session = self.find_session(store)?;
                let port = session
                    .control_plane_port
                    .with_context(|| format!("session {id} has no control plane port"))?;
                Ok(DapperControlPlaneClient::for_port(port))
            }
            Self::Discover { scope } => Ok(DapperControlPlaneClient::discover(
                store.clone(),
                scope.clone(),
            )),
        }
    }

    /// The stored record of the targeted session.
    pub fn find_session(&self, store: &SessionStore) -> anyhow::Result<SessionInfo> {
        match self {
            Self::Port(port) => store
                .iter_active_sessions(None)
                .find(|session| session.control_plane_port == Some(*port))
                .with_context(|| format!("no session found on port {port}")),
            Self::Session { id, scope } => store
                .find_active_session_with_id(scope.clone(), id)
                .with_context(|| format!("no active session found with id {id}")),
            Self::Discover { scope } => resolve_unique_session(
                store.iter_active_sessions(scope.clone()).collect(),
                scope,
                None,
            ),
        }
    }

    /// The active sessions matching the target, from the store alone; no
    /// control plane is contacted. Discovery lists every candidate rather
    /// than requiring a unique one.
    pub fn list_sessions(&self, store: &SessionStore) -> SessionsResult {
        match self {
            Self::Port(_) => SessionsResult {
                sessions: self.find_session(store).into_iter().collect(),
                scope_id: None,
            },
            Self::Session { id, scope } => SessionsResult {
                sessions: store
                    .find_active_session_with_id(scope.clone(), id)
                    .into_iter()
                    .collect(),
                scope_id: scope.clone(),
            },
            Self::Discover { scope } => SessionsResult {
                sessions: store.iter_active_sessions(scope.clone()).collect(),
                scope_id: scope.clone(),
            },
        }
    }
}

/// A `--breakpoints` CLI value: a bare line number or a JSON spec such as
/// `{"line":20,"condition":"x>5"}`.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum BreakpointArg {
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

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    /// A store holding one session in scope `other`, listening on a bound port.
    fn seeded_store() -> (tempfile::TempDir, SessionStore, SessionInfo, TcpListener) {
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
        (dir, store, session, listener)
    }

    fn session(id: &SessionId, scope: &str) -> SessionTarget {
        SessionTarget::Session {
            id: id.clone(),
            scope: Some(ScopeId::new(scope)),
        }
    }

    fn discover(scope: &str) -> SessionTarget {
        SessionTarget::Discover {
            scope: Some(ScopeId::new(scope)),
        }
    }

    #[test]
    fn from_flags_applies_precedence() {
        let port = Port::try_new(1234).expect("non-zero");
        let id = SessionId::new("abc");
        let scope = Some(ScopeId::new("s"));
        assert_eq!(
            SessionTarget::from_flags(Some(port), Some(id.clone()), scope.clone()),
            SessionTarget::Port(port),
            "a control port is unscoped and wins"
        );
        assert_eq!(
            SessionTarget::from_flags(None, Some(id.clone()), scope.clone()),
            SessionTarget::Session {
                id,
                scope: scope.clone()
            }
        );
        assert_eq!(
            SessionTarget::from_flags(None, None, scope.clone()),
            SessionTarget::Discover { scope }
        );
    }

    #[test]
    fn find_session_by_port_session_id_and_discovery() {
        let (_dir, store, seeded, _listener) = seeded_store();
        let port = seeded.control_plane_port.expect("seeded with a port");

        let found = SessionTarget::Port(port)
            .find_session(&store)
            .expect("a control port names the session whatever its scope");
        assert_eq!(found.session_id, seeded.session_id);

        let found = session(&seeded.session_id, "other")
            .find_session(&store)
            .expect("a session id selects the matching session in scope");
        assert_eq!(found.session_id, seeded.session_id);
        assert!(
            session(&seeded.session_id, "mine")
                .find_session(&store)
                .is_err(),
            "the scope must constrain a session id"
        );

        let found = discover("other")
            .find_session(&store)
            .expect("the only session in scope is discovered");
        assert_eq!(found.session_id, seeded.session_id);
        assert!(
            discover("mine").find_session(&store).is_err(),
            "discovery must filter by scope"
        );
    }

    #[test]
    fn client_targets_the_resolved_port() {
        let (_dir, store, seeded, _listener) = seeded_store();
        let port = seeded.control_plane_port.expect("seeded with a port");
        let resolved = |target: SessionTarget| {
            let (port, _) = target
                .client(&store)
                .expect("client for a known session")
                .resolve_port()
                .expect("port resolves");
            port
        };

        assert_eq!(resolved(SessionTarget::Port(port)), port);
        assert_eq!(resolved(session(&seeded.session_id, "other")), port);
        assert_eq!(resolved(discover("other")), port);

        assert!(
            session(&seeded.session_id, "mine").client(&store).is_err(),
            "a session id outside the scope has no client"
        );
        let lazy = discover("mine")
            .client(&store)
            .expect("discovery defers resolution to the first request");
        assert!(lazy.resolve_port().is_err(), "nothing in scope to resolve");
    }

    #[test]
    fn list_sessions_follows_the_target() {
        let (_dir, store, seeded, _listener) = seeded_store();
        let port = seeded.control_plane_port.expect("seeded with a port");

        assert_eq!(discover("other").list_sessions(&store).sessions.len(), 1);
        assert!(
            discover("mine").list_sessions(&store).sessions.is_empty(),
            "listing must filter by scope"
        );
        assert_eq!(
            session(&seeded.session_id, "other")
                .list_sessions(&store)
                .sessions
                .len(),
            1
        );
        assert!(
            session(&seeded.session_id, "mine")
                .list_sessions(&store)
                .sessions
                .is_empty()
        );
        let by_port = SessionTarget::Port(port).list_sessions(&store);
        assert_eq!(by_port.sessions.len(), 1, "the session on that port");
        assert_eq!(by_port.scope_id, None, "a control port is unscoped");
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
