// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use serde::Deserialize;
use serde::Serialize;

use crate::config::BreakpointSpec;

/// A single exception breakpoint filter, paired with an optional condition.
///
/// Mirrors the shape of the DAP `setExceptionBreakpoints` request: when
/// `condition` is `None` the filter belongs in the request's `filters` array,
/// and when `Some` it belongs in `filterOptions`. The actual partition (and
/// adapter-capability gating that may drop unsupported conditions) is
/// performed by the request builder added in a subsequent PR.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionFilterEntry {
    pub filter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl ExceptionFilterEntry {
    /// Convert a config-level `BreakpointSpec` into a control-plane
    /// `ExceptionFilterEntry`. Returns `Some` only for the `Exception`
    /// variant; `Source`/`Function` return `None` since they aren't
    /// exception filters. This is the conversion boundary between the two
    /// crates and keeps the field shapes tied together at compile time.
    pub fn from_breakpoint_spec(spec: &BreakpointSpec) -> Option<Self> {
        match spec {
            BreakpointSpec::Exception { filter, condition } => Some(Self {
                filter: filter.clone(),
                condition: condition.clone(),
            }),
            BreakpointSpec::Function { .. } | BreakpointSpec::Source { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_roundtrip_with_condition() {
        let entry = ExceptionFilterEntry {
            filter: "raised".to_string(),
            condition: Some("isinstance(e, ValueError)".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ExceptionFilterEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, parsed);
    }

    #[test]
    fn test_serde_omits_condition_when_none() {
        let entry = ExceptionFilterEntry {
            filter: "uncaught".to_string(),
            condition: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(json, r#"{"filter":"uncaught"}"#);
    }

    #[test]
    fn test_serde_accepts_missing_condition() {
        let parsed: ExceptionFilterEntry =
            serde_json::from_str(r#"{"filter":"uncaught"}"#).unwrap();
        assert_eq!(parsed.filter, "uncaught");
        assert_eq!(parsed.condition, None);
    }

    #[test]
    fn test_from_breakpoint_spec_exception_returns_some() {
        let spec = BreakpointSpec::exception("raised", Some("x>5".to_string()));
        let entry = ExceptionFilterEntry::from_breakpoint_spec(&spec);
        assert_eq!(
            entry,
            Some(ExceptionFilterEntry {
                filter: "raised".to_string(),
                condition: Some("x>5".to_string()),
            })
        );
    }

    #[test]
    fn test_from_breakpoint_spec_non_exception_returns_none() {
        assert_eq!(
            ExceptionFilterEntry::from_breakpoint_spec(&BreakpointSpec::function("main")),
            None
        );
        assert_eq!(
            ExceptionFilterEntry::from_breakpoint_spec(&BreakpointSpec::source("a.cpp", 10)),
            None
        );
    }
}
