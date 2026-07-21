// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::io::Write;
use std::str::Utf8Error;

use derive_more::From;
use derive_more::TryInto;
use indexmap::IndexMap;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use thiserror::Error;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;

use crate::data_types::Seq;
use crate::events::EventKind;
use crate::requests::RequestCommand;
use crate::responses::ResponseBody;

/// Maximum allowed size for a single DAP message body (10 MB).
/// Prevents unbounded memory allocation from a malicious or malformed Content-Length header.
const MAX_DAP_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Maximum length of a single DAP header line (8 KB).
/// Prevents a peer from streaming bytes without a `\n` terminator and exhausting memory.
/// Matches typical HTTP server defaults (e.g. nginx `large_client_header_buffers 4 8k`).
const MAX_DAP_HEADER_LINE_SIZE: usize = 8 * 1024;

/// Maximum number of header lines accepted before the blank-line terminator.
/// Prevents a peer from streaming an unbounded sequence of header lines.
/// DAP only defines `Content-Length` and `Content-Type`; 32 leaves ample room for
/// non-standard adapter extensions while still being a tight upper bound.
const MAX_DAP_HEADER_COUNT: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    #[serde(default)]
    pub seq: Seq,
    #[serde(flatten)]
    pub command: RequestCommand,
}

impl Request {
    pub fn new(command: RequestCommand) -> Self {
        Self {
            seq: Seq::default(),
            command,
        }
    }

    pub fn command_name(&self) -> &str {
        self.command.command_name()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    #[serde(default)]
    pub seq: Seq,
    #[serde(rename = "request_seq")]
    pub request_seq: Seq,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub body: ResponseBody,
}

impl Response {
    pub fn command_name(&self) -> &str {
        self.body.command_name()
    }

    pub fn check_success(&self) -> anyhow::Result<()> {
        if !self.success {
            let command = self.command_name();
            anyhow::bail!("{} request failed: {}", command, self.error_message());
        }
        Ok(())
    }

    /// Human-readable error for a failed response.
    ///
    /// Prefers the `ErrorResponse` body's `format` text (the spec's display
    /// error), then the short top-level `message` — adapters differ in which
    /// one they populate.
    pub fn error_message(&self) -> String {
        self.dap_error_message()
            .or_else(|| self.message.clone())
            .unwrap_or_else(|| "unknown error".to_owned())
    }

    /// Extract and render `body.error` (a DAP `Message` object) if present.
    ///
    /// Error bodies don't match the typed [`ResponseBody`] variant for their
    /// command, so they normally land in `ResponseBody::Unknown`; bodies whose
    /// fields are all optional can instead capture the error in their
    /// flattened extras. Re-serializing the body handles both uniformly.
    fn dap_error_message(&self) -> Option<String> {
        let body = serde_json::to_value(&self.body).ok()?;
        let error = body.get("body")?.get("error")?;
        let format = error.get("format")?.as_str()?;
        let variables = error.get("variables").and_then(Value::as_object);
        Some(substitute_placeholders(format, variables))
    }
}

/// Resolve `{name}` placeholders in a DAP `Message.format` string from its
/// `variables` map (all values are strings, per the spec). Substituted values
/// are never re-scanned for placeholders; unknown and unterminated
/// placeholders stay literal.
fn substitute_placeholders(format: &str, variables: Option<&Map<String, Value>>) -> String {
    let mut text = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('}') else {
            break;
        };
        let (placeholder, tail) = rest.split_at(close + 1);
        let name = &placeholder[1..close];
        match variables.and_then(|v| v.get(name)).and_then(Value::as_str) {
            Some(value) => text.push_str(value),
            None => text.push_str(placeholder),
        }
        rest = tail;
    }
    text.push_str(rest);
    text
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(default)]
    pub seq: Seq,
    #[serde(flatten)]
    pub event: EventKind,
}

impl Event {
    pub fn new(event: EventKind) -> Self {
        Self {
            seq: Seq::default(),
            event,
        }
    }

    pub fn event_name(&self) -> &str {
        self.event.event_name()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<Seq>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(flatten, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, From, TryInto)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
pub enum Message {
    Request(Request),
    Response(Response),
    Event(Event),
    #[serde(untagged)]
    Custom(CustomMessage),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageType {
    Request,
    Response,
    Event,
    Other(String),
}

#[derive(Error, Debug)]
pub enum ProtocolError {
    #[error("Invalid DAP message header: {0}")]
    HeaderParseError(String),
    #[error("Decoding error: {0}")]
    DecodingError(#[from] Utf8Error),
    #[error("Serde error: {0}")]
    SerdeError(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type ProtocolResult<T> = Result<T, ProtocolError>;

impl Message {
    pub fn message_type(&self) -> MessageType {
        match self {
            Message::Request(_) => MessageType::Request,
            Message::Response(_) => MessageType::Response,
            Message::Event(_) => MessageType::Event,
            Message::Custom(CustomMessage { type_, .. }) => {
                MessageType::Other(type_.clone().unwrap_or_else(|| "custom".to_owned()))
            }
        }
    }

    pub fn seq(&self) -> Seq {
        match self {
            Message::Request(req) => req.seq,
            Message::Response(resp) => resp.seq,
            Message::Event(event) => event.seq,
            Message::Custom(CustomMessage { seq, .. }) => seq.unwrap_or_default(),
        }
    }

    pub async fn read<R: AsyncBufRead + Unpin + ?Sized>(
        input_buffer: &mut R,
    ) -> ProtocolResult<Option<Self>> {
        let mut line_buffer = String::new();
        let mut content_length: Option<usize> = None;
        let mut header_count: usize = 0;

        // Read header lines until the blank separator line.
        // The DAP base protocol allows multiple headers (e.g. Content-Type)
        // separated by \r\n, terminated by an empty \r\n line.
        //
        // Each header line is bounded to `MAX_DAP_HEADER_LINE_SIZE` bytes via
        // `take` to prevent a peer from exhausting memory by streaming bytes
        // without a `\n` terminator. The total number of headers is also
        // capped by `MAX_DAP_HEADER_COUNT`.
        loop {
            line_buffer.clear();
            let bytes_read = (&mut *input_buffer)
                .take(MAX_DAP_HEADER_LINE_SIZE as u64)
                .read_line(&mut line_buffer)
                .await
                .map_err(ProtocolError::IoError)?;

            if bytes_read == 0 {
                if content_length.is_some() {
                    return Err(ProtocolError::IoError(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "unexpected EOF while reading headers",
                    )));
                }
                return Ok(None);
            }

            // If we hit the per-line cap without seeing `\n`, the line is
            // oversized. A *short* line without `\n` means the stream ended
            // mid-line; we let it fall through to the normal header-parse
            // path (which will either accept a parsable line or surface an
            // unexpected-EOF on the next read).
            //
            // Note: DAP headers are ASCII in practice, but if the cap ever
            // splits a multi-byte UTF-8 sequence, `read_line` will surface
            // an `io::ErrorKind::InvalidData` rather than this oversize
            // error. Acceptable trade-off for the simpler implementation.
            // `bytes_read` cannot exceed `MAX_DAP_HEADER_LINE_SIZE` because of the
            // `take` cap above; equality means we filled the entire budget.
            if bytes_read == MAX_DAP_HEADER_LINE_SIZE && !line_buffer.ends_with('\n') {
                return Err(ProtocolError::HeaderParseError(format!(
                    "header line exceeds maximum length of {} bytes",
                    MAX_DAP_HEADER_LINE_SIZE
                )));
            }

            let trimmed = line_buffer.trim_end();
            if trimmed.is_empty() {
                break;
            }

            // Reject the (MAX+1)th non-empty header line; exactly
            // MAX_DAP_HEADER_COUNT headers are allowed.
            header_count += 1;
            if header_count > MAX_DAP_HEADER_COUNT {
                return Err(ProtocolError::HeaderParseError(format!(
                    "too many headers (limit: {})",
                    MAX_DAP_HEADER_COUNT
                )));
            }

            // Use split_once(':') to handle header values containing colons.
            let Some((key, value)) = trimmed.split_once(':') else {
                return Err(ProtocolError::HeaderParseError(line_buffer));
            };

            // DAP base protocol headers are case-insensitive (inherited from RFC 822
            // / HTTP); some real-world adapters ship `content-length` lowercase or
            // with extra whitespace around the key.
            if key.trim().eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    value
                        .trim()
                        .parse()
                        .map_err(|_| ProtocolError::HeaderParseError(line_buffer.clone()))?,
                );
            }
            // Other headers (e.g. Content-Type) are ignored per DAP spec.
        }

        let content_length = content_length.ok_or_else(|| {
            ProtocolError::HeaderParseError("Missing Content-Length header".into())
        })?;

        if content_length > MAX_DAP_MESSAGE_SIZE {
            return Err(ProtocolError::HeaderParseError(format!(
                "Content-Length {} exceeds maximum allowed size of {} bytes",
                content_length, MAX_DAP_MESSAGE_SIZE
            )));
        }

        let mut content = vec![0; content_length];
        input_buffer
            .read_exact(&mut content)
            .await
            .map_err(ProtocolError::IoError)?;

        let content =
            std::str::from_utf8(content.as_slice()).map_err(ProtocolError::DecodingError)?;
        let message: Self = serde_json::from_str(content).map_err(ProtocolError::SerdeError)?;

        Ok(Some(message))
    }

    /// Serialize the message to a JSON `Value`, sanitizing any null content
    /// fields produced by serde's adjacently tagged enum serialization.
    ///
    /// When an `Option`-wrapped variant (e.g. `ConfigurationDone(None)`) is
    /// serialized with `#[serde(tag = "command", content = "arguments")]`,
    /// serde emits `"arguments": null`. The DAP spec does not permit null
    /// for these fields — they should either be omitted or be a valid object.
    /// This method strips those null entries.
    pub fn to_value(&self) -> ProtocolResult<Value> {
        let mut val = serde_json::to_value(self).map_err(ProtocolError::SerdeError)?;
        if let Value::Object(ref mut map) = val {
            for key in &["arguments", "body"] {
                if map.get(*key).is_some_and(Value::is_null) {
                    map.remove(*key);
                }
            }
        }
        Ok(val)
    }

    pub fn format(&self) -> ProtocolResult<Vec<u8>> {
        let val = self.to_value()?;
        let json_bytes = serde_json::to_vec(&val).map_err(ProtocolError::SerdeError)?;

        // "Content-Length: \r\n\r\n" (20 bytes) + up to 10 digits for the length value
        let mut buf = Vec::with_capacity(32 + json_bytes.len());
        write!(buf, "Content-Length: {}\r\n\r\n", json_bytes.len())
            .map_err(ProtocolError::IoError)?;
        buf.extend_from_slice(&json_bytes);
        Ok(buf)
    }
}
