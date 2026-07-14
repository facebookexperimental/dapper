// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! DAP request builders for session initialization.

use anyhow::Context;
use dapper_dap_protocol::capabilities::Capabilities;
use dapper_dap_protocol::data_types::ExceptionFilterOptions;
use dapper_dap_protocol::data_types::FunctionBreakpoint;
use dapper_dap_protocol::data_types::Source as DapSource;
use dapper_dap_protocol::data_types::SourceBreakpoint;
use dapper_dap_protocol::enums::PathFormat;
use dapper_dap_protocol::protocol::Request;
use dapper_dap_protocol::requests::ConfigurationDoneArguments;
use dapper_dap_protocol::requests::InitializeRequestArguments;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::requests::SetBreakpointsArguments;
use dapper_dap_protocol::requests::SetExceptionBreakpointsArguments;
use dapper_dap_protocol::requests::SetFunctionBreakpointsArguments;
use dapper_session::ExceptionFilterEntry;
use dapper_session::config::DebugRequest;
use tracing::warn;

pub fn initialize(
    overrides: Option<&serde_json::Value>,
    supports_start_debugging: bool,
) -> anyhow::Result<Request> {
    let mut args: InitializeRequestArguments = if let Some(overrides) = overrides {
        serde_json::from_value(overrides.clone())
            .context("Failed to parse initialize args override")?
    } else {
        InitializeRequestArguments {
            client_id: Some("dapper-headless".to_owned()),
            client_name: Some("Dapper Headless".to_owned()),
            adapter_id: "dapper".to_owned(),
            path_format: Some(PathFormat::Path),
            lines_start_at1: Some(true),
            columns_start_at1: Some(true),
            supports_variable_type: Some(true),
            supports_variable_paging: Some(true),
            supports_run_in_terminal_request: Some(false),
            locale: Some("en-us".to_owned()),
            ..Default::default()
        }
    };

    // dapper owns this capability: the gate is authoritative regardless of any
    // `initialize_args` override. We force the field (rather than only setting it
    // when gated on) so it stays fail-closed both ways — a user-supplied
    // `supportsStartDebuggingRequest: true` can't make us advertise support we
    // can't honor, and a full override can't silently drop it when enabled.
    args.supports_start_debugging_request = supports_start_debugging.then_some(true);

    Ok(Request::new(RequestCommand::Initialize(args)))
}

pub fn debug_request(request: &DebugRequest) -> Request {
    Request::new(request.clone().into())
}

pub fn configuration_done() -> Request {
    Request::new(RequestCommand::ConfigurationDone(Some(
        ConfigurationDoneArguments {
            ..Default::default()
        },
    )))
}

pub fn set_breakpoints(path: &str, lines: &[usize]) -> Request {
    let breakpoints: Vec<SourceBreakpoint> = lines
        .iter()
        .map(|&line| SourceBreakpoint {
            line: line as i64,
            ..Default::default()
        })
        .collect();

    Request::new(RequestCommand::SetBreakpoints(SetBreakpointsArguments {
        source: DapSource {
            path: Some(path.to_string()),
            ..Default::default()
        },
        breakpoints: Some(breakpoints),
        ..Default::default()
    }))
}

/// Build a `setExceptionBreakpoints` DAP request from a list of desired
/// exception filter entries, partitioning into plain `filters` vs
/// `filterOptions` based on adapter capabilities.
///
/// Sorts the input defensively by filter id so the resulting request and
/// the returned `effective` vec are deterministic regardless of caller
/// order. Conditions are only emitted via `filterOptions` when the adapter
/// advertises `supports_exception_filter_options == Some(true)` AND the
/// matching filter advertises `supports_condition == Some(true)`. When
/// either gate fails, the entry falls back to a plain filter and the
/// condition is dropped with a `warn!`.
///
/// Returns `(request, effective)`:
/// - `request` is the constructed DAP request.
/// - `effective` reflects the sanitized set actually carried by `request` —
///   same `filter` ids in the same (sorted) order, with conditions only
///   present for entries that survived capability gating. Tracker state
///   should always be updated from `effective`, never the input.
pub(crate) fn build_set_exception_breakpoints_request(
    entries: &[ExceptionFilterEntry],
    caps: Option<&Capabilities>,
) -> (Request, Vec<ExceptionFilterEntry>) {
    let supports_filter_options = caps
        .and_then(|c| c.supports_exception_filter_options)
        .unwrap_or(false);
    let advertised = caps.and_then(|c| c.exception_breakpoint_filters.as_ref());

    let mut sorted: Vec<&ExceptionFilterEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.filter.cmp(&b.filter));

    let mut filters: Vec<String> = Vec::with_capacity(sorted.len());
    let mut filter_options: Vec<ExceptionFilterOptions> = Vec::with_capacity(sorted.len());
    let mut effective: Vec<ExceptionFilterEntry> = Vec::with_capacity(sorted.len());

    for entry in sorted {
        let supports_cond_for_filter = advertised
            .map(|adv| {
                adv.iter()
                    .any(|f| f.filter == entry.filter && f.supports_condition == Some(true))
            })
            .unwrap_or(false);

        if let Some(cond) = entry.condition.as_deref() {
            if supports_filter_options && supports_cond_for_filter {
                filter_options.push(ExceptionFilterOptions {
                    filter_id: entry.filter.clone(),
                    condition: Some(cond.to_string()),
                    mode: None,
                });
                effective.push(entry.clone());
                continue;
            }
            warn!(
                filter = %entry.filter,
                supports_filter_options = supports_filter_options,
                supports_condition_for_filter = supports_cond_for_filter,
                "adapter does not support condition for exception filter; dropping condition and sending as plain filter"
            );
            // Fallthrough: send as a plain filter with the condition dropped.
        }

        filters.push(entry.filter.clone());
        effective.push(ExceptionFilterEntry {
            filter: entry.filter.clone(),
            condition: None,
        });
    }

    let request = Request::new(RequestCommand::SetExceptionBreakpoints(
        SetExceptionBreakpointsArguments {
            filters,
            filter_options: if filter_options.is_empty() {
                None
            } else {
                Some(filter_options)
            },
            ..Default::default()
        },
    ));

    (request, effective)
}

pub fn set_function_breakpoints(names: &[String]) -> Request {
    let breakpoints: Vec<FunctionBreakpoint> = names
        .iter()
        .map(|name| FunctionBreakpoint {
            name: name.clone(),
            ..Default::default()
        })
        .collect();

    Request::new(RequestCommand::SetFunctionBreakpoints(
        SetFunctionBreakpointsArguments {
            breakpoints,
            ..Default::default()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_defaults() {
        let req = initialize(None, false).unwrap();
        match req.command {
            RequestCommand::Initialize(args) => {
                assert_eq!(args.adapter_id, "dapper");
                assert_eq!(args.supports_variable_type, Some(true));
                assert_eq!(args.supports_start_debugging_request, None);
            }
            _ => panic!("Expected Initialize command"),
        }
    }

    #[test]
    fn test_initialize_with_overrides_replaces_entirely() {
        let overrides = serde_json::json!({
            "adapterID": "cppdbg",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path"
        });
        let req = initialize(Some(&overrides), false).unwrap();
        match req.command {
            RequestCommand::Initialize(args) => {
                assert_eq!(args.adapter_id, "cppdbg");
                assert_eq!(args.supports_variable_type, None);
                assert_eq!(args.client_id, None);
            }
            _ => panic!("Expected Initialize command"),
        }
    }

    #[test]
    fn test_start_debugging_capability_is_gate_authoritative() {
        // The gate — not the override — decides whether the capability is
        // advertised. An override that sets it true must NOT survive when the
        // gate is off (fail-closed), and a full override can't drop it when on.
        let override_on = serde_json::json!({
            "adapterID": "dapper",
            "supportsStartDebuggingRequest": true
        });

        // Gate off + override true -> forced off (None).
        let req = initialize(Some(&override_on), false).unwrap();
        match req.command {
            RequestCommand::Initialize(args) => assert_eq!(
                args.supports_start_debugging_request, None,
                "an override must not advertise the capability when the gate is off"
            ),
            _ => panic!("Expected Initialize command"),
        }

        // Gate on + override true -> on.
        let req = initialize(Some(&override_on), true).unwrap();
        match req.command {
            RequestCommand::Initialize(args) => {
                assert_eq!(args.supports_start_debugging_request, Some(true))
            }
            _ => panic!("Expected Initialize command"),
        }

        // Gate on, no override -> on (a full override branch can't drop it).
        let req = initialize(None, true).unwrap();
        match req.command {
            RequestCommand::Initialize(args) => {
                assert_eq!(args.supports_start_debugging_request, Some(true))
            }
            _ => panic!("Expected Initialize command"),
        }
    }

    use dapper_dap_protocol::data_types::ExceptionBreakpointsFilter;

    fn caps_with_filters(
        supports_filter_options: bool,
        filters: Vec<ExceptionBreakpointsFilter>,
    ) -> Capabilities {
        Capabilities {
            supports_exception_filter_options: Some(supports_filter_options),
            exception_breakpoint_filters: Some(filters),
            ..Default::default()
        }
    }

    fn filter_def(name: &str, supports_condition: bool) -> ExceptionBreakpointsFilter {
        ExceptionBreakpointsFilter {
            filter: name.to_string(),
            label: name.to_string(),
            supports_condition: Some(supports_condition),
            ..Default::default()
        }
    }

    fn unwrap_set_exception_args(req: &Request) -> &SetExceptionBreakpointsArguments {
        match &req.command {
            RequestCommand::SetExceptionBreakpoints(args) => args,
            other => panic!("expected SetExceptionBreakpoints, got {other:?}"),
        }
    }

    #[test]
    fn test_build_set_exception_breakpoints_bare_filters() {
        let entries = vec![
            ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            },
            ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: None,
            },
        ];
        let (req, effective) = build_set_exception_breakpoints_request(&entries, None);
        let args = unwrap_set_exception_args(&req);
        assert_eq!(args.filters, vec!["raised", "uncaught"]);
        assert!(args.filter_options.is_none());
        assert_eq!(
            effective,
            vec![
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: None,
                },
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
            ]
        );
    }

    #[test]
    fn test_build_set_exception_breakpoints_condition_with_caps_support() {
        let caps = caps_with_filters(true, vec![filter_def("raised", true)]);
        let entries = vec![ExceptionFilterEntry {
            filter: "raised".to_string(),
            condition: Some("x>5".to_string()),
        }];
        let (req, effective) = build_set_exception_breakpoints_request(&entries, Some(&caps));
        let args = unwrap_set_exception_args(&req);
        assert!(args.filters.is_empty());
        let opts = args.filter_options.as_ref().unwrap();
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].filter_id, "raised");
        assert_eq!(opts[0].condition.as_deref(), Some("x>5"));
        assert_eq!(effective, entries);
    }

    #[test]
    fn test_build_set_exception_breakpoints_condition_dropped_when_caps_lack_support() {
        // Adapter advertises the filter but neither overall filterOptions nor the
        // per-filter supports_condition are set — condition must be dropped.
        let caps = caps_with_filters(false, vec![filter_def("raised", false)]);
        let entries = vec![ExceptionFilterEntry {
            filter: "raised".to_string(),
            condition: Some("x>5".to_string()),
        }];
        let (req, effective) = build_set_exception_breakpoints_request(&entries, Some(&caps));
        let args = unwrap_set_exception_args(&req);
        assert_eq!(args.filters, vec!["raised"]);
        assert!(args.filter_options.is_none());
        assert_eq!(
            effective,
            vec![ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: None,
            }]
        );
    }

    #[test]
    fn test_build_set_exception_breakpoints_mixed() {
        // raised has a condition + caps support; uncaught is plain.
        let caps = caps_with_filters(
            true,
            vec![filter_def("raised", true), filter_def("uncaught", false)],
        );
        let entries = vec![
            ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            },
            ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: Some("x>5".to_string()),
            },
        ];
        let (req, _) = build_set_exception_breakpoints_request(&entries, Some(&caps));
        let args = unwrap_set_exception_args(&req);
        assert_eq!(args.filters, vec!["uncaught"]);
        let opts = args.filter_options.as_ref().unwrap();
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].filter_id, "raised");
    }

    #[test]
    fn test_build_set_exception_breakpoints_empty() {
        let (req, effective) = build_set_exception_breakpoints_request(&[], None);
        let args = unwrap_set_exception_args(&req);
        assert!(args.filters.is_empty());
        assert!(args.filter_options.is_none());
        assert!(effective.is_empty());
    }

    #[test]
    fn test_build_set_exception_breakpoints_sorts_input_defensively() {
        // Pass unsorted input; assert output is sorted by filter id.
        let entries = vec![
            ExceptionFilterEntry {
                filter: "uncaught".to_string(),
                condition: None,
            },
            ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: None,
            },
        ];
        let (req, effective) = build_set_exception_breakpoints_request(&entries, None);
        let args = unwrap_set_exception_args(&req);
        assert_eq!(args.filters, vec!["raised", "uncaught"]);
        let effective_ids: Vec<&str> = effective.iter().map(|e| e.filter.as_str()).collect();
        assert_eq!(effective_ids, vec!["raised", "uncaught"]);
    }
}
