// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_session::Port;
use dapper_session::ScopeId;

mod debug;
pub use debug::Debug;

mod mcp;
pub use mcp::Mcp;

mod proxy;
pub use proxy::Proxy;

#[derive(clap::Args)]
pub(crate) struct SessionTarget {
    /// Control plane port to connect to.
    /// If omitted, auto-discovers the unique active session — or errors with the
    /// candidate list when more than one is active. Pass --control-port (always
    /// deterministic) or a tighter --scope-id / DAPPER_SCOPE_ID to disambiguate.
    #[arg(long)]
    control_port: Option<Port>,
    /// Scope identifier to target a specific session.
    /// Filters auto-discovery and the sessions listing. May also be set via DAPPER_SCOPE_ID.
    #[arg(long, env = "DAPPER_SCOPE_ID")]
    scope_id: Option<ScopeId>,
}
