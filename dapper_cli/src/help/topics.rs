// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Built-in OSS topic registry.
//!
//! Each [`Topic`] entry carries one or more `aliases`, a one-line
//! `summary` for the parent's auto-generated index, and a `body` —
//! either a static `include_str!`'d markdown payload or a dynamic
//! callback that materializes the body at render time.
//!
//! The bare `dapper help` overview is **not** a registry entry — it
//! lives in `dispatch::print_overview` which composes the existing
//! `overview.md` content with auto-generated commands and topics
//! lists.

use std::borrow::Cow;
use std::fmt::Write;

use clap::ValueEnum;
use dapper_mcp_server::BuiltinToolset;
use dapper_mcp_server::DebugTool;

use crate::help::topic::Body;
use crate::help::topic::Context;
use crate::help::topic::Topic;
use crate::help::topic::always;

/// All OSS topics, ordered for the `## Available Topics` listing.
///
/// `agent` deliberately appears first so an agent reading the overview
/// immediately sees a "rules of engagement for AI agents driving
/// Dapper" pointer.
pub const BUILTINS: &[Topic] = &[
    Topic {
        name: "agent",
        aliases: &["agents", "workflow", "workflows"],
        summary: "Driving Dapper end-to-end: rules of engagement, the debugging loop, follow-up topics",
        body: Body::Static(include_str!("topics/agent.md")),
        visible: always,
        children: &[],
    },
    Topic {
        name: "sessions",
        aliases: &["session"],
        summary: "scope-id, control-port, session-id targeting",
        body: Body::Static(include_str!("topics/sessions.md")),
        visible: always,
        children: &[],
    },
    Topic {
        name: "breakpoints",
        aliases: &["breakpoint", "bp"],
        summary: "Breakpoint syntax: line, conditional, log-message, function",
        body: Body::Static(include_str!("topics/breakpoints.md")),
        visible: always,
        children: &[],
    },
    // Subcommand topics — also resolvable via the recursive clap
    // fallback, but keeping curated bodies here lets us add narrative
    // context that clap metadata can't carry. Order matches the clap
    // `Commands` enum.
    Topic {
        name: "debug",
        aliases: &[],
        summary: "Debug client: inspect threads, stack, variables; set breakpoints; raw DAP",
        body: Body::Static(include_str!("topics/debug.md")),
        visible: always,
        children: &[],
    },
    Topic {
        name: "proxy",
        aliases: &[],
        summary: "Start the DAP proxy: process, tcp, uds, from-config backends",
        body: Body::Static(include_str!("topics/proxy.md")),
        visible: always,
        children: &[],
    },
    Topic {
        name: "mcp",
        aliases: &[],
        summary: "MCP server: toolsets, --enable-tool, scope/session targeting",
        body: Body::Dynamic(render_mcp_topic),
        visible: always,
        children: &[],
    },
];

/// `mcp` topic body — `mcp.md` with `{{toolset_table}}` expanded from
/// `BuiltinToolset::value_variants()`. The substituted markdown still
/// contains `{{program}}` tokens; the dispatcher's
/// `render::substitute` pass swaps those out downstream.
fn render_mcp_topic(_: &Context<'_>) -> Cow<'static, str> {
    const RAW: &str = include_str!("topics/mcp.md");
    Cow::Owned(RAW.replace("{{toolset_table}}", &render_toolset_table()))
}

/// Render the `| Toolset | Tools |` table from the actual
/// `BuiltinToolset` registry. The default-row marker is derived from
/// `BuiltinToolset::default()` so the help UX stays in lockstep with
/// the enum's `#[default]` attribute.
fn render_toolset_table() -> String {
    let default = BuiltinToolset::default();
    let mut out = String::from("| Toolset | Tools |\n|---|---|\n");
    for ts in BuiltinToolset::value_variants() {
        let tools: Vec<String> = ts.tools().iter().map(display_tool_name).collect();
        let marker = if *ts == default { " *(default)*" } else { "" };
        // Writing into `String` is infallible; ignore the `Result`.
        let _ = writeln!(out, "| `{ts}`{marker} | {} |", tools.join(", "));
    }
    out
}

/// Map a [`DebugTool`] variant to its user-friendly display name.
///
/// `DebugTool` derives `strum::AsRefStr`, so `tool.as_ref()` yields the
/// serialize-spelled form (e.g. `"debug_threads_command"`,
/// `"debug_dap_request"`) without requiring the variant to be `Copy`.
/// We then strip the `debug_` prefix and the `_command` suffix (when
/// present) and replace underscores with hyphens to land on
/// `"threads"`, `"stack-trace"`, `"dap-request"`, and so on.
fn display_tool_name(tool: &DebugTool) -> String {
    let raw: &str = tool.as_ref();
    let trimmed = raw.strip_prefix("debug_").unwrap_or(raw);
    let trimmed = trimmed.strip_suffix("_command").unwrap_or(trimmed);
    trimmed.replace('_', "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::test_util::with_ctx;

    #[test]
    fn every_topic_has_name_summary_and_body() {
        fn check(topics: &[Topic]) {
            for t in topics {
                assert!(!t.name.is_empty(), "topic must have a non-empty name");
                assert!(!t.summary.is_empty(), "topic `{}` summary is empty", t.name);
                let static_body = matches!(t.body, Body::Static(s) if !s.is_empty());
                let dynamic_body = matches!(t.body, Body::Dynamic(_));
                assert!(
                    static_body || dynamic_body,
                    "topic `{}` has empty static body",
                    t.name
                );
                check(t.children);
            }
        }
        check(BUILTINS);
    }

    #[test]
    fn names_and_aliases_unique_at_top_level() {
        let mut seen: Vec<&str> = Vec::new();
        for t in BUILTINS {
            for &n in std::iter::once(&t.name).chain(t.aliases.iter()) {
                assert!(
                    !seen.contains(&n),
                    "name or alias `{n}` is registered by more than one top-level topic"
                );
                seen.push(n);
            }
        }
    }

    #[test]
    fn mcp_dynamic_body_lists_every_builtin_toolset() {
        with_ctx("dapper", |ctx| {
            let mcp = BUILTINS
                .iter()
                .find(|t| t.name == "mcp")
                .expect("mcp topic registered");
            let rendered = mcp.body.render(ctx);
            for ts in BuiltinToolset::value_variants() {
                let row_prefix = format!("| `{ts}`");
                assert!(
                    rendered.contains(&row_prefix),
                    "rendered mcp body missing row for toolset `{ts}`:\n{rendered}"
                );
                for tool in ts.tools() {
                    let display = display_tool_name(&tool);
                    assert!(
                        rendered.contains(&display),
                        "rendered mcp body missing tool `{display}` for toolset `{ts}`"
                    );
                }
            }
        });
    }

    #[test]
    fn mcp_dynamic_body_marks_default_toolset() {
        with_ctx("dapper", |ctx| {
            let mcp = BUILTINS
                .iter()
                .find(|t| t.name == "mcp")
                .expect("mcp topic registered");
            let rendered = mcp.body.render(ctx);
            let default = BuiltinToolset::default();
            let expected = format!("| `{default}` *(default)* |");
            assert!(
                rendered.contains(&expected),
                "rendered mcp body should mark `{default}` as default; got:\n{rendered}"
            );
        });
    }

    #[test]
    fn every_declared_alias_resolves_to_its_topic() {
        // Lock in every alias so a future refactor can't accidentally
        // drop one (e.g. `bp` for breakpoints, `workflow` for agent).
        for t in BUILTINS {
            for &alias in t.aliases {
                let resolved = BUILTINS
                    .iter()
                    .find(|x| x.matches(alias))
                    .unwrap_or_else(|| panic!("alias `{alias}` did not resolve"));
                assert_eq!(
                    resolved.name, t.name,
                    "alias `{alias}` resolved to `{}` instead of canonical `{}`",
                    resolved.name, t.name
                );
            }
        }
    }

    #[test]
    fn display_tool_name_strips_prefix_and_suffix() {
        assert_eq!(display_tool_name(&DebugTool::Threads), "threads");
        assert_eq!(display_tool_name(&DebugTool::StackTrace), "stack-trace");
        assert_eq!(
            display_tool_name(&DebugTool::SetBreakpoints),
            "set-breakpoints"
        );
        assert_eq!(display_tool_name(&DebugTool::SetVariable), "set-variable");
        // `dap_request` lacks the `_command` suffix in its strum
        // serialize; the trim still produces the expected result.
        assert_eq!(display_tool_name(&DebugTool::DapRequest), "dap-request");
    }
}
