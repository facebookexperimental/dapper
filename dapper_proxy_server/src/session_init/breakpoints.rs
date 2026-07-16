// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Breakpoint grouping for DAP setBreakpoints requests.

use std::collections::HashMap;

use dapper_dap_protocol::data_types::ExceptionBreakpointsFilter;
use dapper_session::ExceptionFilterEntry;
use dapper_session::config::BreakpointSpec;

/// Groups breakpoints by type for efficient DAP request generation.
///
/// DAP requires source breakpoints to be grouped by file path (one
/// setBreakpoints request per file), function breakpoints to be sent
/// together in a single setFunctionBreakpoints request, and exception
/// filters to be sent together in a single setExceptionBreakpoints request.
#[derive(Debug, Default)]
pub struct BreakpointGroups {
    /// Source breakpoints grouped by file path -> list of line numbers
    source: HashMap<String, Vec<usize>>,
    /// Function breakpoint names
    function: Vec<String>,
    /// Exception filters (id + optional condition), in config order.
    /// Sorted by filter id at install time by the request builder.
    exceptions: Vec<ExceptionFilterEntry>,
}

impl BreakpointGroups {
    /// Create breakpoint groups from a list of breakpoints. Source-line
    /// breakpoints are bucketed by file path, function-name breakpoints are
    /// collected together, and exception filters are converted to
    /// `ExceptionFilterEntry` via the shared conversion in
    /// `dapper_session`.
    pub fn from_breakpoints(breakpoints: &[BreakpointSpec]) -> Self {
        let mut groups = Self::default();

        for bp in breakpoints {
            match bp {
                BreakpointSpec::Function { name } => {
                    groups.function.push(name.clone());
                }
                BreakpointSpec::Source { path, line } => {
                    groups.source.entry(path.clone()).or_default().push(*line);
                }
                // Destructure directly rather than going through
                // `from_breakpoint_spec(bp)`. The conversion in the shared
                // crate returns `Option` because it accepts any
                // `BreakpointSpec`, but here we've already matched on the
                // `Exception` variant — using the `Option`-returning helper
                // would invite a silent drop if the conversion ever changed
                // shape. The fields are the same on both types.
                BreakpointSpec::Exception { filter, condition } => {
                    groups.exceptions.push(ExceptionFilterEntry {
                        filter: filter.clone(),
                        condition: condition.clone(),
                    });
                }
            }
        }

        groups
    }

    /// Iterate over source breakpoints grouped by path.
    pub fn source_breakpoints(&self) -> impl Iterator<Item = (&str, &[usize])> {
        self.source
            .iter()
            .map(|(path, lines)| (path.as_str(), lines.as_slice()))
    }

    /// Get function breakpoint names.
    pub fn function_breakpoints(&self) -> &[String] {
        &self.function
    }

    /// Get the exception filter entries (in config order; the request
    /// builder sorts them defensively before constructing the DAP request).
    pub fn exception_filters(&self) -> &[ExceptionFilterEntry] {
        &self.exceptions
    }
}

/// Validate requested exception filter ids against the adapter-advertised
/// set. Collects every unknown id (deduped, sorted) before failing so a
/// caller with multiple typos sees the full list at once.
pub(crate) fn validate_exception_filter_ids<'a>(
    advertised: &[ExceptionBreakpointsFilter],
    requested_ids: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let advertised: std::collections::BTreeSet<&str> =
        advertised.iter().map(|f| f.filter.as_str()).collect();
    let unknown: Vec<&str> = requested_ids
        .into_iter()
        .filter(|id| !advertised.contains(id))
        .collect::<std::collections::BTreeSet<&str>>()
        .into_iter()
        .collect();
    if !unknown.is_empty() {
        let valid: Vec<&str> = advertised.into_iter().collect();
        anyhow::bail!("unknown exception breakpoint filter(s) {unknown:?}; valid ids: {valid:?}",);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_breakpoints() {
        let groups = BreakpointGroups::from_breakpoints(&[]);
        assert!(groups.function_breakpoints().is_empty());
        assert!(groups.exception_filters().is_empty());
        assert_eq!(groups.source_breakpoints().count(), 0);
    }

    #[test]
    fn test_function_breakpoints() {
        let breakpoints = vec![
            BreakpointSpec::function("main"),
            BreakpointSpec::function("foo"),
            BreakpointSpec::function("bar"),
        ];
        let groups = BreakpointGroups::from_breakpoints(&breakpoints);

        assert_eq!(groups.function_breakpoints(), &["main", "foo", "bar"]);
        assert_eq!(groups.source_breakpoints().count(), 0);
        assert!(groups.exception_filters().is_empty());
    }

    #[test]
    fn test_source_breakpoints_grouped_by_path() {
        let breakpoints = vec![
            BreakpointSpec::source("a.cpp", 10),
            BreakpointSpec::source("b.cpp", 20),
            BreakpointSpec::source("a.cpp", 15),
        ];
        let groups = BreakpointGroups::from_breakpoints(&breakpoints);

        assert!(groups.function_breakpoints().is_empty());

        let source_bps: HashMap<_, _> = groups
            .source_breakpoints()
            .map(|(path, lines)| (path.to_string(), lines.to_vec()))
            .collect();

        assert_eq!(source_bps.len(), 2);
        assert_eq!(source_bps.get("a.cpp").unwrap(), &[10, 15]);
        assert_eq!(source_bps.get("b.cpp").unwrap(), &[20]);
    }

    #[test]
    fn test_mixed_breakpoints() {
        let breakpoints = vec![
            BreakpointSpec::function("main"),
            BreakpointSpec::source("test.cpp", 42),
            BreakpointSpec::exception("uncaught", None),
        ];
        let groups = BreakpointGroups::from_breakpoints(&breakpoints);

        assert_eq!(groups.function_breakpoints(), &["main"]);
        assert_eq!(groups.source_breakpoints().count(), 1);
        assert_eq!(groups.exception_filters().len(), 1);
        assert_eq!(groups.exception_filters()[0].filter, "uncaught");
    }

    #[test]
    fn test_exception_breakpoints_routed_to_dedicated_bucket() {
        let breakpoints = vec![
            BreakpointSpec::exception("uncaught", None),
            BreakpointSpec::exception("raised", Some("x>5".to_string())),
        ];
        let groups = BreakpointGroups::from_breakpoints(&breakpoints);

        assert!(groups.function_breakpoints().is_empty());
        assert_eq!(groups.source_breakpoints().count(), 0);
        assert_eq!(
            groups.exception_filters(),
            &[
                ExceptionFilterEntry {
                    filter: "uncaught".to_string(),
                    condition: None,
                },
                ExceptionFilterEntry {
                    filter: "raised".to_string(),
                    condition: Some("x>5".to_string()),
                },
            ]
        );
    }
}
