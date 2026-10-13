// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_session::Port;
use dapper_session::ScopeId;
use dapper_session::SessionId;

mod debug;
pub use debug::Debug;

mod mcp;
pub use mcp::Mcp;

mod proxy;
pub use proxy::Proxy;

#[derive(clap::Args)]
pub(crate) struct SessionTarget {
    /// Control plane port to connect to.
    /// If omitted, auto-discovers the unique active session (or, when several are
    /// active, the only root session) and errors with the candidate list when
    /// that is ambiguous. Pass --control-port or --session-id (both deterministic)
    /// or a tighter --scope-id / DAPPER_SCOPE_ID to disambiguate.
    #[arg(long, conflicts_with = "session_id")]
    control_port: Option<Port>,
    /// Session identifier to target directly.
    /// May be combined with --scope-id to require the session to belong to that scope.
    #[arg(long)]
    session_id: Option<SessionId>,
    /// Scope identifier to target a specific session.
    /// Filters auto-discovery and the sessions listing. May also be set via DAPPER_SCOPE_ID.
    #[arg(long, env = "DAPPER_SCOPE_ID")]
    scope_id: Option<ScopeId>,
}
