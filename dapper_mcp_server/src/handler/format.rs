// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Pure formatting helpers for tool output (no handler state).

use std::fmt::Write as _;

use base64::Engine as _;
use dapper_dap_protocol::responses::ReadMemoryResponseBody;
use rmcp::serde_json;

pub(super) fn format_capabilities(value: &serde_json::Value) -> String {
    let mut supported = Vec::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            if val == &serde_json::Value::Bool(true) {
                supported.push(key.as_str());
            }
        }
    }
    let exception_filters_section: Option<String> = value
        .get("exceptionBreakpointFilters")
        .and_then(|v| v.as_array())
        .filter(|arr| !arr.is_empty())
        .map(|arr| format_exception_breakpoint_filters(arr.as_slice()));
    if supported.is_empty() && exception_filters_section.is_none() {
        return "No optional capabilities reported by the adapter.".to_string();
    }
    let mut output = String::new();
    if !supported.is_empty() {
        supported.sort();
        output.push_str("Supported capabilities:\n");
        for cap in &supported {
            let _ = writeln!(output, "  - {cap}");
        }
        output.push('\n');
    }
    if let Some(section) = exception_filters_section {
        output.push_str(&section);
        output.push('\n');
    }
    output.push_str("Capabilities not listed are unsupported by this adapter.");
    output
}

/// Render the `exceptionBreakpointFilters` array (advertised in the
/// `Capabilities` response) as a sorted list of filter ids with optional
/// label/default/supports_condition annotations. The bool-only walker
/// above silently drops this array, so it gets its own dedicated section.
fn format_exception_breakpoint_filters(filters: &[serde_json::Value]) -> String {
    let mut entries: Vec<&serde_json::Value> = filters.iter().collect();
    entries.sort_by(|a, b| {
        a.get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("filter").and_then(|v| v.as_str()).unwrap_or(""))
    });

    let mut output = String::from("Exception breakpoint filters:\n");
    for entry in entries {
        let filter = entry
            .get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        // Up to 3 annotations: label, default, supports_condition.
        let mut annotations: Vec<String> = Vec::with_capacity(3);
        // Debug-format the label so values containing whitespace or
        // punctuation render with visible quotes — useful for an LLM
        // agent reading the capability output.
        if let Some(label) = entry.get("label").and_then(|v| v.as_str()) {
            annotations.push(format!("label: {label:?}"));
        }
        if entry
            .get("default")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            annotations.push("default: true".to_string());
        }
        if entry
            .get("supportsCondition")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            annotations.push("supports_condition: true".to_string());
        }
        if annotations.is_empty() {
            let _ = writeln!(output, "  - {filter}");
        } else {
            let _ = writeln!(output, "  - {filter} ({})", annotations.join(", "));
        }
    }
    output
}

/// Format a ReadMemoryResponseBody as a hex dump with addresses and ASCII sidebar.
///
/// Returns `Err` only when the response payload exists but cannot be base64-decoded —
/// that's a protocol-level failure the caller should surface as a tool error rather
/// than a successful response. All other "no data" cases (None payload, unreadable
/// bytes) render as informational text inside `Ok`.
pub(super) fn format_memory_read(body: &ReadMemoryResponseBody) -> anyhow::Result<String> {
    let data = match &body.data {
        Some(b64) => base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| anyhow::anyhow!("address {}: {}", body.address, e))?,
        None => {
            return Ok(match body.unreadable_bytes {
                Some(n) => format!("Address: {}\n{} byte(s) unreadable.", body.address, n),
                None => format!("Address: {}\nNo data returned.", body.address),
            });
        }
    };

    let base_addr = parse_address(&body.address);
    let mut output = format!("Memory at {} ({} bytes):\n", body.address, data.len());
    if let Some(n) = body.unreadable_bytes {
        let _ = writeln!(output, "({} byte(s) unreadable)", n);
    }

    for (i, chunk) in data.chunks(16).enumerate() {
        match base_addr {
            Some(base) => {
                let addr = base.wrapping_add((i * 16) as u64);
                let _ = write!(output, "0x{:016X}: ", addr);
            }
            // base address didn't parse — show row offsets so the column isn't a lie
            None => {
                let _ = write!(output, "+0x{:08X}:        ", i * 16);
            }
        }

        for (j, byte) in chunk.iter().enumerate() {
            if j == 8 {
                output.push(' ');
            }
            let _ = write!(output, "{:02X} ", byte);
        }
        // Pad short final row
        for j in chunk.len()..16 {
            if j == 8 {
                output.push(' ');
            }
            output.push_str("   ");
        }

        output.push(' ');
        for byte in chunk {
            output.push(if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            });
        }
        output.push('\n');
    }

    Ok(output)
}

/// Parse a DAP `memoryReference`-style address into a u64.
///
/// Per the DAP spec, the address is hex when prefixed with `0x`/`0X` and
/// decimal otherwise. Returns `None` if the input doesn't parse — callers
/// should fall back to relative offsets rather than render a misleading
/// absolute column.
pub(super) fn parse_address(s: &str) -> Option<u64> {
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(rest, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}
