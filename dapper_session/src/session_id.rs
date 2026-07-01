// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;
use std::str::FromStr;

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
    schemars::JsonSchema
)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl FromStr for SessionId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_produces_unique_ids() {
        let id1 = SessionId::generate();
        let id2 = SessionId::generate();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_display() {
        let id: SessionId = "test-session".into();
        assert_eq!(id.to_string(), "test-session");
    }

    #[test]
    fn test_from_str_roundtrip() {
        // The `--parent-session-id` CLI flag parses a `SessionId` via `FromStr`;
        // lock in the `FromStr`/`Display` symmetry it relies on.
        let id: SessionId = "abc".parse().unwrap();
        assert_eq!(id, SessionId::from("abc"));
        // Display -> parse round-trips back to the same id.
        let reparsed: SessionId = id.to_string().parse().unwrap();
        assert_eq!(reparsed, id);
    }

    #[test]
    fn test_serde_roundtrip() {
        let id: SessionId = "test-session".into();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"test-session\"");
        let deserialized: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn test_json_schema_matches_string() {
        let session_id_schema = schemars::schema_for!(SessionId);
        let string_schema = schemars::schema_for!(String);
        assert_eq!(
            serde_json::to_value(&session_id_schema).unwrap(),
            serde_json::to_value(&string_schema).unwrap(),
        );
    }
}
