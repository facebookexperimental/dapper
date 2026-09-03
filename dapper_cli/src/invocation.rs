// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! How this process was invoked: how to re-enter it, and what to call it.
//!
//! Re-entry is declared by the caller, never inferred: it drives an `exec`, so
//! it cannot rest on `argv[0]`, which any caller can set to anything. The
//! display name follows the declaration when embedded, and `argv[0]` when not,
//! where echoing back the name the user typed is the point.

use std::ffi::OsStr;
use std::path::Path;

/// How a running dapper is reached, and what that entry point is called.
///
/// Declared by whoever starts dapper, never inferred. No `Default`: every
/// entry point states which case it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reentry {
    /// This process's executable is dapper itself.
    Standalone,
    /// Dapper is embedded in a host CLI and reached through one of its
    /// subcommands, which a re-invocation has to replay: `fdb dapper proxy
    /// ...`, never `fdb proxy ...`.
    Embedded {
        /// The host's user-facing name, e.g. `fdb`. Display only: a child is
        /// spawned from `current_exe()`, so anything other than the name users
        /// actually type renders help a reader cannot run.
        host: CommandWord,
        /// The host subcommand that reaches dapper, e.g. `"dapper"` in
        /// `fdb dapper`.
        subcommand: CommandWord,
    },
}

impl Reentry {
    /// Argv to insert between the executable and dapper's own arguments.
    pub fn args(&self) -> &[String] {
        match self {
            Self::Standalone => &[],
            Self::Embedded { subcommand, .. } => std::slice::from_ref(&subcommand.0),
        }
    }

    /// What a user types to reach this dapper, for help text and clap's usage
    /// line. Don't switch the standalone arm to `current_exe()`: it resolves
    /// symlinks, so an alias on `PATH` would be told to run a name it may not
    /// have.
    pub fn program_name(&self, arg0: Option<&OsStr>) -> String {
        match self {
            Self::Standalone => arg0
                .and_then(|arg0| executable_stem(Path::new(arg0)))
                .unwrap_or_else(|| DEFAULT_PROGRAM_NAME.to_owned()),
            Self::Embedded { host, subcommand } => format!("{} {}", host.0, subcommand.0),
        }
    }
}

/// One word of the command a user types to reach dapper, e.g. `dapper` in
/// `fdb dapper`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandWord(String);

impl CommandWord {
    /// Rejects anything that is not a single non-blank word: a malformed value
    /// reaches the host as bad argv only after the child spawn is acked, so it
    /// would fail silently.
    pub fn try_new(word: impl Into<String>) -> anyhow::Result<Self> {
        let word = word.into();
        anyhow::ensure!(
            !word.is_empty() && !word.contains(char::is_whitespace),
            "must be a single non-blank word, got {word:?}"
        );
        Ok(Self(word))
    }
}

const DEFAULT_PROGRAM_NAME: &str = "dapper";

fn executable_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded_in_fdb() -> Reentry {
        Reentry::Embedded {
            host: CommandWord::try_new("fdb").expect("valid in tests"),
            subcommand: CommandWord::try_new("dapper").expect("valid in tests"),
        }
    }

    fn standalone_name(arg0: &str) -> String {
        Reentry::Standalone.program_name(Some(OsStr::new(arg0)))
    }

    #[test]
    fn standalone_needs_no_reentry_args() {
        assert!(Reentry::Standalone.args().is_empty());
    }

    #[test]
    fn embedded_replays_the_host_subcommand() {
        assert_eq!(embedded_in_fdb().args(), ["dapper"]);
    }

    #[test]
    fn a_subcommand_that_is_not_one_word_is_rejected() {
        for bad in ["", " ", "fdb dapper", "dapper\t"] {
            assert!(
                CommandWord::try_new(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn embedded_program_name_comes_from_the_declaration() {
        assert_eq!(embedded_in_fdb().program_name(None), "fdb dapper");
        assert_eq!(
            Reentry::Embedded {
                host: CommandWord::try_new("meta").expect("valid in tests"),
                subcommand: CommandWord::try_new("dapper").expect("valid in tests"),
            }
            .program_name(None),
            "meta dapper",
        );
    }

    #[test]
    fn standalone_program_name_is_the_argv0_stem() {
        for arg0 in [
            "/usr/local/bin/dapper",
            "dapper",
            "target/debug/dapper",
            "dapper.exe",
            "/home/user/My Tools/dapper",
        ] {
            assert_eq!(standalone_name(arg0), "dapper", "for {arg0}");
        }
    }

    /// `fb/tests/e2e/help_topics.rs` drives the standalone binary with a
    /// branded `argv[0]`; splitting or trimming here would break it with a
    /// failure that looks unrelated to naming.
    #[test]
    fn a_multi_word_argv0_is_not_split() {
        assert_eq!(standalone_name("fdb dapper"), "fdb dapper");
    }

    #[test]
    fn an_alias_keeps_its_own_name() {
        assert_eq!(
            standalone_name("/usr/local/bin/renamed-dapper"),
            "renamed-dapper"
        );
    }

    #[test]
    fn standalone_program_name_falls_back_when_argv0_is_unusable() {
        assert_eq!(Reentry::Standalone.program_name(None), DEFAULT_PROGRAM_NAME);
        assert_eq!(standalone_name(""), DEFAULT_PROGRAM_NAME);
        assert_eq!(standalone_name("/"), DEFAULT_PROGRAM_NAME);
    }
}
