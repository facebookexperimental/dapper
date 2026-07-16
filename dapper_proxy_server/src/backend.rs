// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::path::Path;
use std::process::Stdio;

use anyhow::bail;
use dapper_dap_protocol::protocol::Message;
use dapper_dap_protocol::protocol::ProtocolError;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::transport::DuplexChannel;

pub struct Backend {
    pub duplex: DuplexChannel,
    pub handle: Option<JoinHandle<anyhow::Result<()>>>,
}

impl Backend {
    pub async fn from_tcp(host: &str, port: u16) -> anyhow::Result<Self> {
        tracing::debug!("Backend: connecting to TCP {}:{}", host, port);

        let duplex = DuplexChannel::from_tcp_client(host, port).await?;
        tracing::info!("Backend: connected to TCP {}:{}", host, port);

        Ok(Self {
            duplex,
            handle: None,
        })
    }

    #[cfg(unix)]
    pub async fn from_uds(path: &Path) -> anyhow::Result<Self> {
        tracing::debug!("Backend: connecting to Unix socket {:?}", path);

        let duplex = DuplexChannel::from_uds_client(path).await?;
        tracing::info!("Backend: connected to Unix socket {:?}", path);

        Ok(Self {
            duplex,
            handle: None,
        })
    }

    pub async fn from_process(args: &[impl AsRef<str>], new_session: bool) -> anyhow::Result<Self> {
        if args.is_empty() {
            bail!("No arguments provided");
        }
        let program = args[0].as_ref().to_owned();
        let args = args[1..]
            .iter()
            .map(|s| s.as_ref().to_owned())
            .collect::<Vec<_>>();
        tracing::debug!("Backend: starting {} {:?}", program, args);

        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Start the debug adapter in a new session so that neither it nor its
        // descendants (the debuggee) can steal the terminal's foreground
        // process group via tcsetpgrp(). Without this, the debuggee becomes
        // the foreground group and Ctrl+C never reaches dapper.
        //
        // Only enabled for `from-config` mode (where the proxy is launched by
        // another tool). In direct `process` mode users may rely on the
        // debug adapter sharing the terminal session.
        //
        // SAFETY: pre_exec runs between fork() and exec(), where only
        // async-signal-safe functions may be called (See
        // https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.pre_exec).
        // Rust marks it unsafe because the compiler can't verify that the closure
        // only calls async-signal-safe functions, libc::setsid() is on the
        // POSIX async-signal-safe list (See
        // https://man7.org/linux/man-pages/man7/signal-safety.7.html)
        #[cfg(unix)]
        if new_session {
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let mut process = cmd.spawn()?;
        tracing::info!("Backend: started {:?}", process.id().unwrap_or_default());

        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to take stdin"))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to take stdout"))?;
        let stderr = process
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to take stderr"))?;

        let duplex = DuplexChannel::from_streams(Box::new(stdin), Box::new(stdout));

        let log_handle = tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stderr);
            let mut line = String::new();
            while let Ok(size) = reader.read_line(&mut line).await {
                if (size == 0) && (line.is_empty()) {
                    break;
                }
                tracing::warn!("Backend stderr: {}", line);
                line.clear();
            }
        });
        let handle = tokio::spawn(async move {
            let result = process.wait().await;
            match &result {
                Ok(status) if status.success() => {
                    tracing::info!("Backend process exited successfully");
                }
                _ => {
                    tracing::error!("Backend process exited with code {:?}", result);
                }
            }

            // Cleanup: abort log reader
            log_handle.abort();

            // Cleanup: kill process if still running
            if let Err(e) = process.kill().await {
                tracing::error!("Failed to kill backend process: {:?}", e);
            }

            Ok(())
        });

        Ok(Self {
            duplex,
            handle: Some(handle),
        })
    }

    pub async fn send(&mut self, message: Message) -> anyhow::Result<()> {
        self.duplex.send(message).await
    }

    pub async fn recv(&mut self) -> Result<Option<Message>, ProtocolError> {
        self.duplex.recv().await
    }
}
