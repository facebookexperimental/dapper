// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

/// Recursively normalizes non-idempotent DAP response fields in a JSON value
/// so that two structurally equivalent responses from separate DAP requests
/// can be compared with `assert_eq!`.
///
/// DAP servers assign monotonically increasing counters and opaque handles
/// (sourceReference, variablesReference, frame IDs, etc.) that differ between
/// requests even when the underlying debuggee state is identical. This function
/// zeroes out those fields while preserving stable fields like thread IDs,
/// names, line numbers, and column numbers.
pub fn normalize_dap_response(value: &serde_json::Value) -> serde_json::Value {
    let mut result = value.clone();
    normalize_dap_value(&mut result);
    result
}

fn normalize_dap_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            // Zero out numeric reference/handle fields.
            for key in [
                "sourceReference",
                "variablesReference",
                "declarationLocationReference",
                "valueLocationReference",
            ] {
                if let Some(value) = map.get_mut(key) {
                    *value = serde_json::Value::Number(0.into());
                }
            }

            // Blank out string reference fields.
            for key in ["memoryReference", "instructionPointerReference"] {
                if let Some(value) = map.get_mut(key) {
                    *value = serde_json::Value::String(String::new());
                }
            }

            // Zero out `id` in StackFrame-shaped objects (has name + line + column)
            // but NOT in Thread-shaped objects (has id + name only).
            // If a new DAP response type collides with these shapes, this logic will need updating.
            let is_stack_frame = map.contains_key("id")
                && map.contains_key("name")
                && map.contains_key("line")
                && map.contains_key("column");

            // Zero out `id` in Breakpoint-shaped objects (has verified).
            let is_breakpoint = map.contains_key("id") && map.contains_key("verified");

            if (is_stack_frame || is_breakpoint)
                && let Some(value) = map.get_mut("id")
            {
                *value = serde_json::Value::Number(0.into());
            }

            for value in map.values_mut() {
                normalize_dap_value(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_dap_value(value);
            }
        }
        _ => {}
    }
}
