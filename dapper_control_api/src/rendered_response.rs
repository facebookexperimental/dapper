// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::io::Write;

use anyhow::Context;

const MAX_RESPONSE_SIZE: usize = 100_000;

fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    &s[..s.floor_char_boundary(max_bytes)]
}

/// A rendered DAP response, split by whether it fits inline.
///
/// Purely a size decision — the *caller* chooses whether and where to
/// persist the full text of an oversized response.
pub enum RenderedResponse {
    Complete(String),
    Truncated {
        /// The first `MAX_RESPONSE_SIZE` bytes (rounded down to a char
        /// boundary).
        shown: String,
        /// The full rendered text, for the caller to persist.
        full: String,
    },
}

impl RenderedResponse {
    pub fn from_text(text: String) -> Self {
        if text.len() > MAX_RESPONSE_SIZE {
            let shown = truncate_at_char_boundary(&text, MAX_RESPONSE_SIZE).to_string();
            Self::Truncated { shown, full: text }
        } else {
            Self::Complete(text)
        }
    }

    /// Spill an oversized response to the user temp dir (best-effort) and
    /// return the final text. Does blocking filesystem IO on the truncated
    /// path — async callers should run this via `spawn_blocking`.
    pub fn spill_to_temp_and_render(self) -> String {
        match self {
            Self::Complete(text) => text,
            Self::Truncated { shown, full } => {
                let path = save_response_to_temp_file(&full)
                    .inspect_err(|e| {
                        tracing::warn!("Failed to save large response to file: {}", e);
                    })
                    .ok();
                match path {
                    Some(path) => format!(
                        "{}...\n\n[Response truncated: {} bytes total, showing first {}. Full response saved to: {}]",
                        shown,
                        full.len(),
                        MAX_RESPONSE_SIZE,
                        path
                    ),
                    None => format!(
                        "{}...\n\n[Response truncated: {} bytes total, showing first {}]",
                        shown,
                        full.len(),
                        MAX_RESPONSE_SIZE
                    ),
                }
            }
        }
    }
}

fn save_response_to_temp_file(content: &str) -> anyhow::Result<String> {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    // Timestamp alone can collide across processes or rapid calls; the pid
    // and a per-process counter make the name unique.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let temp_dir = dapper_session::get_user_temp_dir();
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("Failed to create temp directory '{}'", temp_dir.display()))?;
    let path = temp_dir.join(format!(
        "dapper-response-{}-{}-{}.json",
        timestamp,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::File::create(&path)?;
    file.write_all(content.as_bytes())?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_at_char_boundary_rounds_down_to_char() {
        let s = "a★b";
        assert_eq!(truncate_at_char_boundary(s, 2), "a");
        assert_eq!(truncate_at_char_boundary(s, 3), "a");
        assert_eq!(truncate_at_char_boundary(s, 4), "a★");
        assert_eq!(truncate_at_char_boundary(s, s.len()), s);
        assert_eq!(truncate_at_char_boundary(s, s.len() + 1), s);
        assert_eq!(truncate_at_char_boundary("", 0), "");
    }

    #[test]
    fn rendered_response_small_text_is_complete() {
        let text = "small".to_string();
        match RenderedResponse::from_text(text.clone()) {
            RenderedResponse::Complete(t) => assert_eq!(t, text),
            RenderedResponse::Truncated { .. } => panic!("small text must not truncate"),
        }
    }

    #[test]
    fn rendered_response_oversized_text_truncates_at_char_boundary() {
        // 3-byte chars ensure MAX_RESPONSE_SIZE can land mid-character;
        // truncation must round down to a boundary instead of panicking.
        let ch = "\u{2501}"; // ━, 3 bytes
        let repeat = MAX_RESPONSE_SIZE / ch.len() + 10;
        let text = ch.repeat(repeat);
        assert!(text.len() > MAX_RESPONSE_SIZE);

        match RenderedResponse::from_text(text.clone()) {
            RenderedResponse::Complete(_) => panic!("oversized text must truncate"),
            RenderedResponse::Truncated { shown, full } => {
                assert_eq!(full, text, "the full text must be preserved for spilling");
                assert!(shown.len() <= MAX_RESPONSE_SIZE);
                assert!(
                    shown.is_char_boundary(shown.len()),
                    "shown prefix must end on a char boundary"
                );
                assert!(text.starts_with(&shown));
            }
        }
    }
}
