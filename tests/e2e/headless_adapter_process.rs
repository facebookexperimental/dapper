// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! End-to-end coverage for what a headless proxy reports about the debug
//! adapter process it spawns, using `fake_dap_adapter` as the adapter.

use std::time::Duration;

use anyhow::Context;
use dapper_e2e_support::ProgressEvent;
use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::parse_progress_event;
use dapper_e2e_support::spawn_proxy_from_config;
use dapper_session::config::DebugRequest;
use dapper_session::config::DebugSessionConfig;
use dapper_session::config::SpawnConfig;
use dapper_session::config::StdioSpawnConfig;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Child;

const EVENTS_TIMEOUT: Duration = Duration::from_secs(60);

fn adapter_path() -> anyhow::Result<String> {
    std::env::var("DAPPER_TEST_ADAPTER_EXECUTABLE")
        .context("DAPPER_TEST_ADAPTER_EXECUTABLE must name the DAP adapter")
}

fn headless_config() -> anyhow::Result<DebugSessionConfig> {
    Ok(DebugSessionConfig {
        spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
            cmd: adapter_path()?,
            args: vec![],
            new_session: true,
        }),
        debug_request: Some(DebugRequest::Launch(serde_json::from_value(json!({}))?)),
        breakpoints: vec![],
        metadata: Default::default(),
        initialize_args: None,
        init_timeout_secs: None,
        install_default_exception_breakpoints: false,
        child_sessions: None,
    })
}

async fn events_until_ready(proxy: &mut Child) -> anyhow::Result<Vec<ProgressEvent>> {
    let stdout = proxy.stdout.take().context("proxy has no stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut events = Vec::new();
    tokio::time::timeout(EVENTS_TIMEOUT, async {
        while let Some(line) = lines.next_line().await? {
            let Some(event) = parse_progress_event(&line) else {
                continue;
            };
            let ready = matches!(event, ProgressEvent::SessionReady { .. });
            events.push(event);
            if ready {
                return Ok(());
            }
        }
        anyhow::bail!("proxy stdout closed before SESSION_READY")
    })
    .await
    .context("timed out waiting for SESSION_READY")??;
    Ok(events)
}

#[tokio::test]
async fn headless_session_reports_the_adapter_pid_first() -> anyhow::Result<()> {
    let scope_id = generate_test_scope_id("headless-adapter-pid");
    let (mut proxy, _config_file, _adapter_log) =
        spawn_proxy_from_config(&headless_config()?, &scope_id).await?;

    let events = events_until_ready(&mut proxy).await?;

    let Some(ProgressEvent::AdapterStarted { pid, .. }) = events.first() else {
        anyhow::bail!("ADAPTER_STARTED should be the first event, got {events:?}");
    };
    if cfg!(target_os = "linux") {
        assert_eq!(
            tokio::fs::canonicalize(format!("/proc/{pid}/exe")).await?,
            tokio::fs::canonicalize(adapter_path()?).await?,
            "the pid should be the adapter the proxy spawned"
        );
    }

    proxy.kill().await?;
    Ok(())
}
