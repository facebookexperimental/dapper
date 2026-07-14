// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Tracks the set of exception breakpoint filters currently installed on the
//! debug adapter, plus pending `setExceptionBreakpoints` requests awaiting
//! responses.
//!
//! This state mirrors the wire-level reality: `installed` reflects only what
//! was sent in the most recent successful `setExceptionBreakpoints`. Both the
//! installed list and pending request entries are kept sorted by filter id
//! so context output and test assertions are deterministic.

use std::collections::BTreeMap;
use std::collections::HashMap;

use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::requests::SetExceptionBreakpointsArguments;
use dapper_session::ExceptionFilterEntry;

#[derive(Debug, Default)]
pub(super) struct ExceptionFilterState {
    /// The set of exception filters currently installed on the adapter,
    /// reflecting the most recent successful `setExceptionBreakpoints`
    /// response. Sorted by `filter` id. Kept private so writers go through
    /// `replace`/`complete_response`/`track_request` and the sort
    /// invariant is enforced by construction.
    installed: Vec<ExceptionFilterEntry>,
    /// In-flight `setExceptionBreakpoints` requests, keyed by client-frame
    /// `Seq`. Each entry's `Vec<ExceptionFilterEntry>` is sorted by `filter`
    /// id and represents what the request was attempting to install. When
    /// the matching response arrives, the entry is popped and (on success)
    /// becomes the new `installed` set. If a response never arrives (e.g.
    /// adapter crash mid-flight), the entry stays here for the rest of the
    /// session — same trade-off as `BreakpointState::pending_requests`.
    pending_requests: HashMap<Seq, Vec<ExceptionFilterEntry>>,
}

impl ExceptionFilterState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a pending `setExceptionBreakpoints` request keyed by its
    /// client-frame `Seq`. Entries are sorted by `filter` id on store so
    /// the eventual `installed` set is deterministic regardless of caller
    /// order.
    pub fn track_request(&mut self, seq: Seq, mut entries: Vec<ExceptionFilterEntry>) {
        entries.sort_unstable_by(|a, b| a.filter.cmp(&b.filter));
        self.pending_requests.insert(seq, entries);
    }

    /// Complete a pending `setExceptionBreakpoints` request. Always pops
    /// the pending entry (returning `true` if one existed). On `success`,
    /// the popped entries become the new `installed` set; on failure, they
    /// are discarded so stale pending entries don't accumulate.
    pub fn complete_response(&mut self, seq: Seq, success: bool) -> bool {
        match self.pending_requests.remove(&seq) {
            Some(entries) => {
                if success {
                    self.installed = entries;
                }
                true
            }
            None => {
                tracing::warn!(
                    request_seq = seq.as_i64(),
                    "received setExceptionBreakpoints response without matching pending request"
                );
                false
            }
        }
    }

    /// Replace the installed set wholesale. Used by control-plane callers
    /// (MCP/CLI) whose responses bypass `track_message_to_client`. The
    /// caller is responsible for passing the post-builder *effective* set
    /// — i.e. what was actually accepted by the adapter.
    pub fn replace(&mut self, mut entries: Vec<ExceptionFilterEntry>) {
        entries.sort_unstable_by(|a, b| a.filter.cmp(&b.filter));
        self.installed = entries;
    }

    pub fn get_installed(&self) -> &[ExceptionFilterEntry] {
        &self.installed
    }
}

/// Parse a `SetExceptionBreakpointsArguments` payload (e.g. from an
/// IDE-issued request) into a sorted, deduped `Vec<ExceptionFilterEntry>`.
/// Combines the bare `filters` array and the `filter_options` array; if the
/// same filter id appears in both, the `filter_options` entry wins because
/// it carries the richer (condition-bearing) information.
///
/// **Out-of-scope fields**: `filterOptions[i].mode` and `args.exception_options`
/// are intentionally dropped — they have no `filter` id and the v1 tracker
/// schema doesn't model them. A debug log is emitted if either is non-empty
/// so users have a paper trail when those fields silently disappear (e.g. on
/// a subsequent control-plane call that replaces the active set).
pub(super) fn parse_request_entries(
    args: &SetExceptionBreakpointsArguments,
) -> Vec<ExceptionFilterEntry> {
    // BTreeMap so iteration order is sorted by filter id without a
    // separate sort step, and so `filter_options` overwriting a
    // `filters`-only entry for the same id (the documented dedup
    // behavior) is a single insert.
    let mut by_filter: BTreeMap<String, ExceptionFilterEntry> = BTreeMap::new();

    for filter in &args.filters {
        by_filter
            .entry(filter.clone())
            .or_insert_with(|| ExceptionFilterEntry {
                filter: filter.clone(),
                condition: None,
            });
    }

    if let Some(opts) = &args.filter_options {
        let dropped_mode_filters: Vec<&str> = opts
            .iter()
            .filter(|o| o.mode.is_some())
            .map(|o| o.filter_id.as_str())
            .collect();
        if !dropped_mode_filters.is_empty() {
            tracing::debug!(
                filter_ids = ?dropped_mode_filters,
                "filterOptions[].mode is not modeled in the dapper tracker; dropping"
            );
        }
        for opt in opts {
            // filter_options always wins over a bare filter with the same id.
            let filter_id = opt.filter_id.clone();
            by_filter.insert(
                filter_id.clone(),
                ExceptionFilterEntry {
                    filter: filter_id,
                    condition: opt.condition.clone(),
                },
            );
        }
    }

    if let Some(exc_opts) = &args.exception_options
        && !exc_opts.is_empty()
    {
        tracing::debug!(
            count = exc_opts.len(),
            "args.exception_options is not modeled in the dapper tracker; dropping"
        );
    }

    by_filter.into_values().collect()
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::data_types::ExceptionFilterOptions;
    use dapper_dap_protocol::data_types::ExceptionOptions;

    use super::*;

    #[test]
    fn test_track_request_sorts_entries() {
        let mut state = ExceptionFilterState::new();
        state.track_request(
            Seq(1),
            vec![
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: None,
                },
            ],
        );
        let popped = state.pending_requests.get(&Seq(1)).unwrap();
        let ids: Vec<&str> = popped.iter().map(|e| e.filter.as_str()).collect();
        assert_eq!(ids, vec!["raised", "uncaught"]);
    }

    #[test]
    fn test_complete_response_success_replaces_installed() {
        let mut state = ExceptionFilterState::new();
        state.track_request(
            Seq(7),
            vec![ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: None,
            }],
        );
        let had_pending = state.complete_response(Seq(7), true);
        assert!(had_pending);
        assert_eq!(state.get_installed().len(), 1);
        assert_eq!(state.get_installed()[0].filter, "raised");
        assert!(state.pending_requests.is_empty());
    }

    #[test]
    fn test_complete_response_failure_pops_without_replacing() {
        let mut state = ExceptionFilterState::new();
        state.replace(vec![ExceptionFilterEntry {
            filter: "old".to_string(),
            condition: None,
        }]);
        state.track_request(
            Seq(2),
            vec![ExceptionFilterEntry {
                filter: "new".to_string(),
                condition: None,
            }],
        );
        let had_pending = state.complete_response(Seq(2), false);
        assert!(had_pending);
        // installed unchanged
        assert_eq!(state.get_installed().len(), 1);
        assert_eq!(state.get_installed()[0].filter, "old");
        // pending cleaned up
        assert!(state.pending_requests.is_empty());
    }

    #[test]
    fn test_complete_response_without_matching_pending_warns_and_returns_false() {
        let mut state = ExceptionFilterState::new();
        let had_pending = state.complete_response(Seq(99), true);
        assert!(!had_pending);
        assert!(state.get_installed().is_empty());
    }

    #[test]
    fn test_replace_sorts_entries() {
        let mut state = ExceptionFilterState::new();
        state.replace(vec![
            ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            },
            ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: Some("x>5".to_string()),
            },
            ExceptionFilterEntry {
                filter: "thrown".to_string(),
                condition: None,
            },
        ]);
        let ids: Vec<&str> = state
            .get_installed()
            .iter()
            .map(|e| e.filter.as_str())
            .collect();
        assert_eq!(ids, vec!["raised", "thrown", "uncaught"]);
    }

    #[test]
    fn test_parse_request_entries_combines_filters_and_filter_options() {
        let args = SetExceptionBreakpointsArguments {
            filters: vec!["uncaught".to_string()],
            filter_options: Some(vec![ExceptionFilterOptions {
                filter_id: "raised".to_string(),
                condition: Some("x>5".to_string()),
                mode: None,
            }]),
            exception_options: None,
            ..Default::default()
        };
        let entries = parse_request_entries(&args);
        assert_eq!(
            entries,
            vec![
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: Some("x>5".to_string()),
                },
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
            ]
        );
    }

    #[test]
    fn test_parse_request_entries_dedupes_with_filter_options_winning() {
        // The same filter id appears in both `filters` and `filter_options`.
        // The `filter_options` entry (with the richer condition) wins.
        let args = SetExceptionBreakpointsArguments {
            filters: vec!["raised".to_string()],
            filter_options: Some(vec![ExceptionFilterOptions {
                filter_id: "raised".to_string(),
                condition: Some("x>5".to_string()),
                mode: None,
            }]),
            exception_options: None,
            ..Default::default()
        };
        let entries = parse_request_entries(&args);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filter, "raised");
        assert_eq!(entries[0].condition.as_deref(), Some("x>5"));
    }

    #[test]
    fn test_parse_request_entries_drops_mode_and_exception_options() {
        let args = SetExceptionBreakpointsArguments {
            filters: vec![],
            filter_options: Some(vec![ExceptionFilterOptions {
                filter_id: "raised".to_string(),
                condition: None,
                mode: Some("always".to_string()),
            }]),
            exception_options: Some(vec![ExceptionOptions {
                path: None,
                ..Default::default()
            }]),
            ..Default::default()
        };
        let entries = parse_request_entries(&args);
        // mode dropped, exception_options dropped — only the filter id survives.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filter, "raised");
        assert_eq!(entries[0].condition, None);
    }
}
