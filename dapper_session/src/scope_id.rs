// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    derive_more::From,
    derive_more::FromStr
)]
#[serde(transparent)]
#[from(String, &str)]
pub struct ScopeId(String);

impl ScopeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `" in scope '<id>'"` fragment session-facing messages append when a
    /// scope filter is active, empty otherwise.
    pub fn clause(scope_id: Option<&ScopeId>) -> ScopeClause<'_> {
        ScopeClause(scope_id)
    }
}

/// Renders [`ScopeId::clause`]. The leading space belongs to the clause so
/// callers can interpolate it straight after a sentence fragment without
/// having to vary their own spacing on whether a scope is set.
pub struct ScopeClause<'a>(Option<&'a ScopeId>);

impl fmt::Display for ScopeClause<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(scope_id) => write!(f, " in scope '{}'", scope_id),
            None => Ok(()),
        }
    }
}

impl AsRef<str> for ScopeId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display() {
        let id = ScopeId::new("my-scope");
        assert_eq!(id.to_string(), "my-scope");
    }

    #[test]
    fn test_serde_roundtrip() {
        let id = ScopeId::new("my-scope");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"my-scope\"");
        let deserialized: ScopeId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn clause_renders_with_leading_space() {
        let id = ScopeId::new("my-scope");
        assert_eq!(
            format!("No sessions found{}.", ScopeId::clause(Some(&id))),
            "No sessions found in scope 'my-scope'."
        );
    }

    #[test]
    fn clause_is_empty_without_a_scope() {
        assert_eq!(
            format!("No sessions found{}.", ScopeId::clause(None)),
            "No sessions found."
        );
    }
}
