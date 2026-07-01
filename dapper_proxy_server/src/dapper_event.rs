// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Dapper events sent from the control plane to the proxy server.

use dapper_dap_protocol::events::EventKind;
use dapper_dap_protocol::events::UnknownEvent;
use dapper_session::Port;
use dapper_session::SessionId;
use serde::Deserialize;
use serde::Serialize;

/// Events sent via the "dapper" DAP event type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "category")]
pub enum DapperEvent {
    #[serde(rename = "controlPlaneStatus")]
    ControlPlaneStatus(ControlPlaneStatus),

    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlPlaneStatus {
    #[serde(rename = "sessionId")]
    pub session_id: SessionId,

    pub success: bool,

    /// The port the control plane is listening on (only present on success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<Port>,

    /// Error message (only present on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ControlPlaneStatus {
    pub fn success(session_id: SessionId, port: Port) -> Self {
        Self {
            session_id,
            success: true,
            port: Some(port),
            message: None,
        }
    }

    pub fn failure(session_id: SessionId, message: String) -> Self {
        Self {
            session_id,
            success: false,
            port: None,
            message: Some(message),
        }
    }
}

impl TryFrom<DapperEvent> for EventKind {
    type Error = serde_json::Error;

    fn try_from(event: DapperEvent) -> Result<EventKind, Self::Error> {
        let body = serde_json::to_value(&event)?;
        Ok(EventKind::Unknown(UnknownEvent {
            event: "dapper".to_string(),
            body: Some(body),
            extra: Default::default(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_success() {
        let json = serde_json::json!({
            "category": "controlPlaneStatus",
            "sessionId": "session-123",
            "success": true,
            "port": 8080
        });

        let event: DapperEvent = serde_json::from_value(json.clone()).unwrap();
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_roundtrip_failure() {
        let json = serde_json::json!({
            "category": "controlPlaneStatus",
            "sessionId": "session-456",
            "success": false,
            "message": "bind failed"
        });

        let event: DapperEvent = serde_json::from_value(json.clone()).unwrap();
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_deserialize_unknown_category() {
        let json = serde_json::json!({
            "category": "someFutureEvent",
            "data": "whatever"
        });

        let event: DapperEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(event, DapperEvent::Unknown));
    }
}
