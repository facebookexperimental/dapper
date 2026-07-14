// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::HashMap;

use dapper_dap_protocol::data_types::Breakpoint;
use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::enums::BreakpointEventReason;
use dapper_session::BreakpointInfo;

/// Represents the difference between old and new breakpoints
#[derive(Debug)]
pub struct BreakpointDiff {
    pub to_add: Vec<BreakpointInfo>,
    pub to_remove: Vec<BreakpointInfo>,
}

/// Tracks setBreakpoints requests to match with responses
#[derive(Debug, Clone)]
struct PendingBreakpointRequest {
    source_path: String,
    specs: Vec<SourceBreakpoint>,
}

/// Encapsulates all breakpoint-related state
#[derive(Debug, Default)]
pub(super) struct BreakpointState {
    pub breakpoints: HashMap<String, Vec<BreakpointInfo>>,
    pending_requests: HashMap<Seq, PendingBreakpointRequest>,
    /// Maps original request paths to resolved paths when the debug adapter
    /// returns a different source path in the response. Used as a fallback
    /// in `get_breakpoints` when the exact path lookup returns no results.
    path_aliases: HashMap<String, String>,
}

impl BreakpointState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Track a setBreakpoints request
    pub fn track_request(&mut self, seq: Seq, source_path: String, specs: Vec<SourceBreakpoint>) {
        self.pending_requests
            .insert(seq, PendingBreakpointRequest { source_path, specs });
    }

    /// Update breakpoints for a source file based on a setBreakpoints response,
    /// using the stored request specs for positional fallback when the response
    /// omits the `line` field.
    pub fn update_breakpoints_from_response(
        &mut self,
        request_seq: Seq,
        response_breakpoints: &[Breakpoint],
    ) -> bool {
        if let Some(request) = self.pending_requests.remove(&request_seq) {
            let breakpoints = breakpoints_with_fallback(response_breakpoints, &request.specs);
            if let Some(resolved) = resolved_source_path(response_breakpoints) {
                tracing::debug!(
                    request_seq = request_seq.as_i64(),
                    source_path = %resolved,
                    breakpoint_count = breakpoints.len(),
                    "Updating breakpoints from response (resolved path)"
                );
                self.update_breakpoints(resolved, breakpoints);
                if resolved != request.source_path {
                    self.path_aliases
                        .insert(request.source_path, resolved.to_string());
                }
            } else {
                tracing::debug!(
                    request_seq = request_seq.as_i64(),
                    source_path = %request.source_path,
                    breakpoint_count = breakpoints.len(),
                    "Updating breakpoints from response"
                );
                self.update_breakpoints(&request.source_path, breakpoints);
            }
            true
        } else {
            tracing::warn!(
                request_seq = request_seq.as_i64(),
                "Received setBreakpoints response without matching request - source path unknown, skipping update"
            );
            false
        }
    }

    /// Update breakpoints for a source file directly (for explicit source path)
    pub fn update_breakpoints(&mut self, source_path: &str, breakpoints: Vec<BreakpointInfo>) {
        self.breakpoints
            .insert(source_path.to_string(), breakpoints);
    }

    /// Get current breakpoints for a source file. Falls back to checking
    /// path aliases if no breakpoints are found under the exact path.
    pub fn get_breakpoints(&self, source_path: &str) -> Vec<BreakpointInfo> {
        if let Some(bps) = self.breakpoints.get(source_path) {
            return bps.clone();
        }
        if let Some(resolved) = self.path_aliases.get(source_path)
            && let Some(bps) = self.breakpoints.get(resolved.as_str())
        {
            return bps.clone();
        }
        Vec::new()
    }

    pub(super) fn record_path_alias(&mut self, original_path: &str, resolved_path: &str) {
        self.path_aliases
            .insert(original_path.to_string(), resolved_path.to_string());
    }

    /// Apply a breakpoint event from the debug adapter to update tracked state.
    /// The DAP spec says the `id` field identifies the target breakpoint, and
    /// other attributes provide new values.
    pub fn apply_breakpoint_event(&mut self, reason: &BreakpointEventReason, bp: &Breakpoint) {
        let Some(bp_id) = bp.id else {
            tracing::warn!("Ignoring breakpoint event with no id");
            return;
        };

        match reason {
            BreakpointEventReason::Changed => {
                let event_source_path = bp.source.as_ref().and_then(|s| s.path.as_deref());

                let found = self.breakpoints.iter().find_map(|(file_path, bps)| {
                    bps.iter()
                        .position(|b| b.id == Some(bp_id))
                        .map(|pos| (file_path.clone(), pos))
                });

                let Some((file_path, pos)) = found else {
                    tracing::debug!(
                        bp_id = ?bp_id,
                        "Breakpoint changed event for unknown breakpoint id"
                    );
                    return;
                };

                let breakpoints = self.breakpoints.get_mut(&file_path).unwrap();
                if let Some(new_path) = event_source_path
                    && new_path != file_path.as_str()
                {
                    let mut moved = breakpoints.remove(pos);
                    moved.verified = bp.verified;
                    if let Some(line) = bp.line {
                        if line >= 0 {
                            moved.line = line;
                        } else {
                            tracing::debug!(bp_id = ?bp_id, line = line, "Ignoring negative line in breakpoint changed event");
                        }
                    }
                    self.breakpoints
                        .entry(new_path.to_string())
                        .or_default()
                        .push(moved);
                    self.breakpoints.retain(|_, bps| !bps.is_empty());
                    self.path_aliases
                        .retain(|_, resolved| self.breakpoints.contains_key(resolved));
                    return;
                }

                let existing = &mut breakpoints[pos];
                existing.verified = bp.verified;
                if let Some(line) = bp.line {
                    if line >= 0 {
                        existing.line = line;
                    } else {
                        tracing::debug!(bp_id = ?bp_id, line = line, "Ignoring negative line in breakpoint changed event");
                    }
                }
            }
            BreakpointEventReason::Removed => {
                for breakpoints in self.breakpoints.values_mut() {
                    if let Some(pos) = breakpoints.iter().position(|b| b.id == Some(bp_id)) {
                        breakpoints.remove(pos);
                        break;
                    }
                }
                self.breakpoints.retain(|_, bps| !bps.is_empty());
                self.path_aliases
                    .retain(|_, resolved| self.breakpoints.contains_key(resolved));
            }
            BreakpointEventReason::New => {
                let source_path = bp.source.as_ref().and_then(|s| s.path.as_deref());
                let Some(source_path) = source_path else {
                    tracing::warn!(
                        bp_id = ?bp_id,
                        "Ignoring new breakpoint event with no source path"
                    );
                    return;
                };
                let Some(line) = bp.line else {
                    tracing::warn!(
                        bp_id = ?bp_id,
                        "Ignoring new breakpoint event with no line"
                    );
                    return;
                };
                if line < 0 {
                    tracing::debug!(
                        bp_id = ?bp_id,
                        line = line,
                        "Ignoring new breakpoint event with negative line"
                    );
                    return;
                }
                if self
                    .breakpoints
                    .values()
                    .any(|bps| bps.iter().any(|b| b.id == Some(bp_id)))
                {
                    tracing::debug!(
                        bp_id = ?bp_id,
                        "Ignoring new breakpoint event for already-tracked id"
                    );
                    return;
                }
                let info = BreakpointInfo {
                    line,
                    verified: bp.verified,
                    id: Some(bp_id),
                    condition: None,
                    log_message: None,
                    ..Default::default()
                };
                self.breakpoints
                    .entry(source_path.to_string())
                    .or_default()
                    .push(info);
            }
            _ => {
                tracing::debug!(
                    reason = ?reason,
                    "Ignoring breakpoint event with unknown reason"
                );
            }
        }
    }
}

/// Build BreakpointInfo entries from DAP response breakpoints, falling back to
/// the positionally-matched request spec's line when the response omits `line`.
/// The DAP spec guarantees the response array is 1:1 with the request array.
pub(crate) fn breakpoints_with_fallback(
    response_breakpoints: &[Breakpoint],
    request_specs: &[SourceBreakpoint],
) -> Vec<BreakpointInfo> {
    response_breakpoints
        .iter()
        .enumerate()
        .filter_map(|(i, bp)| {
            let spec = request_specs.get(i);
            let line = bp.line.or_else(|| spec.map(|s| s.line));
            let Some(line) = line else {
                tracing::warn!(
                    index = i,
                    "Breakpoint response missing line and no matching request spec"
                );
                return None;
            };
            Some(BreakpointInfo {
                line,
                verified: bp.verified,
                id: bp.id,
                condition: spec.and_then(|s| s.condition.clone()),
                log_message: spec.and_then(|s| s.log_message.clone()),
                ..Default::default()
            })
        })
        .collect()
}

/// Extract the resolved source path from response breakpoints if any breakpoint
/// includes a `source.path`. Returns `None` if no breakpoint has source info.
pub(crate) fn resolved_source_path(response_breakpoints: &[Breakpoint]) -> Option<&str> {
    response_breakpoints
        .iter()
        .find_map(|bp| bp.source.as_ref().and_then(|s| s.path.as_deref()))
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::data_types::BreakpointId;
    use dapper_dap_protocol::data_types::Source;

    use super::*;

    #[test]
    fn test_from_breakpoint_info_to_source_breakpoint() {
        let info = BreakpointInfo {
            line: 42,
            verified: true,
            id: Some(BreakpointId(5)),
            condition: Some("x > 0".into()),
            log_message: Some("hit".into()),
            ..Default::default()
        };

        let sbp = SourceBreakpoint::from(&info);
        assert_eq!(sbp.line, 42);
        assert_eq!(sbp.condition, Some("x > 0".into()));
        assert_eq!(sbp.log_message, Some("hit".into()));
    }

    #[test]
    fn test_breakpoint_state_track_and_update() {
        let mut state = BreakpointState::new();
        state.track_request(
            Seq(1),
            "/test.rs".into(),
            vec![SourceBreakpoint {
                line: 10,
                ..Default::default()
            }],
        );

        let updated = state.update_breakpoints_from_response(
            Seq(1),
            &[Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(10),
                ..Default::default()
            }],
        );
        assert!(updated);

        let breakpoints = state.get_breakpoints("/test.rs");
        assert_eq!(breakpoints.len(), 1);
        assert_eq!(breakpoints[0].line, 10);
        assert!(breakpoints[0].verified);
        assert_eq!(breakpoints[0].id, Some(BreakpointId(1)));
    }

    #[test]
    fn test_breakpoint_state_unmatched_response() {
        let mut state = BreakpointState::new();

        let updated = state.update_breakpoints_from_response(Seq(999), &[]);
        assert!(!updated);

        let breakpoints = state.get_breakpoints("/any.rs");
        assert!(breakpoints.is_empty());
    }

    #[test]
    fn test_to_dap_breakpoint() {
        let info = BreakpointInfo {
            line: 42,
            verified: true,
            id: Some(BreakpointId(5)),
            condition: None,
            log_message: None,
            ..Default::default()
        };

        let bp = info.to_dap_breakpoint("/src/main.rs", "main.rs");
        assert_eq!(bp.line, Some(42));
        assert!(bp.verified);
        assert_eq!(bp.id, Some(BreakpointId(5)));
        let source = bp.source.unwrap();
        assert_eq!(source.path, Some("/src/main.rs".into()));
        assert_eq!(source.name, Some("main.rs".into()));
    }

    #[test]
    fn test_apply_breakpoint_event_changed() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![
                BreakpointInfo {
                    line: 10,
                    verified: false,
                    id: Some(BreakpointId(1)),
                    ..Default::default()
                },
                BreakpointInfo {
                    line: 20,
                    verified: true,
                    id: Some(BreakpointId(2)),
                    ..Default::default()
                },
            ],
        );

        state.apply_breakpoint_event(
            &BreakpointEventReason::Changed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(12),
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps.len(), 2);
        assert_eq!(bps[0].line, 12);
        assert!(bps[0].verified);
        assert_eq!(bps[1].line, 20);
    }

    #[test]
    fn test_apply_breakpoint_event_changed_preserves_line_when_none() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: false,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );

        state.apply_breakpoint_event(
            &BreakpointEventReason::Changed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: None,
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 10);
        assert!(bps[0].verified);
    }

    #[test]
    fn test_apply_breakpoint_event_removed() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![
                BreakpointInfo {
                    line: 10,
                    verified: true,
                    id: Some(BreakpointId(1)),
                    ..Default::default()
                },
                BreakpointInfo {
                    line: 20,
                    verified: true,
                    id: Some(BreakpointId(2)),
                    ..Default::default()
                },
            ],
        );

        state.apply_breakpoint_event(
            &BreakpointEventReason::Removed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: false,
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 20);
        assert_eq!(bps[0].id, Some(BreakpointId(2)));
    }

    #[test]
    fn test_apply_breakpoint_event_new() {
        let mut state = BreakpointState::new();

        state.apply_breakpoint_event(
            &BreakpointEventReason::New,
            &Breakpoint {
                id: Some(BreakpointId(5)),
                verified: true,
                line: Some(42),
                source: Some(Source {
                    path: Some("/test.rs".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 42);
        assert!(bps[0].verified);
        assert_eq!(bps[0].id, Some(BreakpointId(5)));
    }

    #[test]
    fn test_apply_breakpoint_event_no_id_ignored() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: true,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );

        state.apply_breakpoint_event(
            &BreakpointEventReason::Changed,
            &Breakpoint {
                id: None,
                verified: false,
                line: Some(99),
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 10);
        assert!(bps[0].verified);
    }

    #[test]
    fn test_apply_breakpoint_event_changed_moves_source() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/old/path.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: true,
                id: Some(BreakpointId(1)),
                condition: Some("x > 5".to_string()),
                ..Default::default()
            }],
        );

        state.apply_breakpoint_event(
            &BreakpointEventReason::Changed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(12),
                source: Some(Source {
                    path: Some("/new/path.rs".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        assert!(state.get_breakpoints("/old/path.rs").is_empty());
        assert!(!state.breakpoints.contains_key("/old/path.rs"));

        let bps = state.get_breakpoints("/new/path.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 12);
        assert_eq!(bps[0].id, Some(BreakpointId(1)));
        assert_eq!(bps[0].condition, Some("x > 5".to_string()));
    }

    #[test]
    fn test_apply_breakpoint_event_removed_cleans_up_empty_entry() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: true,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );

        state.apply_breakpoint_event(
            &BreakpointEventReason::Removed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                ..Default::default()
            },
        );

        assert!(!state.breakpoints.contains_key("/test.rs"));
    }

    #[test]
    fn test_apply_breakpoint_event_new_duplicate_id_ignored() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: true,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );

        state.apply_breakpoint_event(
            &BreakpointEventReason::New,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(99),
                source: Some(Source {
                    path: Some("/other.rs".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(bps[0].line, 10);
        assert!(state.get_breakpoints("/other.rs").is_empty());
    }

    #[test]
    fn test_update_breakpoints_from_response_with_resolved_source() {
        let mut state = BreakpointState::new();
        state.track_request(
            Seq(1),
            "/requested/path.rs".into(),
            vec![SourceBreakpoint {
                line: 10,
                ..Default::default()
            }],
        );

        let updated = state.update_breakpoints_from_response(
            Seq(1),
            &[Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(10),
                source: Some(Source {
                    path: Some("/resolved/path.rs".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        );
        assert!(updated);

        let bps_by_resolved = state.get_breakpoints("/resolved/path.rs");
        assert_eq!(bps_by_resolved.len(), 1);
        assert_eq!(bps_by_resolved[0].line, 10);

        let bps_by_original = state.get_breakpoints("/requested/path.rs");
        assert_eq!(
            bps_by_original.len(),
            1,
            "alias fallback should find breakpoints via original path"
        );
        assert_eq!(bps_by_original[0].line, 10);
    }

    #[test]
    fn test_path_alias_direct() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/resolved/path.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: true,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );
        state.record_path_alias("/original/path.rs", "/resolved/path.rs");

        let bps = state.get_breakpoints("/original/path.rs");
        assert_eq!(bps.len(), 1, "alias should resolve to breakpoints");
        assert_eq!(bps[0].line, 10);

        let bps_direct = state.get_breakpoints("/resolved/path.rs");
        assert_eq!(bps_direct.len(), 1, "direct lookup should still work");
    }

    #[test]
    fn test_apply_breakpoint_event_changed_ignores_negative_line() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![BreakpointInfo {
                line: 15,
                verified: false,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );

        // Simulate HHVM sending a negative line in a changed event
        state.apply_breakpoint_event(
            &BreakpointEventReason::Changed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(-98),
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(
            bps[0].line, 15,
            "original line should be preserved when event has negative line"
        );
        assert!(bps[0].verified, "verified should still be updated");
    }

    #[test]
    fn test_apply_breakpoint_event_changed_accepts_zero_line() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/test.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: false,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );

        // Line 0 is valid when linesStartAt1 is false (0-based indexing)
        state.apply_breakpoint_event(
            &BreakpointEventReason::Changed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(0),
                ..Default::default()
            },
        );

        let bps = state.get_breakpoints("/test.rs");
        assert_eq!(bps[0].line, 0, "line 0 is valid for 0-based line numbering");
    }

    #[test]
    fn test_apply_breakpoint_event_changed_move_ignores_negative_line() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/old.rs",
            vec![BreakpointInfo {
                line: 15,
                verified: true,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );

        // Changed event that moves to a new file but has a negative line
        state.apply_breakpoint_event(
            &BreakpointEventReason::Changed,
            &Breakpoint {
                id: Some(BreakpointId(1)),
                verified: true,
                line: Some(-98),
                source: Some(Source {
                    path: Some("/new.rs".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        // Breakpoint should move to new file but keep original line
        assert!(state.get_breakpoints("/old.rs").is_empty());
        let bps = state.get_breakpoints("/new.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(
            bps[0].line, 15,
            "original line should be preserved when move event has negative line"
        );
    }

    #[test]
    fn test_apply_breakpoint_event_new_ignores_negative_line() {
        let mut state = BreakpointState::new();

        state.apply_breakpoint_event(
            &BreakpointEventReason::New,
            &Breakpoint {
                id: Some(BreakpointId(5)),
                verified: true,
                line: Some(-98),
                source: Some(Source {
                    path: Some("/test.rs".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        assert!(
            state.get_breakpoints("/test.rs").is_empty(),
            "new breakpoint with negative line should be rejected"
        );
    }

    #[test]
    fn test_path_alias_not_used_when_direct_match_exists() {
        let mut state = BreakpointState::new();
        state.update_breakpoints(
            "/path_a.rs",
            vec![BreakpointInfo {
                line: 10,
                verified: true,
                id: Some(BreakpointId(1)),
                ..Default::default()
            }],
        );
        state.update_breakpoints(
            "/path_b.rs",
            vec![BreakpointInfo {
                line: 20,
                verified: true,
                id: Some(BreakpointId(2)),
                ..Default::default()
            }],
        );
        state.record_path_alias("/path_a.rs", "/path_b.rs");

        let bps = state.get_breakpoints("/path_a.rs");
        assert_eq!(bps.len(), 1);
        assert_eq!(
            bps[0].line, 10,
            "direct match should take priority over alias"
        );
    }
}
