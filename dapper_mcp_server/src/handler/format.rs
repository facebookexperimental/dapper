// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Pure formatting helpers for tool output (no handler state).

use std::fmt::Write as _;

use base64::Engine as _;
use dapper_dap_protocol::responses::ReadMemoryResponseBody;

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
