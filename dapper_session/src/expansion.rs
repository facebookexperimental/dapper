// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

/// Which nested results a `threads`, `stack_trace` or `scopes` call may fold
/// into its response, on behalf of the caller.
///
/// Each flag gates the matching auto-expansion on top of the proxy's own
/// config: a level is expanded only when both allow it, so [`Expansion::ALL`]
/// leaves the decision to the config. A command ignores the flags for levels
/// it cannot reach, e.g. `scopes` ignores `stack_trace` and `scopes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expansion {
    /// Attach the first thread's stack trace to `threads`.
    pub stack_trace: bool,
    /// Attach the topmost frame's scopes to a stack trace.
    pub scopes: bool,
    /// Attach the locals scope's variables to scopes.
    pub locals: bool,
}

impl Expansion {
    /// Expand whatever the proxy's config expands.
    pub const ALL: Self = Self {
        stack_trace: true,
        scopes: true,
        locals: true,
    };

    /// Expand nothing, whatever the proxy's config says.
    pub const NONE: Self = Self {
        stack_trace: false,
        scopes: false,
        locals: false,
    };
}

impl Default for Expansion {
    fn default() -> Self {
        Self::ALL
    }
}
