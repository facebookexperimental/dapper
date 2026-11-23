// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! A headless proxy reports the access policy in its session config in
//! `DAPPER_READY` and in `status`, and the child session it spawns for the
//! adapter's `startDebugging` reverse request enforces the same policy.
//!
//! Child-session spawning is Unix-only, so the whole crate is gated on Unix.

#![cfg(unix)]

use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use dapper_access::Policy;
use dapper_access::PolicyReport;
use dapper_access::Preset;
use dapper_e2e_support::ProgressEvent;
use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::parse_progress_event;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::spawn_proxy_from_config;
use dapper_session::ScopeId;
use dapper_session::config::AccessConfig;
use dapper_session::config::ChildSessionConfig;
use dapper_session::config::DebugRequest;
use dapper_session::config::DebugSessionConfig;
use dapper_session::config::SpawnConfig;
use dapper_session::config::StdioSpawnConfig;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::task::JoinSet;

#[tokio::test]
async fn a_headless_session_and_its_child_enforce_the_same_access_policy() -> anyhow::Result<()> {
    let scope_id = generate_test_scope_id("headless-access-policy");
    let adapter_path = std::env::var("DAPPER_TEST_ADAPTER_EXECUTABLE")
        .context("DAPPER_TEST_ADAPTER_EXECUTABLE must name the DAP adapter")?;

    let child_sessions: ChildSessionConfig = serde_json::from_value(json!({
        "autoSpawn": true,
        "maxChildren": 4,
        "maxDepth": 1,
        "profile": {
            "rules": [{
                "when": { "request": "launch" },
                "childBackend": { "type": "stdio", "cmd": adapter_path, "args": [] },
                "debugRequest": { "request": "launch", "arguments": {} }
            }]
        }
    }))?;
    let config = DebugSessionConfig::builder()
        .spawn_config(SpawnConfig::Stdio(StdioSpawnConfig {
            cmd: adapter_path.clone(),
            args: vec!["--emit-start-debugging".to_string()],
            new_session: true,
        }))
        .debug_request(DebugRequest::Launch(serde_json::from_value(json!({}))?))
        .install_default_exception_breakpoints(false)
        .child_sessions(child_sessions)
        .access(AccessConfig {
            allow: vec!["minimal".parse()?],
        })
        .build();

    let (mut proxy, _config_file, _adapter_log) =
        spawn_proxy_from_config(&config, &scope_id).await?;

    // Drain the proxy's stdout for the whole test so it never blocks on a full
    // pipe, keeping what its DAPPER_READY reports.
    let stdout = proxy.stdout.take().context("the proxy has no stdout")?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let mut drain = JoinSet::new();
    drain.spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut ready_tx = Some(ready_tx);
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(ProgressEvent::DapperReady {
                control_port,
                access,
                ..
            }) = parse_progress_event(&line)
                && let Some(tx) = ready_tx.take()
            {
                let _ = tx.send((control_port, access));
            }
        }
    });
    let (parent_port, reported) = tokio::time::timeout(Duration::from_secs(60), ready_rx)
        .await
        .context("timed out waiting for DAPPER_READY")?
        .context("the proxy's stdout ended before DAPPER_READY")?;

    assert_eq!(
        reported,
        Some(PolicyReport::from(Policy::try_new(
            Preset::Minimal.operations()
        )?)),
        "DAPPER_READY should report the policy in force"
    );

    let child_port = child_session_port(&scope_id, parent_port.get()).await?;
    for port in [parent_port.get(), child_port] {
        let port = port.to_string();
        let status = run_debug_command(
            Some(scope_id.clone()),
            &["--control-port", &port, "status", "--json"],
        )
        .await?;
        anyhow::ensure!(status.success, "status failed, stderr: {}", status.stderr);
        let status: serde_json::Value = serde_json::from_str(&status.stdout)
            .with_context(|| format!("status --json not valid JSON: {}", status.stdout))?;
        assert_eq!(
            status["result"]["access"],
            json!({ "allow": ["inspect", "read_values"] }),
            "the session on port {port} should report the parent's policy"
        );

        let eval = run_debug_command(
            Some(scope_id.clone()),
            &["--control-port", &port, "eval", "1"],
        )
        .await?;
        assert!(
            !eval.success,
            "the session on port {port} should refuse eval"
        );
        assert!(
            eval.stderr.contains("`eval_repl` needs evaluate"),
            "the refusal should say what eval needs; stderr: {}",
            eval.stderr
        );
    }

    // `stop` needs an operation `minimal` does not allow; the child proxy dies
    // with its parent.
    proxy.kill().await?;
    Ok(())
}

/// The control port of the session in `scope_id` other than the parent's,
/// once the parent has spawned it.
async fn child_session_port(scope_id: &ScopeId, parent_port: u16) -> anyhow::Result<u16> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out waiting for the child session to appear in scope {}",
            scope_id.as_str()
        );
        let listing = run_debug_command(Some(scope_id.clone()), &["sessions", "--json"]).await?;
        if listing.success {
            let listing: serde_json::Value = serde_json::from_str(&listing.stdout)
                .with_context(|| format!("sessions --json not valid JSON: {}", listing.stdout))?;
            let child_port = listing["result"]["sessions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|session| session["controlPlanePort"].as_u64())
                .find(|port| *port != u64::from(parent_port));
            if let Some(port) = child_port {
                return Ok(u16::try_from(port)?);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
