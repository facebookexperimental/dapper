// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;
use std::str::FromStr;

use serde::Serialize;
use serde_with::DeserializeFromStr;
use subtle::ConstantTimeEq;

/// A secret that lets a control-plane call skip the access policy. A headless
/// proxy with a policy makes one at startup and reports it only to its
/// launcher, in `DAPPER_READY`. It has no `Display`, and `Debug` hides it, so
/// it stays out of logs unless [`OverrideToken::expose`] is called.
#[derive(Clone, Serialize, DeserializeFromStr)]
#[serde(transparent)]
pub struct OverrideToken(String);

impl OverrideToken {
    /// The environment variable that carries a session's token: from a
    /// launcher to `dapper debug`, and from a parent session to the child
    /// sessions it spawns. Never a command-line argument, which every user on
    /// the host can read while the command runs.
    pub const ENV_VAR: &'static str = "DAPPER_OVERRIDE_TOKEN";

    /// The token in [`OverrideToken::ENV_VAR`], if it is set.
    pub fn from_env() -> Result<Option<Self>, InvalidOverrideToken> {
        let Some(value) = std::env::var_os(Self::ENV_VAR) else {
            return Ok(None);
        };
        value
            .to_str()
            .ok_or(InvalidOverrideToken)?
            .parse()
            .map(Some)
    }

    /// A new token of 256 random bits.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)?;
        Ok(Self(hex::encode(bytes)))
    }

    /// The token itself, to send with a call.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether `presented` is this token, compared in constant time.
    pub fn matches(&self, presented: &str) -> bool {
        self.0.as_bytes().ct_eq(presented.as_bytes()).into()
    }
}

/// Compares in constant time, like [`OverrideToken::matches`]; a derived
/// comparison would stop at the first byte that differs.
impl PartialEq for OverrideToken {
    fn eq(&self, other: &Self) -> bool {
        self.matches(other.expose())
    }
}

impl Eq for OverrideToken {}

impl fmt::Debug for OverrideToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OverrideToken(..)")
    }
}

/// Any non-empty string of visible ASCII characters, which a call can carry
/// in its metadata. A proxy makes 64 hex digits, but a token is not
/// otherwise checked here, so a client keeps reading a newer proxy's tokens.
impl FromStr for OverrideToken {
    type Err = InvalidOverrideToken;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(InvalidOverrideToken);
        }
        Ok(Self(token.to_owned()))
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("an override token is a non-empty string of visible ASCII characters")]
pub struct InvalidOverrideToken;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_long_random_and_readable_back() {
        let token = OverrideToken::generate().expect("the OS should provide randomness");
        let other = OverrideToken::generate().expect("the OS should provide randomness");
        assert_eq!(token.expose().len(), 64, "256 bits as hex digits");
        assert_ne!(token, other, "two tokens should not collide");
        assert_eq!(token.expose().parse(), Ok(token));
    }

    #[test]
    fn a_token_matches_only_itself() {
        let token: OverrideToken = "d34db33f".parse().expect("hex digits are a valid token");
        assert!(token.matches("d34db33f"));
        for presented in ["d34db33e", "d34db33", "d34db33ff", ""] {
            assert!(!token.matches(presented), "`{presented}` should not match");
        }
        assert_eq!(
            token,
            "d34db33f".parse().expect("hex digits are a valid token")
        );
        assert_ne!(
            token,
            "d34db33e".parse().expect("hex digits are a valid token")
        );
    }

    #[test]
    fn a_token_stays_out_of_debug_output() {
        let token: OverrideToken = "d34db33f".parse().expect("hex digits are a valid token");
        assert_eq!(format!("{token:?}"), "OverrideToken(..)");
        assert_eq!(format!("{:?}", Some(&token)), "Some(OverrideToken(..))");
    }

    #[test]
    fn a_token_crosses_serde_as_itself() {
        let token: OverrideToken = "d34db33f".parse().expect("hex digits are a valid token");
        assert_eq!(
            serde_json::to_value(&token).expect("a token should serialize"),
            serde_json::json!("d34db33f")
        );
        assert_eq!(
            serde_json::from_value::<OverrideToken>(serde_json::json!("d34db33f"))
                .expect("a token should parse"),
            token
        );
        assert!(serde_json::from_value::<OverrideToken>(serde_json::json!("")).is_err());
    }

    #[test]
    fn a_token_comes_from_the_environment_when_set() {
        temp_env::with_var(OverrideToken::ENV_VAR, Some("d34db33f"), || {
            assert_eq!(
                OverrideToken::from_env(),
                Ok(Some(
                    "d34db33f"
                        .parse::<OverrideToken>()
                        .expect("hex digits are a valid token")
                ))
            );
        });
        temp_env::with_var_unset(OverrideToken::ENV_VAR, || {
            assert_eq!(OverrideToken::from_env(), Ok(None));
        });
        temp_env::with_var(OverrideToken::ENV_VAR, Some(""), || {
            assert_eq!(
                OverrideToken::from_env(),
                Err(InvalidOverrideToken),
                "a variable set to nothing is not a token"
            );
        });
    }

    #[test]
    fn a_token_is_visible_ascii() {
        for invalid in ["", "has space", "tab\t", "caf\u{e9}"] {
            assert_eq!(
                invalid.parse::<OverrideToken>(),
                Err(InvalidOverrideToken),
                "`{invalid}` cannot travel in call metadata"
            );
        }
    }
}
