// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Stable names for the outcome events Dapper emits, one per MCP tool call
//! and one per debug session, so telemetry layers can select them by a
//! structured field instead of matching log message text.
//!
//! Each outcome event carries a [`FIELD`] field whose value is one of the
//! event constants below. Telemetry consumers persist these values, so
//! renaming one is a schema change for that data: add a new name instead.

/// The tracing field that names an outcome event.
pub const FIELD: &str = "dapper_event";

/// An MCP tool call completed successfully.
pub const TOOL_CALL_SUCCESS: &str = "tool_call_success";
/// An MCP tool call ran but its result reported an error.
pub const TOOL_CALL_ERROR: &str = "tool_call_error";
/// An MCP tool call failed at the protocol level, e.g. an unknown tool.
pub const TOOL_CALL_FAILED: &str = "tool_call_failed";

/// A headless debug session finished the DAP handshake.
pub const DAP_INIT_SUCCESS: &str = "dap_init_success";
/// A headless debug session ended in failure; see [`SESSION_FAILURE_KIND`].
pub const DAP_SESSION_FAILED: &str = "dap_session_failed";

/// The tracing field that classifies a [`DAP_SESSION_FAILED`] event.
pub const SESSION_FAILURE_KIND: &str = "session_failure_kind";
/// The DAP handshake failed, so no tool call could run.
pub const FAILURE_HANDSHAKE: &str = "handshake";
/// The handshake completed, but the adapter then went silent past the idle
/// timeout.
pub const FAILURE_HUNG: &str = "hung";
/// The session initializer task panicked.
pub const FAILURE_PANICKED: &str = "panicked";
