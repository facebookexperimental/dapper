// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#![warn(clippy::all)]

use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

/// Output format for CLI commands
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Plaintext,
    Json,
}

/// Main configuration file structure
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DapperConfig {
    /// Output format: "plaintext" (default) or "json"
    #[serde(default)]
    pub output_format: OutputFormat,
    /// Configuration for threads command
    #[serde(default)]
    pub threads: ThreadsConfig,
    /// Configuration for stack_trace command
    #[serde(default)]
    pub stack_trace: StackTraceConfig,
    /// Configuration for scopes command
    #[serde(default)]
    pub scopes: ScopesConfig,
    /// Configuration for navigate command
    #[serde(default)]
    pub navigate: NavigateConfig,
    /// Configuration for context in tool responses
    #[serde(default)]
    pub context: ContextConfig,
}

/// Configuration for the threads command
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ThreadsConfig {
    /// Whether to automatically show the first thread's stack trace
    pub show_stacktrace: bool,
    /// Maximum number of threads before skipping automatic stacktrace expansion
    pub expand_stacktrace_threshold: usize,
}

impl Default for ThreadsConfig {
    fn default() -> Self {
        Self {
            show_stacktrace: true,
            expand_stacktrace_threshold: 10,
        }
    }
}

/// Configuration for the stack_trace command
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct StackTraceConfig {
    /// Whether to automatically expand the topmost frame's scopes
    pub expand_scopes: bool,
    /// Maximum number of frames to request (0 for all frames) in case of hitting token limit
    pub max_frames: usize,
}

impl Default for StackTraceConfig {
    fn default() -> Self {
        Self {
            expand_scopes: true,
            max_frames: 250,
        }
    }
}

/// Configuration for the scopes command
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ScopesConfig {
    /// Whether to automatically expand local variables
    pub expand_locals: bool,
}

impl Default for ScopesConfig {
    fn default() -> Self {
        Self {
            expand_locals: true,
        }
    }
}

/// Configuration for the navigate command
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct NavigateConfig {
    /// Timeout in seconds for continue navigation type to wait for stopped/exited events.
    /// Set to 0 or null to disable timeout and wait indefinitely. Defaults to 60 seconds.
    pub continue_timeout_seconds: Option<u64>,
    /// Timeout in seconds for pause navigation type to wait for stopped events.
    /// Pause is expected to stop quickly, so this has a shorter default (5 seconds).
    /// Set to 0 or null to disable timeout and wait indefinitely.
    pub pause_timeout_seconds: Option<u64>,
}

impl Default for NavigateConfig {
    fn default() -> Self {
        Self {
            continue_timeout_seconds: Some(60),
            pause_timeout_seconds: Some(5),
        }
    }
}

impl NavigateConfig {
    /// Get the timeout duration for continue operations.
    /// Returns None if timeout is disabled (continue_timeout_seconds is 0 or None).
    pub fn continue_timeout(&self) -> Option<std::time::Duration> {
        match self.continue_timeout_seconds {
            Some(0) | None => None,
            Some(secs) => Some(std::time::Duration::from_secs(secs)),
        }
    }

    /// Get the timeout duration for pause operations.
    /// Returns None if timeout is disabled (pause_timeout_seconds is 0 or None).
    pub fn pause_timeout(&self) -> Option<std::time::Duration> {
        match self.pause_timeout_seconds {
            Some(0) | None => None,
            Some(secs) => Some(std::time::Duration::from_secs(secs)),
        }
    }
}

/// Configuration for context in tool responses (session info + state summary).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Whether to include execution state, breakpoints, output, and other sessions in
    /// tool responses. Session info is controlled separately by `show_session`.
    pub enable: bool,
    /// Whether to show session info (session ID, debugger type, program) in the context header.
    pub show_session: bool,
    /// Whether to show source-line breakpoints in the context footer.
    pub show_breakpoints: bool,
    /// Whether to show installed exception breakpoint filters in the context footer.
    pub show_exception_breakpoints: bool,
    /// Maximum number of source files to show in the breakpoint list.
    pub max_source_files: usize,
    /// Whether to show execution state in the context footer.
    pub show_execution_state: bool,
    /// Whether to show active debug sessions in the context footer.
    pub show_sessions: bool,
    /// Maximum number of output lines to show in the context footer (0 to disable).
    pub max_output_lines: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            enable: true,
            show_session: true,
            show_breakpoints: true,
            show_exception_breakpoints: true,
            max_source_files: 25,
            show_execution_state: true,
            show_sessions: true,
            max_output_lines: 20,
        }
    }
}

impl ContextConfig {
    pub fn all_enabled() -> Self {
        Self {
            enable: true,
            show_session: true,
            show_breakpoints: true,
            show_exception_breakpoints: true,
            max_source_files: usize::MAX,
            show_execution_state: true,
            show_sessions: true,
            max_output_lines: 20,
        }
    }
}

impl DapperConfig {
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            tracing::debug!(
                "Config file not found at {}, using defaults",
                config_path.display()
            );
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

        tracing::info!("Loaded config from {}", config_path.display());
        Ok(config)
    }

    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|e| {
            tracing::warn!("Failed to load config: {}, using defaults", e);
            Self::default()
        })
    }

    fn get_config_path() -> Result<PathBuf> {
        Ok(Self::get_config_dir()?.join("config.toml"))
    }

    #[cfg(not(test))]
    fn get_config_dir() -> Result<PathBuf> {
        if let Ok(env_dir) = std::env::var("DAPPER_CONFIG_DIR") {
            return Ok(PathBuf::from(env_dir));
        }

        let config_dir = dirs::config_local_dir()
            .ok_or_else(|| anyhow::anyhow!("Failed to get config directory"))?
            .join("dapper");
        Ok(config_dir)
    }

    #[cfg(test)]
    fn get_config_dir() -> Result<PathBuf> {
        Ok(dapper_session::get_user_temp_dir().join("test_config"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_config_sections() {
        let toml_content = r#"
[threads]
show_stacktrace = false
expand_stacktrace_threshold = 5

[stack_trace]
expand_scopes = false
max_frames = 100

[scopes]
expand_locals = false
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert!(!config.threads.show_stacktrace);
        assert_eq!(config.threads.expand_stacktrace_threshold, 5);
        assert!(!config.stack_trace.expand_scopes);
        assert_eq!(config.stack_trace.max_frames, 100);
        assert!(!config.scopes.expand_locals);
    }

    #[test]
    fn test_command_config_defaults() {
        let toml_content = r#""#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert!(config.threads.show_stacktrace);
        assert_eq!(config.threads.expand_stacktrace_threshold, 10);
        assert!(config.stack_trace.expand_scopes);
        assert!(config.scopes.expand_locals);
        assert_eq!(config.navigate.continue_timeout_seconds, Some(60));
        assert!(config.context.enable);
        assert!(config.context.show_session);
    }

    #[test]
    fn test_navigate_config_with_timeout() {
        let toml_content = r#"
[navigate]
continue_timeout_seconds = 30
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert_eq!(config.navigate.continue_timeout_seconds, Some(30));
        assert_eq!(
            config.navigate.continue_timeout(),
            Some(std::time::Duration::from_secs(30))
        );
    }

    #[test]
    fn test_navigate_config_no_timeout_zero() {
        let toml_content = r#"
[navigate]
continue_timeout_seconds = 0
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert_eq!(config.navigate.continue_timeout_seconds, Some(0));
        assert_eq!(config.navigate.continue_timeout(), None);
    }

    #[test]
    fn test_navigate_config_default_timeout() {
        // Test that default is 60 seconds when not specified
        let toml_content = r#"
[navigate]
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert_eq!(config.navigate.continue_timeout_seconds, Some(60));
        assert_eq!(
            config.navigate.continue_timeout(),
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(config.navigate.pause_timeout_seconds, Some(5));
        assert_eq!(
            config.navigate.pause_timeout(),
            Some(std::time::Duration::from_secs(5))
        );
    }

    #[test]
    fn test_navigate_config_pause_timeout() {
        let toml_content = r#"
[navigate]
pause_timeout_seconds = 10
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert_eq!(config.navigate.pause_timeout_seconds, Some(10));
        assert_eq!(
            config.navigate.pause_timeout(),
            Some(std::time::Duration::from_secs(10))
        );
    }

    #[test]
    fn test_navigate_config_pause_timeout_disabled() {
        let toml_content = r#"
[navigate]
pause_timeout_seconds = 0
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert_eq!(config.navigate.pause_timeout_seconds, Some(0));
        assert_eq!(config.navigate.pause_timeout(), None);
    }

    #[test]
    fn test_context_config_show_session_enabled() {
        let toml_content = r#"
[context]
show_session = true
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert!(config.context.show_session);
    }

    #[test]
    fn test_context_config_show_session_disabled() {
        let toml_content = r#"
[context]
show_session = false
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert!(!config.context.show_session);
    }

    #[test]
    fn test_context_config_fields() {
        let toml_content = r#"
[context]
enable = false
show_session = false
show_breakpoints = false
max_source_files = 10
show_execution_state = false
show_sessions = false
max_output_lines = 5
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();

        assert!(!config.context.enable);
        assert!(!config.context.show_session);
        assert!(!config.context.show_breakpoints);
        assert_eq!(config.context.max_source_files, 10);
        assert!(!config.context.show_execution_state);
        assert!(!config.context.show_sessions);
        assert_eq!(config.context.max_output_lines, 5);
    }

    #[test]
    fn test_context_config_defaults() {
        let config: DapperConfig = toml::from_str("").unwrap();

        assert!(config.context.enable);
        assert!(config.context.show_session);
        assert!(config.context.show_breakpoints);
        assert_eq!(config.context.max_source_files, 25);
        assert!(config.context.show_execution_state);
        assert!(config.context.show_sessions);
        assert_eq!(config.context.max_output_lines, 20);
    }

    #[test]
    fn test_output_format_default_is_plaintext() {
        let config: DapperConfig = toml::from_str("").unwrap();
        assert_eq!(config.output_format, OutputFormat::Plaintext);
    }

    #[test]
    fn test_output_format_json() {
        let toml_content = r#"
output_format = "json"
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.output_format, OutputFormat::Json);
    }

    #[test]
    fn test_output_format_plaintext_explicit() {
        let toml_content = r#"
output_format = "plaintext"
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.output_format, OutputFormat::Plaintext);
    }

    #[test]
    fn test_output_format_invalid_value() {
        let toml_content = r#"
output_format = "xml"
        "#;
        let result: Result<DapperConfig, _> = toml::from_str(toml_content);
        assert!(result.is_err());
    }

    #[test]
    fn test_output_format_coexists_with_other_config() {
        let toml_content = r#"
output_format = "json"

[threads]
show_stacktrace = false

[navigate]
continue_timeout_seconds = 30
        "#;
        let config: DapperConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.output_format, OutputFormat::Json);
        assert!(!config.threads.show_stacktrace);
        assert_eq!(config.navigate.continue_timeout_seconds, Some(30));
    }
}
