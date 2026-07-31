// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::io::BufRead;
use std::io::BufReader;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::Child;
use std::process::ChildStdout;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use debugserver_types::Event;
use debugserver_types::ProtocolMessage;
use debugserver_types::Response;

/// A dummy client to use for testing. This serves as a mock for VS code itself.
pub struct DapClient {
    child: Child,
    unread_responses: Receiver<Response>,
    unread_events: Receiver<Event>,
    _read_message_in_background: JoinHandle<()>,
}

impl DapClient {
    pub fn new(server_path: &Path) -> Result<Self> {
        let cmd = Command::new(server_path);

        Self::new_with_command(cmd)
    }

    pub fn new_with_command(mut cmd: Command) -> Result<Self> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("Failed to spawn DAP server:")?;

        let (send_response, read_response): (Sender<Response>, Receiver<Response>) =
            mpsc::channel();
        let (send_event, read_event): (Sender<Event>, Receiver<Event>) = mpsc::channel();

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to take dap server stdout"))?;
        let handler = thread::spawn(move || {
            read_messages(stdout, send_response, send_event);
        });

        let dap_client = DapClient {
            child,
            unread_responses: read_response,
            unread_events: read_event,
            _read_message_in_background: handler,
        };

        Ok(dap_client)
    }

    pub fn send(&mut self, message: String) -> Result<()> {
        self.child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("Error writing to server"))?
            .write_all(to_base_protocol(message)?.as_bytes())?;
        Ok(())
    }

    pub fn read_response(&mut self) -> Result<Response> {
        self.unread_responses
            .recv()
            .context("Failed to receive a response")
    }

    pub fn read_response_timeout(&mut self, timeout: Duration) -> Result<Response> {
        self.unread_responses
            .recv_timeout(timeout)
            .context("Timeout waiting for response")
    }

    pub fn read_event(&mut self) -> Result<Event> {
        self.unread_events
            .recv()
            .context("Failed to receive an event")
    }

    pub fn read_initialized_event(&mut self) -> Result<()> {
        let r = self.read_event()?;
        assert_eq!(r.event, "initialized");
        Ok(())
    }

    pub fn read_stopped_event(&mut self) -> Result<()> {
        let r = self.read_event()?;
        assert_eq!(r.event, "stopped");
        Ok(())
    }

    pub fn kill(&mut self) -> Result<()> {
        if let Err(e) = self.child.kill() {
            if e.kind() == ErrorKind::InvalidInput {
                anyhow::bail!("Child process has already exited");
            }
        }
        Ok(())
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        // try to shutdown the dap process
        let _ = self.child.kill();
    }
}

/// Loop through stdout content, parse it into event or responses and send them to the right channel.
fn read_messages(
    stdout: ChildStdout,
    response_sender: Sender<Response>,
    event_sender: Sender<Event>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_dap_message(&mut reader) {
            Ok(Some(msg)) => {
                let result = parse_and_send(&msg, &response_sender, &event_sender);
                if let Err(e) = result {
                    eprintln!("Failed to parse and send message: {:?}", e);
                }
            }
            Ok(None) => {
                eprintln!("Reached EOF on DAP stdout. Exiting loop.");
                return;
            }
            Err(e) => eprintln!("Failed to read message: {:?}", e),
        }
    }
}

fn parse_and_send(
    msg: &str,
    response_sender: &Sender<Response>,
    event_sender: &Sender<Event>,
) -> Result<()> {
    match serde_json::from_str::<ProtocolMessage>(msg) {
        Ok(ProtocolMessage {
            seq: _,
            type_: ref t,
        }) if t == "response" => {
            let response = serde_json::from_str::<Response>(msg)?;
            response_sender.send(response)?;
            Ok(())
        }
        Ok(ProtocolMessage {
            seq: _,
            type_: ref t,
        }) if t == "event" => {
            let event = serde_json::from_str::<Event>(msg)?;
            event_sender.send(event)?;
            Ok(())
        }
        Ok(message) => Err(anyhow!(format!(
            "Invalid message type {}. Only response and event types are supported",
            message.type_
        ))),
        Err(e) => Err(anyhow!(format!(
            "Failed to parse message. Couldn't parse {}. Error: {:?}",
            msg, e
        ))),
    }
}

fn read_dap_message<R>(reader: &mut BufReader<R>) -> Result<Option<String>>
where
    R: Read,
{
    let mut buf = vec![0u8; 100];
    let nbytes = reader.read_until(b'\n', &mut buf)?;
    // EOF
    if nbytes == 0 {
        return Ok(None);
    }

    let header = String::from_utf8_lossy(&buf).to_string();
    reader.read_until(b'\n', &mut buf)?;

    let header = header
        .trim_matches(char::from(0))
        .strip_prefix("Content-Length:")
        .ok_or_else(|| anyhow!("Incorrect base protocol"))?;
    let len: usize = header.trim().parse()?;

    let mut res = vec![0u8; len];
    reader.read_exact(&mut res)?;
    let res = String::from_utf8_lossy(&res).to_string();
    Ok(Some(res))
}

fn to_base_protocol(content: String) -> Result<String> {
    let mut ret = format!("Content-Length: {:?}\r\n\r\n", content.len());
    ret.push_str(&content);
    Ok(ret)
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::io::Cursor;
    use std::sync::mpsc;

    use debugserver_types::Event;
    use debugserver_types::Response;
    use serde_json::Value;
    use serde_json::json;

    use super::parse_and_send;
    use super::read_dap_message;
    use super::to_base_protocol;

    #[test]
    fn test_parse_and_send() {
        let (response_sender, response_receiver) = mpsc::channel::<Response>();
        let (event_sender, event_receiver) = mpsc::channel::<Event>();

        let response_msg = r#"{"body":{"supportsConfigurationDoneRequest":true,"supportsFunctionBreakpoints":true},"command":"initialize","request_seq":1,"seq":0,"success":true,"type":"response"}"#;
        parse_and_send(response_msg, &response_sender, &event_sender)
            .expect("Valid response. Expected not to fail.");
        assert!(
            response_receiver.try_recv().is_ok(),
            "Expected to receive a response"
        );
        assert!(
            event_receiver.try_recv().is_err(),
            "Not expected to receive an event"
        );

        let event_msg = r#"{"event":"initialized","seq":0,"type":"event"}"#;
        parse_and_send(event_msg, &response_sender, &event_sender)
            .expect("Valid event. Expected not to fail.");
        assert!(
            response_receiver.try_recv().is_err(),
            "Not expected to receive a response"
        );
        assert!(
            event_receiver.try_recv().is_ok(),
            "Expected to receive an event"
        );

        let msg = r#"invalid message"#;
        let r = parse_and_send(msg, &response_sender, &event_sender);
        assert!(r.is_err(), "Reading an invalid message, expecting a error");
        assert!(
            response_receiver.try_recv().is_err(),
            "Not expected to receive a response"
        );
        assert!(
            event_receiver.try_recv().is_err(),
            "Not expected to receive an event"
        );
    }

    #[test]
    fn test_read_dap_message_valid_message() -> anyhow::Result<()> {
        let body = json!({
          "body": {
            "supportsConfigurationDoneRequest": true,
            "supportsFunctionBreakpoints": true
          },
          "command": "initialize",
          "request_seq": 1,
          "seq": 0,
          "success": true,
          "type": "response"
        });
        let msg = to_base_protocol(body.to_string()).expect("Valid body, expected not to fail");
        let cursor = Cursor::new(msg.as_bytes().to_vec());
        let mut buffer = BufReader::new(cursor);

        let msg = read_dap_message::<Cursor<Vec<u8>>>(&mut buffer)?;

        assert_eq!(
            body,
            serde_json::from_str::<Value>(&msg.unwrap()).expect("Valid body. Expected not to fail")
        );
        Ok(())
    }

    #[test]
    fn test_read_dap_message_invalid_message() {
        let msg = "Content-Length: 123\r\n\r\n{\"Wrong size\":\"this should fail\"}";
        let cursor = Cursor::new(msg.as_bytes().to_vec());
        let mut buffer = BufReader::new(cursor);

        let result = read_dap_message::<Cursor<Vec<u8>>>(&mut buffer);

        assert!(result.is_err());
    }
}
