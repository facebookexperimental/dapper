// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Dapper events sent from the control plane to the proxy server.

use anyhow::Context;
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

impl TryFrom<&EventKind> for DapperEvent {
    type Error = anyhow::Error;

    /// Recognize a dapper event carried in an incoming DAP event. Fails when the
    /// event is not the custom `"dapper"` event, has no body, or its body does
    /// not parse as a known `DapperEvent`. Used by a proxy to detect that its
    /// backend is itself a dapper proxy.
    fn try_from(kind: &EventKind) -> anyhow::Result<Self> {
        let EventKind::Unknown(unknown) = kind else {
            anyhow::bail!("not a custom (unknown) DAP event");
        };
        anyhow::ensure!(unknown.event == "dapper", "not a dapper event");
        let body = unknown.body.as_ref().context("dapper event has no body")?;
        Ok(serde_json::from_value(body.clone())?)
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

    #[test]
    fn try_from_event_kind_recognizes_control_plane_status_success() {
        let kind = EventKind::Unknown(UnknownEvent {
            event: "dapper".to_string(),
            body: Some(serde_json::json!({
                "category": "controlPlaneStatus",
                "sessionId": "s-1",
                "success": true,
                "port": 8080
            })),
            extra: Default::default(),
        });

        let parsed = DapperEvent::try_from(&kind).expect("should recognize dapper event");
        match parsed {
            DapperEvent::ControlPlaneStatus(status) => {
                assert!(status.success);
                assert_eq!(status.session_id.as_str(), "s-1");
            }
            other => panic!("expected ControlPlaneStatus, got {other:?}"),
        }
    }

    #[test]
    fn try_from_event_kind_rejects_non_dapper_events() {
        let kind = EventKind::Unknown(UnknownEvent {
            event: "output".to_string(),
            body: Some(serde_json::json!({ "category": "stdout" })),
            extra: Default::default(),
        });
        assert!(DapperEvent::try_from(&kind).is_err());
    }

    #[test]
    fn try_from_event_kind_rejects_dapper_event_without_body() {
        let kind = EventKind::Unknown(UnknownEvent {
            event: "dapper".to_string(),
            body: None,
            extra: Default::default(),
        });
        assert!(DapperEvent::try_from(&kind).is_err());
    }
}
