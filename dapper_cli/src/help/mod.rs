// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! `dapper help` subcommand: LLM-optimized self-documentation.
//!
//! Modeled on `sl help` (`eden/scm/sapling/help.py`) — topic-first
//! lookup with fallback to clap subcommand introspection. Topics are
//! organized as a unified tree: built-in OSS topics
//! ([`topics::BUILTINS`]) and any embedder-supplied overlay slice
//! (e.g. `dapper_fb_main::help::TOPICS`) are merged at lookup time.
//!
//! Supersedes the previous `--skill` flag, which is kept as a hidden
//! pre-clap alias that rewrites argv to invoke this subcommand.

pub mod dispatch;
pub(crate) mod render;
pub mod topic;
pub(crate) mod topics;

pub use dispatch::HelpError;
pub use dispatch::handle;
pub use render::substitute;
pub use topic::Body;
pub use topic::Context;
pub use topic::Topic;
pub use topic::always;
pub use topics::BUILTINS;

/// Shared test helpers for the help subsystem. The construction is
/// trivial but it threads `Cli::command()` through `Context`, which
/// every `dispatch`/`topic`/`topics` test needs — consolidating it
/// here means a `Context` shape change touches one site instead of
/// three.
///
/// Exposed `pub` (and not `#[cfg(test)]`-gated) so embedder overlay
/// crates — which depend on `dapper_cli` as a regular runtime
/// dependency, not a `dev-dependency` — can use the same helper in
/// their own test modules. The cost is a few lines of always-compiled
/// code in release builds.
pub mod test_util {
    use super::topic::Context;

    pub fn with_ctx(program: &str, f: impl FnOnce(&Context<'_>)) {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let ctx = Context {
            program_name: program,
            clap_cmd: &cmd,
        };
        f(&ctx);
    }
}

/// Pre-clap argv rewriter that translates the legacy `--skill` flag into
/// the canonical `help` subcommand.
///
/// Both `dapper_cli/bin/main.rs` and `fb/dapper_fb_main/src/lib.rs` call
/// this once on the raw argv vector before invoking `Cli::parse_from`.
/// `dapper proxy --skill` becomes `dapper help proxy`, bare
/// `dapper --skill` becomes `dapper help`. The first element (program
/// name) is preserved so `program_name::from_args` and clap's usage
/// strings see the right invocation name.
///
/// All other flags in argv (e.g. `--scope-id=X`, `--json`,
/// `--caller-to-log=foo`) are intentionally dropped: topic content is
/// invariant of CLI flags, and forwarding unknown flags to `dapper
/// help` would just produce a clap parse error.
///
/// Only the contiguous run of positionals immediately following the
/// program name is preserved as the topic path. Stopping at the first
/// `-`-prefixed token keeps space-separated flag values (e.g. the
/// `4711` in `--client-port 4711`) from sneaking through as bogus
/// topic tokens.
///
/// `--skill` detection stops at the first `--` end-of-options marker
/// so `dapper proxy process /path/to/dbg -- --skill` (where `--skill`
/// is a literal arg meant for the spawned debugger) doesn't trigger
/// the rewrite. The `--`-less `trailing_var_arg` form
/// (`dapper proxy process /path/to/dbg --skill`) is a known limitation
/// — the rewriter has no clap schema to consult and will still fire.
/// Callers passing literal flags to a child process should use `--`.
///
/// Assumes `args[0]` is the program path, never a flag — which is what
/// every real-world `std::env::args()` invocation produces. Flag
/// detection deliberately starts at `args[1]` so an `argv[0]` that
/// happens to begin with `-` never gets misclassified.
pub fn rewrite_skill_to_help(args: Vec<String>) -> Vec<String> {
    // `--` ends options for the whole argv, so any `--skill` past it
    // is a literal arg destined for a child process — see test
    // `skill_after_double_dash_is_a_literal_passthrough`.
    let prefix_end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    if !args[..prefix_end].iter().skip(1).any(|a| a == "--skill") {
        return args;
    }
    let mut iter = args.into_iter();
    let program = iter.next().unwrap_or_else(|| "dapper".to_owned());
    let mut new_args = vec![program, "help".to_owned()];
    // Filter out any `help` token from the topic path so
    // `dapper help --skill` doesn't become `["dapper", "help", "help"]`
    // (which would dispatch as `Help { topic: ["help"] }` and fall
    // through to clap auto-doc). The filter is unconditional rather
    // than leading-only because no real topic name is `help`, so a
    // mid-path `help` would always be a typo or misuse — dropping it
    // before dispatch yields the same bail diagnostic at one less
    // reasoning hop.
    new_args.extend(
        iter.take_while(|a| !a.starts_with('-'))
            .filter(|a| a != "help"),
    );
    new_args
}

#[cfg(test)]
mod argv_tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn no_skill_flag_passes_args_through_unchanged() {
        let input = s(&["dapper", "debug", "threads"]);
        assert_eq!(rewrite_skill_to_help(input.clone()), input);
    }

    #[test]
    fn empty_args_passes_through_unchanged() {
        // The function is documented to assume `args[0]` is the
        // program path, but should still gracefully no-op on an empty
        // vec rather than panic — pin that.
        assert_eq!(rewrite_skill_to_help(vec![]), Vec::<String>::new());
    }

    #[test]
    fn dash_dash_help_passes_through_to_clap() {
        // `--help` is clap's terse synopsis — it must reach clap
        // intact, never get rewritten into the new `help` subcommand.
        let input = s(&["dapper", "--help"]);
        assert_eq!(rewrite_skill_to_help(input.clone()), input);
        let nested = s(&["dapper", "proxy", "--help"]);
        assert_eq!(rewrite_skill_to_help(nested.clone()), nested);
    }

    #[test]
    fn bare_skill_becomes_help() {
        assert_eq!(
            rewrite_skill_to_help(s(&["dapper", "--skill"])),
            s(&["dapper", "help"])
        );
    }

    #[test]
    fn subcommand_skill_becomes_help_subcommand() {
        assert_eq!(
            rewrite_skill_to_help(s(&["dapper", "proxy", "--skill"])),
            s(&["dapper", "help", "proxy"])
        );
    }

    #[test]
    fn nested_subcommand_skill_becomes_help_path() {
        assert_eq!(
            rewrite_skill_to_help(s(&["dapper", "debug", "threads", "--skill"])),
            s(&["dapper", "help", "debug", "threads"])
        );
    }

    #[test]
    fn embedder_program_name_preserved() {
        // `argv[0]` carries `"fdb dapper"` because `fdb` sets it that
        // way; the rewriter must preserve the multi-word program name
        // so `program_name::from_args` and clap's usage strings see
        // the right invocation.
        assert_eq!(
            rewrite_skill_to_help(s(&["fdb dapper", "mcp", "--skill"])),
            s(&["fdb dapper", "help", "mcp"])
        );
    }

    #[test]
    fn other_flags_intentionally_dropped_legacy_compat() {
        // `--scope-id=X` is a flag; `proxy` is the positional we keep.
        // Help output is invariant of CLI flags, and forwarding flags
        // to `dapper help` would produce a clap parse error — so the
        // rewriter drops them, matching the legacy `skill::handle`
        // contract. (`--scope-id=X` is fused so `take_while` stops on
        // it without leaking the value as a bogus positional.)
        assert_eq!(
            rewrite_skill_to_help(s(&["dapper", "--scope-id=X", "proxy", "--skill"])),
            s(&["dapper", "help"])
        );
    }

    #[test]
    fn help_token_in_argv_is_not_duplicated() {
        // `dapper help --skill` should still produce `["dapper", "help"]`,
        // not `["dapper", "help", "help"]` — the latter would dispatch
        // as `Help { topic: ["help"] }` and fall through to clap.
        assert_eq!(
            rewrite_skill_to_help(s(&["dapper", "help", "--skill"])),
            s(&["dapper", "help"])
        );
    }

    #[test]
    fn skill_in_trailing_var_arg_without_separator_is_known_limitation() {
        // Documented limitation in the doc comment: without a clap
        // schema, the rewriter can't tell that `--skill` here is meant
        // for the spawned debugger. Pinning the current (lossy)
        // behavior so a future "fix" surfaces as a test diff and
        // forces the trade-off back into review.
        assert_eq!(
            rewrite_skill_to_help(s(&[
                "dapper",
                "proxy",
                "process",
                "/path/to/dbg",
                "--skill"
            ])),
            s(&["dapper", "help", "proxy", "process", "/path/to/dbg"])
        );
    }

    #[test]
    fn skill_after_double_dash_is_a_literal_passthrough() {
        // `dapper proxy process /path/to/dbg -- --skill` passes
        // `--skill` to the spawned debugger. The rewriter must NOT
        // intercept it.
        let argv = s(&[
            "dapper",
            "proxy",
            "process",
            "/path/to/dbg",
            "--",
            "--skill",
        ]);
        assert_eq!(rewrite_skill_to_help(argv.clone()), argv);
    }

    #[test]
    fn space_separated_flag_value_does_not_leak_as_topic_token() {
        // `--client-port 4711 --skill` — without `take_while`, the
        // `4711` would slip through the positional filter and end up
        // as a bogus second topic token (`dapper help proxy 4711`).
        // Stopping at the first `-`-prefixed argv element after the
        // program name closes that hole.
        assert_eq!(
            rewrite_skill_to_help(s(&["dapper", "proxy", "--client-port", "4711", "--skill"])),
            s(&["dapper", "help", "proxy"])
        );
    }
}
