// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

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
}
