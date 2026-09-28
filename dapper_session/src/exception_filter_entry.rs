// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use serde::Deserialize;
use serde::Serialize;

/// A single exception breakpoint filter, paired with an optional condition.
///
/// Mirrors the shape of the DAP `setExceptionBreakpoints` request: when
/// `condition` is `None` the filter belongs in the request's `filters` array,
/// and when `Some` it belongs in `filterOptions`. The request builder sends it
/// there only if the adapter advertises both `supportsExceptionFilterOptions`
/// and the filter's `supportsCondition`; otherwise it drops the condition with
/// a warning.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionFilterEntry {
    pub filter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
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
}
