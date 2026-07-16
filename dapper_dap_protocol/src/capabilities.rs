// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::data_types::BreakpointMode;
use crate::data_types::ColumnDescriptor;
use crate::data_types::ExceptionBreakpointsFilter;
use crate::enums::ChecksumAlgorithm;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_configuration_done_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_function_breakpoints: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_conditional_breakpoints: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_hit_conditional_breakpoints: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_evaluate_for_hovers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_breakpoint_filters: Option<Vec<ExceptionBreakpointsFilter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_step_back: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_set_variable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_restart_frame: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_goto_targets_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_step_in_targets_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_completions_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_trigger_characters: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_modules_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_module_columns: Option<Vec<ColumnDescriptor>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_checksum_algorithms: Option<Vec<ChecksumAlgorithm>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_restart_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_exception_options: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_value_formatting_options: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_exception_info_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_terminate_debuggee: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_suspend_debuggee: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_delayed_stack_trace_loading: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_loaded_sources_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_log_points: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_terminate_threads_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_set_expression: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_terminate_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_data_breakpoints: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_read_memory_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_write_memory_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_disassemble_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_cancel_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_breakpoint_locations_request: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_clipboard_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_stepping_granularity: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_instruction_breakpoints: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_exception_filter_options: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_single_thread_execution_requests: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_data_breakpoint_bytes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint_modes: Option<Vec<BreakpointMode>>,
    #[serde(
        default,
        rename = "supportsANSIStyling",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_ansi_styling: Option<bool>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

impl Capabilities {
    /// Merge a `capabilities` event's partial delta: each field takes the
    /// delta's value when set, else keeps ours; `extra` merges key-wise.
    /// The exhaustive struct literal makes a new `Capabilities` field a
    /// compile error here, so nothing can silently drop from the merge.
    pub(crate) fn merge(&mut self, other: Capabilities) {
        let base = std::mem::take(self);
        *self = Capabilities {
            supports_configuration_done_request: other
                .supports_configuration_done_request
                .or(base.supports_configuration_done_request),
            supports_function_breakpoints: other
                .supports_function_breakpoints
                .or(base.supports_function_breakpoints),
            supports_conditional_breakpoints: other
                .supports_conditional_breakpoints
                .or(base.supports_conditional_breakpoints),
            supports_hit_conditional_breakpoints: other
                .supports_hit_conditional_breakpoints
                .or(base.supports_hit_conditional_breakpoints),
            supports_evaluate_for_hovers: other
                .supports_evaluate_for_hovers
                .or(base.supports_evaluate_for_hovers),
            exception_breakpoint_filters: other
                .exception_breakpoint_filters
                .or(base.exception_breakpoint_filters),
            supports_step_back: other.supports_step_back.or(base.supports_step_back),
            supports_set_variable: other.supports_set_variable.or(base.supports_set_variable),
            supports_restart_frame: other.supports_restart_frame.or(base.supports_restart_frame),
            supports_goto_targets_request: other
                .supports_goto_targets_request
                .or(base.supports_goto_targets_request),
            supports_step_in_targets_request: other
                .supports_step_in_targets_request
                .or(base.supports_step_in_targets_request),
            supports_completions_request: other
                .supports_completions_request
                .or(base.supports_completions_request),
            completion_trigger_characters: other
                .completion_trigger_characters
                .or(base.completion_trigger_characters),
            supports_modules_request: other
                .supports_modules_request
                .or(base.supports_modules_request),
            additional_module_columns: other
                .additional_module_columns
                .or(base.additional_module_columns),
            supported_checksum_algorithms: other
                .supported_checksum_algorithms
                .or(base.supported_checksum_algorithms),
            supports_restart_request: other
                .supports_restart_request
                .or(base.supports_restart_request),
            supports_exception_options: other
                .supports_exception_options
                .or(base.supports_exception_options),
            supports_value_formatting_options: other
                .supports_value_formatting_options
                .or(base.supports_value_formatting_options),
            supports_exception_info_request: other
                .supports_exception_info_request
                .or(base.supports_exception_info_request),
            support_terminate_debuggee: other
                .support_terminate_debuggee
                .or(base.support_terminate_debuggee),
            support_suspend_debuggee: other
                .support_suspend_debuggee
                .or(base.support_suspend_debuggee),
            supports_delayed_stack_trace_loading: other
                .supports_delayed_stack_trace_loading
                .or(base.supports_delayed_stack_trace_loading),
            supports_loaded_sources_request: other
                .supports_loaded_sources_request
                .or(base.supports_loaded_sources_request),
            supports_log_points: other.supports_log_points.or(base.supports_log_points),
            supports_terminate_threads_request: other
                .supports_terminate_threads_request
                .or(base.supports_terminate_threads_request),
            supports_set_expression: other
                .supports_set_expression
                .or(base.supports_set_expression),
            supports_terminate_request: other
                .supports_terminate_request
                .or(base.supports_terminate_request),
            supports_data_breakpoints: other
                .supports_data_breakpoints
                .or(base.supports_data_breakpoints),
            supports_read_memory_request: other
                .supports_read_memory_request
                .or(base.supports_read_memory_request),
            supports_write_memory_request: other
                .supports_write_memory_request
                .or(base.supports_write_memory_request),
            supports_disassemble_request: other
                .supports_disassemble_request
                .or(base.supports_disassemble_request),
            supports_cancel_request: other
                .supports_cancel_request
                .or(base.supports_cancel_request),
            supports_breakpoint_locations_request: other
                .supports_breakpoint_locations_request
                .or(base.supports_breakpoint_locations_request),
            supports_clipboard_context: other
                .supports_clipboard_context
                .or(base.supports_clipboard_context),
            supports_stepping_granularity: other
                .supports_stepping_granularity
                .or(base.supports_stepping_granularity),
            supports_instruction_breakpoints: other
                .supports_instruction_breakpoints
                .or(base.supports_instruction_breakpoints),
            supports_exception_filter_options: other
                .supports_exception_filter_options
                .or(base.supports_exception_filter_options),
            supports_single_thread_execution_requests: other
                .supports_single_thread_execution_requests
                .or(base.supports_single_thread_execution_requests),
            supports_data_breakpoint_bytes: other
                .supports_data_breakpoint_bytes
                .or(base.supports_data_breakpoint_bytes),
            breakpoint_modes: other.breakpoint_modes.or(base.breakpoint_modes),
            supports_ansi_styling: other.supports_ansi_styling.or(base.supports_ansi_styling),
            extra: {
                // Delta keys win on collision.
                let mut extra = base.extra;
                extra.extend(other.extra);
                extra
            },
        };
    }
}

/// Apply a `capabilities` event delta to an optional baseline: merge into the
/// existing caps, or adopt the delta when there are none yet.
pub fn apply_capabilities_event(baseline: &mut Option<Capabilities>, delta: Capabilities) {
    match baseline {
        Some(existing) => existing.merge(delta),
        None => *baseline = Some(delta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_overlays_some_preserves_none_and_merges_extra() {
        let mut base = Capabilities {
            supports_step_back: Some(false),
            supports_single_thread_execution_requests: Some(true),
            ..Default::default()
        };
        base.extra.insert("a".to_owned(), Value::from(1));

        let mut delta = Capabilities {
            supports_step_back: Some(true),
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "raised".to_owned(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        delta.extra.insert("b".to_owned(), Value::from(2));

        base.merge(delta);

        assert_eq!(
            base.supports_step_back,
            Some(true),
            "a changed capability should take the delta's value"
        );
        assert_eq!(
            base.supports_single_thread_execution_requests,
            Some(true),
            "capabilities absent from the delta must keep their prior value"
        );
        assert_eq!(
            base.exception_breakpoint_filters
                .as_ref()
                .map(|f| f[0].filter.as_str()),
            Some("raised"),
            "a capability the baseline lacked should be picked up from the delta"
        );
        assert_eq!(base.extra.get("a"), Some(&Value::from(1)));
        assert_eq!(base.extra.get("b"), Some(&Value::from(2)));
    }

    #[test]
    fn merge_empty_delta_preserves_existing() {
        let mut base = Capabilities {
            supports_step_back: Some(true),
            ..Default::default()
        };

        base.merge(Capabilities::default());

        assert_eq!(
            base.supports_step_back,
            Some(true),
            "an empty capabilities delta must not clear existing values"
        );
    }

    #[test]
    fn merge_replaces_list_valued_capabilities_wholesale() {
        let mut base = Capabilities {
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "old".to_owned(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let delta = Capabilities {
            exception_breakpoint_filters: Some(vec![ExceptionBreakpointsFilter {
                filter: "new".to_owned(),
                ..Default::default()
            }]),
            ..Default::default()
        };

        base.merge(delta);

        let filters = base
            .exception_breakpoint_filters
            .expect("filters present after merge");
        assert_eq!(
            filters
                .iter()
                .map(|f| f.filter.as_str())
                .collect::<Vec<_>>(),
            vec!["new"],
            "a list-valued capability delta should replace, not append"
        );
    }

    #[test]
    fn merge_delta_can_disable_capability_and_override_extra() {
        let mut base = Capabilities {
            supports_step_back: Some(true),
            ..Default::default()
        };
        base.extra.insert("k".to_owned(), Value::from(1));

        let mut delta = Capabilities {
            supports_step_back: Some(false),
            ..Default::default()
        };
        delta.extra.insert("k".to_owned(), Value::from(2));

        base.merge(delta);

        assert_eq!(
            base.supports_step_back,
            Some(false),
            "a delta may turn a capability off"
        );
        assert_eq!(
            base.extra.get("k"),
            Some(&Value::from(2)),
            "a delta extra key should override the baseline extra key of the same name"
        );
    }

    #[test]
    fn apply_capabilities_event_seeds_absent_baseline() {
        let mut baseline: Option<Capabilities> = None;
        apply_capabilities_event(
            &mut baseline,
            Capabilities {
                supports_step_back: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(
            baseline.and_then(|c| c.supports_step_back),
            Some(true),
            "applying an event to an absent baseline should seed it"
        );
    }
}
