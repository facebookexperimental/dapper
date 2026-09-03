// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! How this process was invoked: how to re-enter it, and what to call it.

use std::path::Path;

/// How a running dapper is reached.
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
            Self::Embedded { subcommand } => std::slice::from_ref(&subcommand.0),
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

/// Resolve the user-facing program name from the process's argv.
///
/// - If `argv[0]` looks like a deliberate brand string — contains a
///   space *and* no path separators — assume the embedder set it
///   that way (e.g. `"fdb dapper"`, `"meta dapper"`) and return it
///   verbatim. Path-separator presence rules out the
///   spaces-in-installation-path case (`/home/user/My Tools/dapper`).
/// - Otherwise, take the file stem of the path (so `/usr/local/bin/dapper`,
///   `./dapper`, `target/debug/dapper`, and `dapper.exe` all become `dapper`).
/// - Fall back to `"dapper"` if neither yields a value.
pub fn from_args(args: &[String]) -> String {
    let arg0 = args.first().map(String::as_str).unwrap_or("dapper");
    if arg0.contains(' ') && !arg0.contains('/') && !arg0.contains('\\') {
        return arg0.to_owned();
    }
    Path::new(arg0)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("dapper")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_needs_no_reentry_args() {
        assert!(Reentry::Standalone.args().is_empty());
    }

    fn embedded(subcommand: &str) -> Reentry {
        Reentry::Embedded {
            subcommand: CommandWord::try_new(subcommand).expect("valid in tests"),
        }
    }

    #[test]
    fn embedded_replays_the_host_subcommand() {
        assert_eq!(embedded("dapper").args(), ["dapper"]);
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
    fn standalone_dapper_path() {
        assert_eq!(from_args(&["/usr/local/bin/dapper".into()]), "dapper");
    }

    #[test]
    fn standalone_dapper_basename() {
        assert_eq!(from_args(&["dapper".into()]), "dapper");
    }

    #[test]
    fn relative_target_dir() {
        assert_eq!(from_args(&["target/debug/dapper".into()]), "dapper");
    }

    #[test]
    fn windows_exe_suffix() {
        assert_eq!(from_args(&["dapper.exe".into()]), "dapper");
    }

    #[test]
    fn embedder_supplies_branded_name() {
        assert_eq!(from_args(&["fdb dapper".into()]), "fdb dapper");
        assert_eq!(from_args(&["meta dapper".into()]), "meta dapper");
    }

    #[test]
    fn empty_args_defaults_to_dapper() {
        assert_eq!(from_args(&[]), "dapper");
    }

    #[test]
    fn empty_arg0_defaults_to_dapper() {
        assert_eq!(from_args(&[String::new()]), "dapper");
    }

    #[test]
    fn install_path_with_space_is_not_treated_as_brand() {
        // Path-separator-bearing strings always go through file-stem
        // extraction so an installation under `My Tools/` doesn't
        // become the rendered program name. The brand-name detector
        // also short-circuits on `\\` to keep Windows install paths
        // out of the brand path; that lookup happens on Windows where
        // `file_stem` recognizes `\\` as a separator (Linux's
        // `file_stem` does not, so we only assert the unix form here).
        assert_eq!(from_args(&["/home/user/My Tools/dapper".into()]), "dapper");
    }
}
