// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#![warn(clippy::all)]

use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use tracing_glog::Glog;
use tracing_glog::GlogFields;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

/// Get a user-specific temporary directory for dapper.
/// On Unix, appends the username to avoid collisions on shared multi-user systems.
/// On Windows, temp_dir() is already per-user.
///
/// Deliberate copy of `dapper_session::get_user_temp_dir` so this crate
/// stays dependency-free; the two must agree so logs and session artifacts
/// land under the same directory. Change both together.
fn get_user_temp_dir() -> PathBuf {
    let base = std::env::temp_dir();
    #[cfg(unix)]
    {
        let username = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        base.join(format!("dapper-{}", username))
    }
    #[cfg(not(unix))]
    {
        base.join("dapper")
    }
}

/// Get the default log file path for dapper proxy server logs.
///
/// Returns a platform-appropriate path based on the DAPPER_LOG_PATH environment
/// variable, or uses the system temp directory if not set.
pub fn default_log_file_path() -> PathBuf {
    std::env::var("DAPPER_LOG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| get_user_temp_dir().join("dapper_proxy_server.log"))
}

/// Cap on a caller-supplied reason, marker included. `dapper proxy` stamps one
/// onto every Scuba sample for the life of the process, and an MCP client can
/// send one per tool call, so an unbounded string is amplified across a whole
/// session.
pub(crate) const MAX_REASON_LEN: usize = 200;

const TRUNCATION_MARKER: &str = "...";

/// Characters dropped outright from a reason: C0/C1 controls, plus the
/// zero-width and bidi formatters that can hide or visually reorder the rest of
/// a log line. Whitespace is handled separately, as a word separator.
fn is_unsafe_in_logs(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{feff}')
}

/// Make a caller-supplied `--reason` (CLI) or `reason` (MCP tool argument) safe
/// to log, or `None` if nothing is left.
///
/// Truncation is marked so a cut reason stays distinguishable in telemetry from
/// one that genuinely ended there.
pub fn normalize_reason(reason: &str) -> Option<String> {
    let mut normalized = reason
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| !is_unsafe_in_logs(*c))
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .fold(String::new(), |mut acc, word| {
            if !acc.is_empty() {
                acc.push(' ');
            }
            acc.push_str(&word);
            acc
        });

    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().count() > MAX_REASON_LEN {
        normalized = normalized
            .chars()
            .take(MAX_REASON_LEN.saturating_sub(TRUNCATION_MARKER.chars().count()))
            .collect();
        normalized.push_str(TRUNCATION_MARKER);
    }
    Some(normalized)
}

pub type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync + 'static>;

pub fn default_layers() -> Result<Vec<BoxedLayer>> {
    let filter = EnvFilter::from_default_env();
    let console_layer: BoxedLayer = Box::new(console_logging_layer().with_filter(filter.clone()));

    let mut layers = vec![console_layer];

    // Add file logging layer if available, but don't fail if file can't be opened
    match file_logging_layer(&default_log_file_path()) {
        Ok(file_layer) => {
            layers.push(Box::new(file_layer.with_filter(filter)));
        }
        Err(e) => {
            eprintln!("Warning: {:#}. File logging disabled.", e);
        }
    }

    Ok(layers)
}

/// Create a console logging layer for stderr output.
///
/// This function creates a tracing layer that outputs human-readable logs to stderr
/// with appropriate ANSI color formatting based on terminal capabilities.
pub fn console_logging_layer() -> impl Layer<Registry> {
    tracing_subscriber::fmt::Layer::default()
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .event_format(Glog::default().with_timer(tracing_glog::LocalTime::default()))
        .fmt_fields(GlogFields::default())
}

/// Create a file logging layer for the specified file path.
///
/// This function creates a tracing layer that outputs structured JSON logs to a file.
/// The file is created if it doesn't exist and logs are appended to existing content.
pub fn file_logging_layer(file_path: &Path) -> Result<impl Layer<Registry>> {
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create log directory '{}'", parent.display()))?;
    }

    let log_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(file_path)
        .with_context(|| format!("Failed to open log file '{}'", file_path.display()))?;

    Ok(tracing_subscriber::fmt::Layer::default()
        .json()
        .with_file(true)
        .with_line_number(true)
        .with_writer(log_file))
}

/// Set up a panic hook to capture panics and backtraces in the logs.
pub fn set_panic_hook() {
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = Backtrace::force_capture();

        let payload = info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "Unknown panic payload"
        };

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        tracing::error!(
            "Process panicked: {} at {}\nBacktrace:\n{}",
            message,
            location,
            backtrace
        );

        // Note: With set_hook, we can't call the previous hook.
        // The default panic behavior (printing to stderr) won't occur.
        // This is acceptable since we're logging the panic info ourselves.
    }));
}

/// Initialize logging with the given directives.
///
/// This function sets up a global tracing subscriber with both stderr and file output.
/// It configures the subscriber with the provided filter directives and sets up
/// appropriate formatting for both human-readable (stderr) and structured (file) logging.
pub fn init_logging(layers: Vec<BoxedLayer>) -> Result<()> {
    let subscriber = Registry::default().with(layers);

    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set global subscriber")?;

    set_panic_hook();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace_and_control_characters() {
        assert_eq!(
            normalize_reason("  step past\n the retry\tloop  ").as_deref(),
            Some("step past the retry loop")
        );
        assert_eq!(
            normalize_reason("drop \u{1b}[31m esc").as_deref(),
            Some("drop [31m esc"),
            "an escape sequence must not reach the terminal verbatim"
        );
    }

    #[test]
    fn blank_input_yields_nothing() {
        for input in ["", "   ", "\n\t", "\u{1b}"] {
            assert_eq!(normalize_reason(input), None, "for {input:?}");
        }
    }

    #[test]
    fn overlong_input_is_capped_and_marked() {
        let capped = normalize_reason(&"x".repeat(500)).expect("a long reason survives");
        assert_eq!(
            capped.chars().count(),
            MAX_REASON_LEN,
            "the marker counts against the cap"
        );
        assert!(
            capped.ends_with(TRUNCATION_MARKER),
            "truncation must be visible"
        );
    }

    #[test]
    fn bidi_and_zero_width_formatters_are_stripped() {
        assert_eq!(
            normalize_reason("safe\u{202e}gnippops\u{200b}text\u{2060}joined\u{061c}mark")
                .as_deref(),
            Some("safegnippopstextjoinedmark"),
            "dropped, not replaced with a space, so the obfuscation does not survive"
        );
    }
}
