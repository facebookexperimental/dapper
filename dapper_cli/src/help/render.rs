// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Markdown rendering helpers for the `dapper help` subcommand.
//!
//! - [`substitute`] expands the `{{program}}` token in topic bodies and
//!   auto-generated content to whatever the user invoked us as
//!   (`dapper`, `fdb dapper`, `meta dapper`, ...).
//! - [`generate_from_clap`] auto-renders any clap `Command` into
//!   markdown. The output emits `"{{program}} "` substrings and relies
//!   on `substitute` at print time so the same body works under any
//!   invocation name.

use std::io::IsTerminal;

use termimad::CompoundStyle;
use termimad::MadSkin;
use termimad::crossterm::style::Attribute;
use termimad::crossterm::style::Color;

/// Literal token expanded by [`substitute`] to the resolved program
/// name (e.g. `"dapper"` or `"fdb dapper"`).
pub(crate) const PROGRAM_TOKEN: &str = "{{program}}";
const HELP_FORMAT_ENV_VAR: &str = "DAPPER_HELP_FORMAT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Plain,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, strum::AsRefStr, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
enum HelpFormatPreference {
    Auto,
    Plain,
    Terminal,
}

const INLINE_CODE_ACCENT: Color = Color::Magenta;
const CODE_BLOCK_ACCENT: Color = Color::Cyan;

/// Single-pass `{{program}}` → program-name substitution.
///
/// Plain prose mentions of "Dapper" or "dapper" are NOT touched — only
/// the explicit `{{program}}` token is. This keeps prose like
/// "Dapper is a DAP proxy" intact even when invoked under `fdb dapper`.
///
/// Re-exported from `dapper_cli::help` so embedder overlay tests can
/// run the same substitution pass production uses, instead of
/// inlining their own `replace("{{program}}", program)` and drifting
/// if the substitution surface ever grows.
pub fn substitute(body: &str, program_name: &str) -> String {
    body.replace(PROGRAM_TOKEN, program_name)
}

/// Print help Markdown. Interactive terminals get styled Markdown;
/// redirected output stays raw Markdown so scripts and snapshots keep
/// seeing the documented text format.
pub(crate) fn print_help_markdown(markdown: &str) {
    print!(
        "{}",
        format_help_markdown(markdown, output_mode_for_stdout())
    );
}

fn output_mode_for_stdout() -> OutputMode {
    output_mode_for_preference(
        help_format_preference_from_env(),
        std::io::stdout().is_terminal(),
    )
}

fn help_format_preference_from_env() -> HelpFormatPreference {
    std::env::var(HELP_FORMAT_ENV_VAR)
        .ok()
        .as_deref()
        .map(|value| value.trim().parse().unwrap_or(HelpFormatPreference::Auto))
        .unwrap_or(HelpFormatPreference::Auto)
}

fn output_mode_for_preference(
    preference: HelpFormatPreference,
    stdout_is_terminal: bool,
) -> OutputMode {
    match preference {
        HelpFormatPreference::Plain => OutputMode::Plain,
        HelpFormatPreference::Terminal => OutputMode::Terminal,
        HelpFormatPreference::Auto if stdout_is_terminal => OutputMode::Terminal,
        HelpFormatPreference::Auto => OutputMode::Plain,
    }
}

fn format_help_markdown(markdown: &str, mode: OutputMode) -> String {
    match mode {
        OutputMode::Plain => markdown.to_owned(),
        OutputMode::Terminal => help_skin().term_text(markdown).to_string(),
    }
}

fn help_skin() -> MadSkin {
    MadSkin {
        inline_code: inline_code_style(),
        code_block: CompoundStyle::with_fg(CODE_BLOCK_ACCENT).into(),
        ..Default::default()
    }
}

fn inline_code_style() -> CompoundStyle {
    let mut style = CompoundStyle::with_fg(INLINE_CODE_ACCENT);
    style.add_attr(Attribute::Bold);
    style
}

/// Build a markdown reference for a clap `Command` from its metadata.
///
/// `path` is the space-joined topic path the user typed (e.g. `"debug"`
/// or `"debug threads"`); the renderer uses it for the H1 and for nested
/// command examples. The output contains `{{program}}` tokens that the
/// caller is expected to feed through [`substitute`] before printing.
pub(crate) fn generate_from_clap(cmd: &clap::Command, path: &str) -> String {
    use std::fmt::Write;

    let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
    let mut output = format!("# {PROGRAM_TOKEN} {path}\n\n{about}\n");

    let positionals: Vec<_> = cmd.get_positionals().collect();
    if !positionals.is_empty() {
        output.push_str("\n## Arguments\n\n");
        for arg in positionals {
            let name = arg.get_id().as_str();
            let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            let marker = if arg.is_required_set() {
                "(required)"
            } else {
                "(optional)"
            };
            let _ = writeln!(output, "- `<{name}>` {marker} — {help}");
        }
    }

    let opts: Vec<_> = cmd.get_opts().collect();
    if !opts.is_empty() {
        output.push_str("\n## Options\n\n");
        for opt in opts {
            let long = opt.get_long().map(|l| format!("--{l}")).unwrap_or_default();
            let short = opt
                .get_short()
                .map(|s| format!("-{s}, "))
                .unwrap_or_default();
            let help = opt.get_help().map(|h| h.to_string()).unwrap_or_default();
            let _ = writeln!(output, "- `{short}{long}` — {help}");
        }
    }

    let subs: Vec<_> = cmd.get_subcommands().filter(|s| !s.is_hide_set()).collect();
    if !subs.is_empty() {
        output.push_str("\n## Subcommands\n\n");
        for sub in subs {
            let name = sub.get_name();
            let sub_about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            let _ = writeln!(output, "- `{PROGRAM_TOKEN} {path} {name}` — {sub_about}");
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_replaces_token() {
        assert_eq!(
            substitute("run `{{program}} debug`", "dapper"),
            "run `dapper debug`"
        );
    }

    #[test]
    fn substitute_uses_branded_name() {
        assert_eq!(
            substitute("run `{{program}} debug`", "fdb dapper"),
            "run `fdb dapper debug`"
        );
    }

    #[test]
    fn substitute_handles_multiple_occurrences() {
        let body = "{{program}} a; {{program}} b; {{program}} c";
        assert_eq!(substitute(body, "x"), "x a; x b; x c");
    }

    #[test]
    fn substitute_leaves_prose_dapper_alone() {
        // "Dapper" the noun should NOT be substituted — only the token.
        let body = "Dapper is a DAP proxy. Run `{{program}} debug`.";
        assert_eq!(
            substitute(body, "fdb dapper"),
            "Dapper is a DAP proxy. Run `fdb dapper debug`."
        );
    }

    #[test]
    fn substitute_no_op_when_no_token() {
        assert_eq!(substitute("plain text", "dapper"), "plain text");
    }

    #[test]
    fn plain_output_preserves_markdown() {
        let markdown = "# Title\n\n- `item`\n";
        assert_eq!(format_help_markdown(markdown, OutputMode::Plain), markdown);
    }

    #[test]
    fn terminal_output_renders_markdown() {
        let rendered = format_help_markdown("# Title\n\n- `item`\n", OutputMode::Terminal);
        assert!(
            rendered.contains("Title"),
            "rendered help should preserve heading text"
        );
        assert!(
            rendered.contains("item"),
            "rendered help should preserve inline code text"
        );
        assert_ne!(
            rendered, "# Title\n\n- `item`\n",
            "terminal output should be rendered instead of raw Markdown"
        );
    }

    #[test]
    fn help_skin_uses_distinct_code_styles() {
        let skin = help_skin();
        assert_eq!(skin.inline_code.get_fg(), Some(INLINE_CODE_ACCENT));
        assert_eq!(skin.inline_code.get_bg(), None);
        assert!(skin.inline_code.has_attr(Attribute::Bold));
        assert_eq!(
            skin.code_block.compound_style.get_fg(),
            Some(CODE_BLOCK_ACCENT)
        );
        assert_eq!(skin.code_block.compound_style.get_bg(), None);
    }

    #[test]
    fn output_mode_auto_tracks_stdout_terminal_state() {
        assert_eq!(
            output_mode_for_preference(HelpFormatPreference::Auto, false),
            OutputMode::Plain
        );
        assert_eq!(
            output_mode_for_preference(HelpFormatPreference::Auto, true),
            OutputMode::Terminal
        );
    }

    #[test]
    fn output_mode_can_be_forced() {
        assert_eq!(
            output_mode_for_preference(HelpFormatPreference::Plain, true),
            OutputMode::Plain
        );
        assert_eq!(
            output_mode_for_preference(HelpFormatPreference::Terminal, false),
            OutputMode::Terminal
        );
    }

    #[test]
    fn help_format_preference_reads_env_var() {
        temp_env::with_var(HELP_FORMAT_ENV_VAR, Some("plain"), || {
            assert_eq!(
                help_format_preference_from_env(),
                HelpFormatPreference::Plain
            );
        });
    }

    #[test]
    fn help_format_preference_defaults_to_auto_for_unknown_env_var() {
        temp_env::with_var(HELP_FORMAT_ENV_VAR, Some("definitely-not-a-format"), || {
            assert_eq!(
                help_format_preference_from_env(),
                HelpFormatPreference::Auto
            );
        });
    }

    #[test]
    fn help_format_preference_defaults_to_auto_when_env_var_is_unset() {
        temp_env::with_var_unset(HELP_FORMAT_ENV_VAR, || {
            assert_eq!(
                help_format_preference_from_env(),
                HelpFormatPreference::Auto
            );
        });
    }
}
