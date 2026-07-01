// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub struct Port(u16);

impl Port {
    /// Creates a new Port from a u16.
    /// Returns None if the port is 0, as port 0 represents an unbound port.
    pub fn new(port: u16) -> Option<Self> {
        if port == 0 { None } else { Some(Port(port)) }
    }

    /// Returns the inner u16 value
    pub fn get(&self) -> u16 {
        self.0
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_new_with_valid_port() {
        let port = Port::new(8080);
        assert!(port.is_some());
        assert_eq!(port.unwrap().get(), 8080);
    }

    #[test]
    fn test_port_new_with_zero_returns_none() {
        let port = Port::new(0);
        assert!(port.is_none());
    }
}
