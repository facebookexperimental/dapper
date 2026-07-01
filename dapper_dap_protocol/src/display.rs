// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Temporary Display implementations for DAP data types.
//!
//! These impls provide text rendering for debugging tools (MCP, CLI).
//! They will be removed when rendering moves to the client side.

use std::fmt;

use crate::data_types::Scope;
use crate::data_types::StackFrame;
use crate::data_types::Thread;
use crate::data_types::Variable;

impl fmt::Display for Thread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Thread {}: {}", self.id, self.name)
    }
}

impl StackFrame {
    pub fn format_with_index(&self, index: usize) -> String {
        let source_info = self
            .source
            .as_ref()
            .and_then(|s| s.path.as_ref())
            .map(|path| format!(" at {}:{}", path, self.line))
            .unwrap_or_default();

        format!(
            "#{}: {} (frame id: {}){}",
            index, self.name, self.id, source_info
        )
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Scope: {} (ref: {}, expensive: {})",
            self.name, self.variables_reference, self.expensive
        )
    }
}

impl fmt::Display for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let type_info = self
            .var_type
            .as_ref()
            .map(|t| format!(" ({})", t))
            .unwrap_or_default();

        let child_ref = if self.variables_reference.has_children() {
            format!(" [ref: {}]", self.variables_reference)
        } else {
            String::new()
        };

        write!(f, "{}: {}{}{}", self.name, self.value, type_info, child_ref)
    }
}
