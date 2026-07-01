// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! `dapper help [TOPIC...]` dispatcher.
//!
//! Layout follows Sapling's `_helpdispatch.dispatch` (`help.py:407-428`):
//! topic-first lookup, then fall back to clap subcommand introspection.
//!
//! Topics live in a unified tree — built-in OSS topics
//! ([`crate::help::topics::BUILTINS`]) and any embedder-supplied
//! overlay slice are merged at lookup time. Multi-token queries like
//! `dapper help debuggers lldb` walk the tree token by token. A node
//! with non-empty visible children gets a `## Subtopics` index
//! auto-appended to its rendered body.

use std::borrow::Cow;
use std::fmt::Write;

use clap::CommandFactory;

use crate::cli::Cli;
use crate::help::render;
use crate::help::topic::Context;
use crate::help::topic::Topic;
use crate::help::topics;

/// Errors surfaced by [`handle`]. Carry the data needed to render a
/// stderr diagnostic plus the exit code the caller should use, so the
/// dispatcher itself never calls [`std::process::exit`] — the binary
/// entry point gets a clean `Result` to propagate, which keeps tokio's
/// runtime shutdown intact.
#[derive(Debug)]
pub enum HelpError {
    UnknownTopic { key: String, program_name: String },
}

impl HelpError {
    /// Process exit code the caller should use after rendering this
    /// error. Modeled on `sl help` returning 2 for unknown topics.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::UnknownTopic { .. } => 2,
        }
    }

    /// Render the diagnostic to stderr in the format
    /// `dapper help <unknown-key>` returns today.
    pub fn print(&self) {
        match self {
            Self::UnknownTopic { key, program_name } => {
                eprintln!(
                    "no such help topic: {key}\n\
                     Run `{program_name} help` to see available topics."
                );
            }
        }
    }
}

/// Top-level entry point. Reads positional `topic` tokens from the
/// parsed `Help` subcommand args and dispatches accordingly.
///
/// `program_name` is the resolved invocation name (`dapper`,
/// `fdb dapper`, ...) and `overlay` is the embedder-supplied topic
/// slice — `&[]` for the OSS binary.
///
/// On `Err(HelpError)` the binary entry point is expected to call
/// `err.print(); std::process::exit(err.exit_code())`. Returning the
/// error rather than exiting from inside the dispatcher keeps the
/// process-exit at the boundary where tokio's runtime is no longer
/// holding any live state.
pub fn handle(
    topic: &[String],
    program_name: &str,
    overlay: &'static [Topic],
) -> Result<(), HelpError> {
    let clap_cmd = Cli::command();
    let ctx = Context {
        program_name,
        clap_cmd: &clap_cmd,
    };

    if topic.is_empty() {
        print_overview(&ctx, overlay);
        return Ok(());
    }

    if let Some((node, path)) = walk(topic, &ctx, overlay) {
        print_topic(node, &path, &ctx);
        return Ok(());
    }

    // Recursive clap traversal: walk as deep as the topic tokens match.
    // We require *every* token to land on a subcommand — partial matches
    // (e.g. `debug nonexistent`) bail with the unknown-topic diagnostic
    // rather than silently rendering the parent's auto-doc and dropping
    // the trailing tokens. Hidden subcommands are filtered so a future
    // `#[command(hide = true)]` doesn't silently leak through `dapper
    // help <hidden>` (matches `print_overview` and `generate_from_clap`,
    // both of which already honor `is_hide_set`).
    let mut current: &clap::Command = ctx.clap_cmd;
    let mut consumed = 0;
    for token in topic {
        let Some(sub) = current.find_subcommand(token).filter(|s| !s.is_hide_set()) else {
            break;
        };
        current = sub;
        consumed += 1;
    }
    if consumed == topic.len() {
        let path = topic.join(" ");
        let body = render::generate_from_clap(current, &path);
        render::print_help_markdown(&render::substitute(&body, ctx.program_name));
        return Ok(());
    }

    Err(HelpError::UnknownTopic {
        key: topic.join(" "),
        program_name: program_name.to_owned(),
    })
}

/// Walk the merged `BUILTINS + overlay` tree token-by-token. Returns
/// the matched leaf and the path of canonical names taken to reach it
/// (so the caller can render `program help name1 name2 ...` headers
/// for subtopic indexes).
///
/// Top-level lookup chains `BUILTINS.iter()` then `overlay.iter()`, so
/// a built-in always wins on name collision — the overlay is never
/// allowed to shadow OSS surface. Children at deeper levels come from
/// the matched parent's `children` slice; the dispatcher has no notion
/// of which root the parent came from.
fn walk<'a>(
    tokens: &[String],
    ctx: &Context<'_>,
    overlay: &'a [Topic],
) -> Option<(&'a Topic, Vec<&'a str>)> {
    // Callers (`handle`) only invoke `walk` after checking
    // `topic.is_empty()`, so the empty case is unreachable in
    // practice. The `?` here keeps the signature self-contained for
    // any future caller that doesn't enforce the precondition.
    let (head, rest) = tokens.split_first()?;
    let topic = topics::BUILTINS
        .iter()
        .chain(overlay.iter())
        .find(|t| t.matches(head) && (t.visible)(ctx))?;

    let mut path = vec![topic.name];
    let mut current = topic;
    for tok in rest {
        let child = current
            .children
            .iter()
            .find(|c| c.matches(tok) && (c.visible)(ctx))?;
        path.push(child.name);
        current = child;
    }
    Some((current, path))
}

/// Render and print the topic body. When the topic has visible
/// children, append a `## Subtopics` index using each child's
/// `summary` so the parent doesn't have to hand-list its own
/// children.
fn print_topic(topic: &Topic, path: &[&str], ctx: &Context<'_>) {
    let body = topic.body.render(ctx);
    let visible: Vec<&Topic> = topic.children.iter().filter(|c| (c.visible)(ctx)).collect();

    // Common path: leaf topic with no visible children. Pass the `Cow`
    // straight through to `substitute` so a static body avoids the
    // extra allocation that `into_owned` would force.
    let final_body = if visible.is_empty() {
        body
    } else {
        let path_str = path.join(" ");
        let mut owned = body.into_owned();
        owned.push_str("\n## Subtopics\n\n");
        for c in visible {
            // Writing into `String` is infallible; ignore the `Result`.
            let _ = writeln!(
                owned,
                "- `{} help {} {}` — {}",
                ctx.program_name, path_str, c.name, c.summary
            );
        }
        Cow::Owned(owned)
    };

    render::print_help_markdown(&render::substitute(&final_body, ctx.program_name));
}

/// Bare `dapper help`: composes the `overview.md` body, an
/// auto-generated `## Available Commands` block, an `## Available
/// Topics` block (visible top-level topics from the merged
/// `BUILTINS + overlay` tree, minus topics whose name shadows a clap
/// subcommand), and a footer pointer to `--help` for the terse
/// synopsis.
fn print_overview(ctx: &Context<'_>, overlay: &[Topic]) {
    let program = ctx.program_name;
    let overview_body = include_str!("topics/overview.md");
    let mut output = render::substitute(overview_body, program);

    output.push_str("\n## Available Commands\n\n");
    for sub in ctx.clap_cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let name = sub.get_name();
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
        // If a curated topic exists for this subcommand, surface it on
        // the row so the agent learns both surfaces in one glance —
        // otherwise the curated body is reachable but invisible from
        // the overview (because `is_subcommand_topic` filters it out
        // of `## Available Topics` to avoid double-listing). Match by
        // name *or* alias to mirror `is_subcommand_topic`'s scan.
        let has_topic = topics::BUILTINS
            .iter()
            .chain(overlay.iter())
            .any(|t| t.matches(name) && (t.visible)(ctx));
        let pointer = if has_topic {
            format!(" — see `{program} help {name}` for details")
        } else {
            String::new()
        };
        let about_sep = if about.is_empty() { "" } else { " — " };
        let _ = writeln!(output, "- `{program} {name}`{about_sep}{about}{pointer}");
    }

    let visible: Vec<&Topic> = topics::BUILTINS
        .iter()
        .chain(overlay.iter())
        .filter(|t| (t.visible)(ctx))
        // Topics whose name (or any alias) shadows a clap subcommand
        // are advertised by `## Available Commands`; don't double-list.
        .filter(|t| !is_subcommand_topic(t, ctx.clap_cmd))
        .collect();
    if !visible.is_empty() {
        output.push_str("\n## Available Topics\n\n");
        for t in visible {
            let _ = writeln!(output, "- `{program} help {}` — {}", t.name, t.summary);
        }
    }

    let _ = writeln!(
        output,
        "\n(For Usage / Options / version, run `{program} --help`.)"
    );

    render::print_help_markdown(&output);
}

/// True if any of `topic`'s names (canonical or alias) resolves to a
/// clap subcommand. `clap::Command::find_subcommand` is itself
/// alias-aware, so the effective check is "any topic name-or-alias
/// against any clap subcommand name-or-alias".
fn is_subcommand_topic(topic: &Topic, clap_cmd: &clap::Command) -> bool {
    std::iter::once(topic.name)
        .chain(topic.aliases.iter().copied())
        .any(|n| clap_cmd.find_subcommand(n).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::test_util::with_ctx;
    use crate::help::topic::Body;
    use crate::help::topic::always;

    fn never(_: &Context<'_>) -> bool {
        false
    }

    fn tokens(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn walk_finds_top_level_builtin() {
        with_ctx("dapper", |ctx| {
            let (node, path) = walk(&tokens(&["agent"]), ctx, &[]).expect("agent topic");
            assert_eq!(node.name, "agent");
            assert_eq!(path, vec!["agent"]);
        });
    }

    #[test]
    fn walk_resolves_top_level_alias() {
        with_ctx("dapper", |ctx| {
            // `breakpoints` has aliases `breakpoint`, `bp`.
            let (node, _) = walk(&tokens(&["bp"]), ctx, &[]).expect("bp alias");
            assert_eq!(node.name, "breakpoints");
        });
    }

    #[test]
    fn walk_resolves_workflow_alias_to_agent_topic() {
        // `workflow` was a separate topic before being merged into
        // `agent`; the alias keeps `dapper help workflow` working so
        // any pre-existing references don't dead-end.
        with_ctx("dapper", |ctx| {
            let (node, _) = walk(&tokens(&["workflow"]), ctx, &[]).expect("workflow alias");
            assert_eq!(node.name, "agent");
        });
    }

    #[test]
    fn walk_returns_none_for_unknown_top_level() {
        with_ctx("dapper", |ctx| {
            assert!(walk(&tokens(&["definitely-not-a-topic"]), ctx, &[]).is_none());
        });
    }

    #[test]
    fn walk_descends_into_overlay_children() {
        const FAKE_OVERLAY: &[Topic] = &[Topic {
            name: "ns",
            aliases: &[],
            summary: "namespace",
            body: Body::Static("# parent\n"),
            visible: always,
            children: &[Topic {
                name: "leaf",
                aliases: &["alias"],
                summary: "leaf",
                body: Body::Static("# leaf\n"),
                visible: always,
                children: &[],
            }],
        }];
        with_ctx("dapper", |ctx| {
            let (node, path) = walk(&tokens(&["ns", "leaf"]), ctx, FAKE_OVERLAY).expect("leaf");
            assert_eq!(node.name, "leaf");
            assert_eq!(path, vec!["ns", "leaf"]);

            let (alias_node, _) =
                walk(&tokens(&["ns", "alias"]), ctx, FAKE_OVERLAY).expect("alias");
            assert_eq!(alias_node.name, "leaf");
        });
    }

    #[test]
    fn walk_skips_invisible_topic() {
        const HIDDEN: &[Topic] = &[Topic {
            name: "hidden",
            aliases: &[],
            summary: "",
            body: Body::Static(""),
            visible: never,
            children: &[],
        }];
        with_ctx("dapper", |ctx| {
            assert!(walk(&tokens(&["hidden"]), ctx, HIDDEN).is_none());
        });
    }

    #[test]
    fn walk_oss_shadows_overlay_on_collision() {
        // Overlay declares a topic whose name collides with an OSS
        // built-in. The OSS one wins; the overlay node is silently
        // unreachable.
        const SHADOWED: &[Topic] = &[Topic {
            name: "agent",
            aliases: &[],
            summary: "shadow",
            body: Body::Static("OVERLAY_SHADOW_SENTINEL\n"),
            visible: always,
            children: &[],
        }];
        with_ctx("dapper", |ctx| {
            let (node, _) = walk(&tokens(&["agent"]), ctx, SHADOWED).expect("agent");
            // Compare the *rendered body* against the overlay's
            // sentinel marker. `std::ptr::eq` would be tempting but
            // const-slice references aren't guaranteed to share
            // identity across use sites in Rust; comparing content is
            // the stable check.
            let rendered = node.body.render(ctx);
            assert!(
                !rendered.contains("OVERLAY_SHADOW_SENTINEL"),
                "OSS BUILTINS must win over overlay on name collision; \
                 walk returned the overlay node instead of the built-in"
            );
        });
    }

    #[test]
    fn subcommand_topics_skipped_in_overview_topics_list() {
        let cmd = Cli::command();
        let ctx = Context {
            program_name: "dapper",
            clap_cmd: &cmd,
        };
        let visible: Vec<&Topic> = topics::BUILTINS
            .iter()
            .filter(|t| (t.visible)(&ctx))
            .filter(|t| !is_subcommand_topic(t, ctx.clap_cmd))
            .collect();
        let names: Vec<&str> = visible.iter().map(|t| t.name).collect();
        assert!(!names.contains(&"debug"), "debug is a subcommand");
        assert!(!names.contains(&"proxy"), "proxy is a subcommand");
        assert!(!names.contains(&"mcp"), "mcp is a subcommand");
        assert!(names.contains(&"agent"));
        assert!(names.contains(&"sessions"));
        assert!(names.contains(&"breakpoints"));
        assert!(
            !names.contains(&"workflow"),
            "`workflow` should NOT be a top-level topic; it merged into `agent`"
        );
    }

    #[test]
    fn topic_with_alias_shadowing_subcommand_is_treated_as_subcommand_topic() {
        // `is_subcommand_topic` checks every alias as well as the
        // canonical name; a topic whose name is unique but whose alias
        // collides with a clap subcommand still gets filtered from
        // the overview's `## Available Topics`.
        const T: Topic = Topic {
            name: "unique-overlay-only",
            aliases: &["debug"],
            summary: "",
            body: Body::Static(""),
            visible: always,
            children: &[],
        };
        let cmd = Cli::command();
        assert!(is_subcommand_topic(&T, &cmd));
    }

    #[test]
    fn no_unsubstituted_program_token_in_any_builtin_body() {
        // After substitute(), no `{{program}}` should remain in any
        // OSS topic body — catches missing tokens or future renames.
        with_ctx("dapper", |ctx| {
            for t in topics::BUILTINS {
                let rendered = render::substitute(&t.body.render(ctx), "dapper");
                assert!(
                    !rendered.contains("{{"),
                    "topic `{}` body has an unsubstituted `{{{{...}}}}` token",
                    t.name
                );
            }
        });
    }

    #[test]
    fn recursive_clap_traversal_finds_nested_subcommand() {
        let cmd = Cli::command();
        let mut current: &clap::Command = &cmd;
        let mut consumed = 0;
        for token in &["debug".to_string(), "threads".to_string()] {
            let Some(sub) = current.find_subcommand(token) else {
                break;
            };
            current = sub;
            consumed += 1;
        }
        assert_eq!(consumed, 2, "should descend into `debug threads`");
        assert_eq!(current.get_name(), "threads");
    }

    #[test]
    fn clap_partial_match_does_not_consume_unknown_subtoken() {
        // `dapper help debug nonexistent`: `debug` matches a clap
        // subcommand but `nonexistent` does not. Earlier behavior
        // silently rendered `debug` auto-doc and dropped the trailing
        // token — the dispatcher now requires full consumption so this
        // case returns `HelpError::UnknownTopic` instead.
        let cmd = Cli::command();
        let mut current: &clap::Command = &cmd;
        let mut consumed = 0;
        for token in &["debug".to_string(), "nonexistent".to_string()] {
            let Some(sub) = current.find_subcommand(token) else {
                break;
            };
            current = sub;
            consumed += 1;
        }
        assert_eq!(
            consumed, 1,
            "clap should descend into `debug` and stop on `nonexistent`"
        );
        assert_ne!(
            consumed, 2,
            "partial-match path is what `handle` now treats as unknown"
        );
    }
}
