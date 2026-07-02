// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;
use std::num::NonZeroU16;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

/// A non-zero TCP port. Port 0 represents an unbound port and is
/// unrepresentable — including through `Deserialize`, so a session file
/// cannot smuggle one in.
#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub struct Port(NonZeroU16);

impl Port {
    /// Creates a new Port from a u16.
    /// Returns None if the port is 0, as port 0 represents an unbound port.
    pub fn try_new(port: u16) -> Option<Self> {
        NonZeroU16::new(port).map(Port)
    }

    /// Returns the inner u16 value
    pub fn get(&self) -> u16 {
        self.0.get()
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParsePortError {
    #[error("invalid port number: {0}")]
    Invalid(#[from] std::num::ParseIntError),
    #[error("port must be non-zero")]
    Zero,
}

impl FromStr for Port {
    type Err = ParsePortError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let port: u16 = s.parse()?;
        Port::try_new(port).ok_or(ParsePortError::Zero)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_try_new_with_valid_port() {
        let port = Port::try_new(8080);
        assert!(port.is_some());
        assert_eq!(port.unwrap().get(), 8080);
    }

    #[test]
    fn test_port_try_new_with_zero_returns_none() {
        let port = Port::try_new(0);
        assert!(port.is_none());
    }

    #[test]
    fn test_port_deserialize_rejects_zero() {
        let result: Result<Port, _> = serde_json::from_str("0");
        assert!(result.is_err(), "port 0 must not deserialize");
        let port: Port = serde_json::from_str("8080").expect("valid port");
        assert_eq!(port.get(), 8080);
    }

    #[test]
    fn test_port_from_str() {
        assert_eq!("8080".parse::<Port>().unwrap().get(), 8080);
        assert!(matches!("0".parse::<Port>(), Err(ParsePortError::Zero)));
        assert!(matches!(
            "notaport".parse::<Port>(),
            Err(ParsePortError::Invalid(_))
        ));
    }
}
