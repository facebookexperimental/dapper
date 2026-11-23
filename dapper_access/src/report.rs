// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;

use enumset::EnumSet;
use serde::Deserialize;
use serde::Serialize;

use crate::Operation;
use crate::Policy;

/// What a proxy reports about the access policy it enforces. Clients of every
/// version read it, so an operation a client does not know is kept as a name
/// instead of failing the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyReport {
    /// The operations the policy allows.
    pub allow: Vec<ReportedOperation>,
}

/// An operation in a [`PolicyReport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, derive_more::Display)]
#[serde(untagged)]
pub enum ReportedOperation {
    #[display("{_0}")]
    Known(Operation),
    /// An operation of a newer dapper.
    #[display("{_0}")]
    Unknown(String),
}

impl From<Policy> for PolicyReport {
    fn from(policy: Policy) -> Self {
        Self {
            allow: policy
                .allowed()
                .iter()
                .map(ReportedOperation::Known)
                .collect(),
        }
    }
}

impl PolicyReport {
    /// The allowed operations this dapper knows.
    pub fn known_operations(&self) -> EnumSet<Operation> {
        self.allow
            .iter()
            .filter_map(|operation| match operation {
                ReportedOperation::Known(operation) => Some(*operation),
                ReportedOperation::Unknown(_) => None,
            })
            .collect()
    }
}

/// The allowed operations as a comma-separated list, or "nothing".
impl fmt::Display for PolicyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.allow.is_empty() {
            return f.write_str("nothing");
        }
        let names: Vec<String> = self.allow.iter().map(ToString::to_string).collect();
        f.write_str(&names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Preset;

    fn minimal() -> Policy {
        Policy::try_new(Preset::Minimal.operations()).expect("a preset is a whole policy")
    }

    #[test]
    fn a_report_lists_the_allowed_operations() {
        let report = PolicyReport::from(minimal());
        assert_eq!(
            serde_json::to_value(&report).expect("a report should serialize"),
            json!({ "allow": ["inspect", "read_values"] })
        );
        assert_eq!(report.to_string(), "inspect, read_values");
        assert_eq!(report.known_operations(), Preset::Minimal.operations());
    }

    #[test]
    fn a_report_keeps_operations_this_dapper_does_not_know() {
        let report: PolicyReport =
            serde_json::from_value(json!({ "allow": ["inspect", "rewrite_history"] }))
                .expect("a newer proxy's report should still parse");
        assert_eq!(
            report.allow,
            [
                ReportedOperation::Known(Operation::Inspect),
                ReportedOperation::Unknown("rewrite_history".to_owned())
            ]
        );
        assert_eq!(report.known_operations(), EnumSet::only(Operation::Inspect));
        assert_eq!(report.to_string(), "inspect, rewrite_history");
    }

    #[test]
    fn a_policy_that_allows_nothing_reports_nothing() {
        let report = PolicyReport::from(Policy::try_new(EnumSet::empty()).expect("whole"));
        assert_eq!(report.allow, []);
        assert_eq!(report.to_string(), "nothing");
    }
}
