// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Minimal stdio DAP adapter used by dapper e2e tests for reverse-debugging
//! coverage. Hand-crafts JSON to stay independent of `dapper_dap_protocol` so
//! the fixture is an oracle rather than a self-test of dapper's own types.
//!
//! Configured by CLI args (NOT env vars) so each test can spawn a subprocess
//! with its own configuration without racing other parallel tests on the
//! shared process environment.

use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;

#[derive(Parser, Debug)]
#[command(about = "Fake DAP adapter for dapper e2e tests")]
struct Args {
    /// Whether to advertise Capabilities.supportsStepBack in the initialize
    /// response. Pass as `--supports-step-back true` or `false`; the bare flag
    /// form is intentionally not supported so callers must be explicit.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    supports_step_back: bool,
    /// If set, the adapter exits with a non-zero status if it receives
    /// `stepBack` or `reverseContinue`. Used by the gating-refusal e2e test
    /// to prove the proxy never forwarded the request.
    #[arg(long, default_value_t = false)]
    fail_on_reverse: bool,
    /// Emit a `startDebugging` reverse request after the handshake (request
    /// `launch`, empty `configuration`), to drive the headless child-session e2e.
    #[arg(long, default_value_t = false)]
    emit_start_debugging: bool,
}

static SEQ: AtomicI64 = AtomicI64::new(1);

fn next_seq() -> i64 {
    // Relaxed is sufficient for a simple monotonic counter: fetch_add is
    // atomic and no dependent loads/stores need to be ordered with respect
    // to this increment, regardless of tokio runtime flavor.
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Sanity cap on `Content-Length` so a malformed peer can't trigger an
/// allocation failure / OOM. 16 MiB is far above any realistic DAP message.
const MAX_BODY: usize = 16 * 1024 * 1024;

async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut header_line = Vec::new();

    loop {
        header_line.clear();
        let n = reader.read_until(b'\n', &mut header_line).await?;
        if n == 0 {
            return Ok(None); // clean EOF before any bytes
        }
        if !header_line.ends_with(b"\n") {
            anyhow::bail!("peer closed mid-header line: {:?}", header_line);
        }
        if !header_line.ends_with(b"\r\n") {
            anyhow::bail!(
                "header line missing CR before LF (DAP requires CRLF): {:?}",
                header_line
            );
        }
        if header_line == b"\r\n" {
            break; // end of headers
        }
        let header = std::str::from_utf8(&header_line)?.trim();
        // DAP headers are case-insensitive (inherited from LSP); compare on
        // the lowercased form so unusual casings still parse.
        let lower = header.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = Some(rest.trim().parse().context("invalid Content-Length")?);
        }
    }

    let len = content_length.context("missing Content-Length header")?;
    anyhow::ensure!(
        len <= MAX_BODY,
        "Content-Length {} exceeds {} byte cap",
        len,
        MAX_BODY,
    );
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    let value: Value = serde_json::from_slice(&body).context("invalid JSON body")?;
    Ok(Some(value))
}

async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, value: Value) -> Result<()> {
    let body = serde_json::to_vec(&value)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

async fn send_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request_seq: i64,
    command: &str,
    body: Option<Value>,
) -> Result<()> {
    let mut response = json!({
        "type": "response",
        "seq": next_seq(),
        "request_seq": request_seq,
        "success": true,
        "command": command,
    });
    if let Some(body) = body {
        response["body"] = body;
    }
    write_message(writer, response).await
}

async fn send_event<W: AsyncWrite + Unpin>(
    writer: &mut W,
    event: &str,
    body: Option<Value>,
) -> Result<()> {
    let mut payload = json!({
        "type": "event",
        "seq": next_seq(),
        "event": event,
    });
    if let Some(body) = body {
        payload["body"] = body;
    }
    write_message(writer, payload).await
}

/// Emit a reverse request (adapter -> client) such as `startDebugging`. Real
/// adapters send these to ask the client to perform an action on their behalf;
/// the fake uses it only to exercise the proxy's reverse-request handling.
async fn send_reverse_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    command: &str,
    arguments: Value,
) -> Result<()> {
    let request = json!({
        "type": "request",
        "seq": next_seq(),
        "command": command,
        "arguments": arguments,
    });
    write_message(writer, request).await
}

fn stopped_body(reason: &str) -> Value {
    json!({
        "reason": reason,
        "threadId": 1,
        "allThreadsStopped": true,
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    // --fail-on-reverse only fires if a reverse request reaches the fake,
    // which can only happen when the proxy was not gating the request — i.e.
    // when supports_step_back is false. Pairing it with --supports-step-back
    // true is almost certainly a test-author mistake. Use eprintln+exit
    // (rather than bail!) for the same reason as the in-band failure path
    // below: clean closed pipe, no multi-line backtrace.
    if args.fail_on_reverse && args.supports_step_back {
        eprintln!(
            "--fail-on-reverse only makes sense with --supports-step-back false; otherwise the proxy forwards the reverse request and the fake exits unexpectedly"
        );
        std::process::exit(2);
    }
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);

    while let Some(message) = read_message(&mut reader).await? {
        // A `response` here is the ack to our reverse request, not a new
        // request. Stay a strict oracle: a `success: false` ack means the proxy
        // failed to honor it, so exit loudly; consume a successful ack.
        if message["type"].as_str() == Some("response") {
            if message["success"].as_bool() != Some(true) {
                eprintln!(
                    "fake_dap_adapter got a failed reverse-request ack: {}",
                    message
                );
                std::process::exit(2);
            }
            continue;
        }
        // Fail fast on malformed protocol — the fake's job is to be a strict
        // oracle, so a missing `seq` or `command` should surface as an error
        // rather than silently defaulting to 0 / "".
        let request_seq = message["seq"]
            .as_i64()
            .context("request missing required `seq` field")?;
        let command = message["command"]
            .as_str()
            .context("request missing required `command` field")?
            .to_string();

        match command.as_str() {
            "initialize" => {
                let caps = json!({
                    "supportsConfigurationDoneRequest": true,
                    "supportsStepBack": args.supports_step_back,
                });
                send_response(&mut stdout, request_seq, "initialize", Some(caps)).await?;
            }
            "launch" => {
                send_response(&mut stdout, request_seq, &command, None).await?;
                // The dapper e2e harness blocks on `wait_for_event("initialized")`
                // immediately after sending `launch`, so emit it here. (Real
                // adapters typically emit `initialized` right after the
                // `initialize` response; the fake follows the harness's
                // sequencing.) `attach` is not handled — no test exercises
                // it, and the unknown-command branch will surface its use
                // loudly if a future test needs it.
                send_event(&mut stdout, "initialized", None).await?;
            }
            "configurationDone" => {
                send_response(&mut stdout, request_seq, "configurationDone", None).await?;
                send_event(&mut stdout, "stopped", Some(stopped_body("entry"))).await?;
                // Emit the reverse request only after the handshake is fully
                // complete, mirroring how real adapters (e.g. debugpy with
                // subProcess) request a child session once the parent is live.
                if args.emit_start_debugging {
                    send_reverse_request(
                        &mut stdout,
                        "startDebugging",
                        json!({ "request": "launch", "configuration": {} }),
                    )
                    .await?;
                }
            }
            "threads" => {
                let body = json!({
                    "threads": [{ "id": 1, "name": "main" }],
                });
                send_response(&mut stdout, request_seq, "threads", Some(body)).await?;
            }
            "stackTrace" => {
                let body = json!({
                    "stackFrames": [{
                        "id": 1,
                        "name": "fake_frame",
                        "line": 1,
                        "column": 1,
                        "source": { "name": "fake.rs", "path": "/fake/fake.rs" },
                    }],
                    "totalFrames": 1,
                });
                send_response(&mut stdout, request_seq, "stackTrace", Some(body)).await?;
            }
            // This guarded arm must precede the catch-all step arm below;
            // Rust evaluates match arms top-to-bottom, so the guard relies on
            // ordering to short-circuit before the unguarded arm.
            "stepBack" | "reverseContinue" if args.fail_on_reverse => {
                // Use eprintln + exit rather than panic so the failure
                // surfaces as a clean closed pipe + non-zero exit, not a
                // multi-line backtrace. The follow-up liveness check in
                // the gating-refusal e2e test then sees a broken pipe
                // when it tries to call the proxy, which is the signal
                // the test relies on.
                eprintln!(
                    "fake_dap_adapter received {} but --fail-on-reverse is set; the proxy should have gated this request",
                    command
                );
                std::process::exit(2);
            }
            "continue" | "next" | "stepIn" | "stepOut" | "stepBack" | "reverseContinue" => {
                send_response(&mut stdout, request_seq, &command, None).await?;
                send_event(&mut stdout, "stopped", Some(stopped_body("step"))).await?;
            }
            "disconnect" | "terminate" => {
                send_response(&mut stdout, request_seq, &command, None).await?;
                break;
            }
            other => {
                // Unknown command: respond with an error so dapper sees structured
                // failure rather than a hang.
                let response = json!({
                    "type": "response",
                    "seq": next_seq(),
                    "request_seq": request_seq,
                    "success": false,
                    "command": other,
                    "message": format!("unknown command in fake_dap_adapter: {}", other),
                });
                write_message(&mut stdout, response).await?;
            }
        }
    }

    Ok(())
}
