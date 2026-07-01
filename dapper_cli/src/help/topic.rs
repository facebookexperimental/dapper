// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Unified topic registry types — used by both the OSS built-in topics
//! and any embedder-supplied overlay tree.
//!
//! A [`Topic`] is the recursive node: it carries the canonical name,
//! aliases, summary, body, visibility predicate, and optional children.
//! The dispatcher walks the merged `BUILTINS + overlay` tree, and a
//! topic with non-empty visible children gets a `## Subtopics` index
//! auto-appended to its rendered body.

use std::borrow::Cow;

/// Per-invocation context threaded through every render and visibility
/// predicate. The dispatcher constructs it once before walking the
/// topic tree.
pub struct Context<'a> {
    /// User-facing program name, resolved from argv[0] via
    /// `crate::program_name::from_args`. Examples: `"dapper"`,
    /// `"fdb dapper"`, `"meta dapper"`.
    pub program_name: &'a str,

    /// The clap command tree as a *schema* — used by the recursive
    /// command-fallback path in `dispatch::handle` and by the overview
    /// renderer when it filters out topics whose name shadows a clap
    /// subcommand. This is constructed via `Cli::command()` purely for
    /// introspection; it does not carry parser state (no defaults
    /// applied, no values mutated). Treat it as read-only metadata.
    pub clap_cmd: &'a clap::Command,
}

/// One node in the unified topic tree.
///
/// Built-in OSS topics live in [`crate::help::topics::BUILTINS`].
/// Embedders (e.g. `dapper_fb_main`) supply their own
/// `&'static [Topic]` slice that the dispatcher merges with the
/// built-ins at lookup time.
pub struct Topic {
    /// Canonical name — the *last* path segment of the topic's address.
    /// `"breakpoints"` for the topic reachable as `dapper help breakpoints`,
    /// `"lldb"` for an embedder-supplied topic at `dapper help debuggers lldb`.
    pub name: &'static str,

    /// Alternate names matched at the same tree level. The OSS
    /// `breakpoints` topic declares `aliases: &["breakpoint", "bp"]`
    /// so all three spellings resolve to the same node; embedder
    /// overlays use the same mechanism.
    pub aliases: &'static [&'static str],

    /// One-line description shown in the parent's auto-generated
    /// `## Subtopics` index and (for top-level topics) in the bare
    /// overview's `## Available Topics` listing.
    pub summary: &'static str,

    /// Topic body. [`Body::Static`] holds an `include_str!`'d markdown
    /// payload; [`Body::Dynamic`] is a callback that materializes the
    /// body at render time (used when content depends on runtime state,
    /// e.g. the MCP toolset table derived from
    /// `dapper_mcp_server::BuiltinToolset`).
    pub body: Body,

    /// Visibility predicate. Gates both lookup and listing. Use
    /// [`always`] for topics that are always reachable; use a custom
    /// predicate for invocation-gated topics like the fdb-only
    /// `headless` topic.
    pub visible: fn(&Context<'_>) -> bool,

    /// Nested subtopics. When non-empty, the dispatcher appends a
    /// `## Subtopics` index to the parent's body listing every visible
    /// child by `name` and `summary`.
    ///
    /// Names and aliases must be unique within a sibling group — the
    /// dispatcher walks `children.iter().find(...)` and returns the
    /// first visible match, so duplicates produce silent shadowing.
    pub children: &'static [Topic],
}

impl Topic {
    /// True if this topic answers to `segment` either by canonical name
    /// or by one of its aliases. Used during tree traversal to find the
    /// next node along a multi-token path.
    pub fn matches(&self, segment: &str) -> bool {
        self.name == segment || self.aliases.contains(&segment)
    }
}

/// Topic body, static or computed.
pub enum Body {
    /// A markdown payload baked into the binary at compile time, almost
    /// always via `include_str!`.
    Static(&'static str),

    /// A callback that materializes the body at render time. The
    /// returned `Cow` lets static-text generators avoid allocating
    /// while still admitting fully-owned `String` results from
    /// generators that compute their content (e.g. the MCP toolset
    /// table).
    Dynamic(fn(&Context<'_>) -> Cow<'static, str>),
}

impl Body {
    /// Materialize the body for the current invocation. The `{{program}}`
    /// substitution still happens downstream — every print path in the
    /// dispatcher routes through `render::substitute` before emitting
    /// bytes, so dynamic bodies should leave program tokens intact.
    pub fn render(&self, ctx: &Context<'_>) -> Cow<'static, str> {
        match self {
            Self::Static(s) => Cow::Borrowed(*s),
            Self::Dynamic(f) => f(ctx),
        }
    }
}

/// Visibility predicate that always returns `true`. Convenience for
/// topics that should be reachable in every invocation.
pub fn always(_: &Context<'_>) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::test_util::with_ctx;

    #[test]
    fn matches_canonical_name() {
        let t = Topic {
            name: "breakpoints",
            aliases: &[],
            summary: "",
            body: Body::Static(""),
            visible: always,
            children: &[],
        };
        assert!(t.matches("breakpoints"));
        assert!(!t.matches("lldb"));
    }

    #[test]
    fn matches_alias() {
        let t = Topic {
            name: "breakpoints",
            aliases: &["breakpoint", "bp"],
            summary: "",
            body: Body::Static(""),
            visible: always,
            children: &[],
        };
        assert!(t.matches("breakpoints"));
        assert!(t.matches("breakpoint"));
        assert!(t.matches("bp"));
        assert!(!t.matches("lldb"));
    }

    #[test]
    fn body_static_renders_borrowed() {
        let body = Body::Static("hello {{program}}");
        with_ctx("dapper", |ctx| {
            let rendered = body.render(ctx);
            assert!(matches!(rendered, Cow::Borrowed(_)));
            assert_eq!(rendered.as_ref(), "hello {{program}}");
        });
    }

    #[test]
    fn body_dynamic_renders_owned_with_context() {
        fn dyn_body(ctx: &Context<'_>) -> Cow<'static, str> {
            Cow::Owned(format!("invoked as {}", ctx.program_name))
        }
        let body = Body::Dynamic(dyn_body);
        with_ctx("fdb dapper", |ctx| {
            let rendered = body.render(ctx);
            assert!(matches!(rendered, Cow::Owned(_)));
            assert_eq!(rendered.as_ref(), "invoked as fdb dapper");
        });
    }

    #[test]
    fn always_visible_returns_true_for_any_program() {
        with_ctx("dapper", |ctx| assert!(always(ctx)));
        with_ctx("fdb dapper", |ctx| assert!(always(ctx)));
    }
}
