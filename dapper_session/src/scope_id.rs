// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeId(String);

impl ScopeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ScopeId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for ScopeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ScopeId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl FromStr for ScopeId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
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
