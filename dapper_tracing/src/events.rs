// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Stable names for the outcome events Dapper emits, one per MCP tool call
//! and one per debug session, so telemetry layers can select them by a
//! structured field instead of matching log message text.
//!
//! Each outcome event carries a [`TelemetryEvent::field_name`] field whose
//! value is the event's [`Display`] representation. Telemetry consumers persist
//! these values, so renaming one is a schema change for that data: add a new
//! name instead.

use strum::Display;

/// An MCP tool-call or debug-session outcome recorded in telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "snake_case")]
pub enum TelemetryEvent {
    /// An MCP tool call completed successfully.
    ToolCallSuccess,
    /// An MCP tool call ran but its result reported an error. When the debug
    /// adapter rejected the tool's request, the event also carries
    /// `rejected_by_debugger = true`.
    ToolCallError,
    /// An MCP tool call failed at the protocol level, e.g. an unknown tool.
    ToolCallFailed,
    /// A headless debug session finished the DAP handshake.
    DapInitSuccess,
    /// A headless debug session ended in failure; see [`SessionFailureKind`].
    DapSessionFailed,
}

impl TelemetryEvent {
    /// The tracing field that names an outcome event.
    pub const fn field_name() -> &'static str {
        "dapper_event"
    }
}

/// The failure classification of a [`TelemetryEvent::DapSessionFailed`] event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "snake_case")]
pub enum SessionFailureKind {
    /// The DAP handshake failed, so no tool call could run.
    Handshake,
    /// The handshake completed, but the adapter then went silent past the idle
    /// timeout.
    Hung,
    /// The session initializer task panicked.
    Panicked,
}

impl SessionFailureKind {
    /// The tracing field that classifies a failed debug session.
    pub const fn field_name() -> &'static str {
        "session_failure_kind"
    }
}
