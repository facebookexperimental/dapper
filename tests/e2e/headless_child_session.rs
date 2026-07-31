// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! End-to-end coverage for headless child-session spawning via the DAP
//! `startDebugging` reverse request.
//!
//! A headless parent proxy is configured with `childSessions.autoSpawn` and a
//! single declarative rule. Its backend is `fake_dap_adapter
//! --emit-start-debugging`, which emits a `startDebugging` reverse request once
//! the handshake completes. The parent advertises the capability (Unix +
//! autoSpawn + maxDepth>0 + maxChildren>0 + a resolvable rule), resolves the
//! request against the rule, and spawns a peer child `dapper proxy from-config`
//! whose backend is a plain (leaf) `fake_dap_adapter`. The test asserts a
//! second session appears in the scope and is independently drivable over its
//! own control port.
//!
//! Child-session spawning is Unix-only, so the whole crate is gated on Unix.

#![cfg(unix)]

use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use dapper_e2e_support::ProgressEvent;
use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::parse_progress_event;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::spawn_proxy_from_config;
use dapper_session::config::ChildSessionConfig;
use dapper_session::config::DebugRequest;
use dapper_session::config::DebugSessionConfig;
use dapper_session::config::SpawnConfig;
use dapper_session::config::StdioSpawnConfig;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;

#[tokio::test]
async fn headless_start_debugging_spawns_child_session() -> anyhow::Result<()> {
    let scope_id = generate_test_scope_id("headless-child-session");
    let adapter_path = std::env::var("DAPPER_TEST_ADAPTER_EXECUTABLE")
        .context("DAPPER_TEST_ADAPTER_EXECUTABLE must name the DAP adapter")?;

    // Declarative child-session profile: a single rule that matches the
    // `launch` reverse request the fake adapter emits and resolves it to a
    // plain (leaf) fake adapter over stdio. maxDepth=1 means the spawned child
    // carries maxDepth=0, so it neither advertises the capability nor spawns a
    // grandchild even though it runs the same fixture binary.
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

    let config = DebugSessionConfig {
        spawn_config: SpawnConfig::Stdio(StdioSpawnConfig {
            cmd: adapter_path.clone(),
            args: vec!["--emit-start-debugging".to_string()],
            new_session: true,
        }),
        // debug_request present -> headless mode (no external DAP client).
        debug_request: Some(DebugRequest::Launch(serde_json::from_value(json!({}))?)),
        breakpoints: vec![],
        metadata: Default::default(),
        initialize_args: None,
        init_timeout_secs: None,
        install_default_exception_breakpoints: false,
        child_sessions: Some(child_sessions),
    };

    let (mut child, _config_file, _adapter_log) =
        spawn_proxy_from_config(&config, &scope_id).await?;

    // Drain the parent's stdout for the whole test so it never blocks on a full
    // pipe, capturing its own control-plane port so we can tell the spawned
    // child apart from the parent in the session listing.
    let stdout = child.stdout.take().context("parent proxy has no stdout")?;
    let (port_tx, port_rx) = tokio::sync::oneshot::channel();
    let drain = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut port_tx = Some(port_tx);
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(ProgressEvent::DapperReady { control_port, .. }) =
                parse_progress_event(&line)
            {
                if let Some(tx) = port_tx.take() {
                    let _ = tx.send(control_port);
                }
            }
        }
    });

    // Run the rest capturing the result so the parent proxy is torn down no
    // matter where we exit. `spawn_proxy_from_config` does not set
    // `kill_on_drop`, so an early `?`/`ensure!` return would otherwise leak the
    // parent — and, via the child's `PR_SET_PDEATHSIG` on it, the spawned child
    // and its adapter too — which is flaky on a slow CI host.
    let result: anyhow::Result<()> = async {
        let parent_port = tokio::time::timeout(Duration::from_secs(60), port_rx)
            .await
            .context("timed out waiting for parent DAPPER_READY")?
            .context("parent stdout drain ended before DAPPER_READY")?
            .get();

        // Poll the scope's session listing until the spawned child appears (a
        // second session whose control port differs from the parent's). The
        // scope is unique per test, so any non-parent session is the spawned
        // child.
        let deadline = Instant::now() + Duration::from_secs(60);
        let child_port = loop {
            anyhow::ensure!(
                Instant::now() < deadline,
                "timed out waiting for the child session to appear in scope {}",
                scope_id.as_str()
            );

            let result = run_debug_command(Some(scope_id.clone()), &["sessions", "--json"]).await?;
            if result.success {
                let parsed: serde_json::Value =
                    serde_json::from_str(&result.stdout).with_context(|| {
                        format!("sessions --json not valid JSON: {}", result.stdout)
                    })?;
                if let Some(sessions) = parsed["result"]["sessions"].as_array() {
                    if let Some(found) = sessions.iter().find_map(|s| {
                        let port = s["controlPlanePort"].as_u64()?;
                        (port != parent_port as u64).then_some(port)
                    }) {
                        break found as u16;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };

        // The child records its parent: the session listing must show the child
        // with `parentSessionId` equal to the (root) parent's `sessionId`, and
        // the parent itself must report no `parentSessionId`.
        let listing = run_debug_command(Some(scope_id.clone()), &["sessions", "--json"]).await?;
        anyhow::ensure!(
            listing.success,
            "sessions --json failed, stderr: {}",
            listing.stderr
        );
        let listing: serde_json::Value = serde_json::from_str(&listing.stdout)
            .with_context(|| format!("sessions --json not valid JSON: {}", listing.stdout))?;
        let sessions = listing["result"]["sessions"]
            .as_array()
            .context("sessions response missing sessions array")?;
        let parent = sessions
            .iter()
            .find(|s| s["controlPlanePort"].as_u64() == Some(parent_port as u64))
            .context("parent session not found in listing")?;
        let child_session = sessions
            .iter()
            .find(|s| s["controlPlanePort"].as_u64() == Some(child_port as u64))
            .context("child session not found in listing")?;
        anyhow::ensure!(
            parent.get("parentSessionId").is_none(),
            "root parent must not report a parentSessionId, got: {parent}"
        );
        anyhow::ensure!(
            child_session["parentSessionId"] == parent["sessionId"],
            "child parentSessionId {:?} should equal parent sessionId {:?}",
            child_session.get("parentSessionId"),
            parent.get("sessionId")
        );

        // The child is independently drivable over its own control port: it is a
        // leaf fake adapter stopped at entry, so `threads` returns its thread.
        let child_port_str = child_port.to_string();
        let threads = run_debug_command(
            Some(scope_id.clone()),
            &["--control-port", &child_port_str, "threads", "--json"],
        )
        .await?;
        anyhow::ensure!(
            threads.success,
            "driving the child session failed, stderr: {}",
            threads.stderr
        );
        let parsed: serde_json::Value = serde_json::from_str(&threads.stdout)
            .with_context(|| format!("child threads --json not valid JSON: {}", threads.stdout))?;
        let thread_ids = parsed["result"]["threads"]
            .as_array()
            .context("child threads response missing threads array")?;
        anyhow::ensure!(
            !thread_ids.is_empty(),
            "child session should report at least one thread"
        );
        // Gracefully stop the parent via the control plane. Its `stop` hook runs
        // the children-before-parent teardown cascade — `ChildTeardown::teardown`
        // SIGTERMs the child proxy's process group — so the child shuts down and
        // removes its `SessionInfo`. Asserting the child session disappears from
        // the scope proves the teardown cascade ran end-to-end (not just the
        // `PR_SET_PDEATHSIG` backstop).
        let parent_port_str = parent_port.to_string();
        let stop = run_debug_command(
            Some(scope_id.clone()),
            &["--control-port", &parent_port_str, "stop"],
        )
        .await?;
        anyhow::ensure!(
            stop.success,
            "stopping the parent failed, stderr: {}",
            stop.stderr
        );

        let teardown_deadline = Instant::now() + Duration::from_secs(60);
        loop {
            anyhow::ensure!(
                Instant::now() < teardown_deadline,
                "child session (port {child_port}) did not disappear after the parent was stopped"
            );
            let listing =
                run_debug_command(Some(scope_id.clone()), &["sessions", "--json"]).await?;
            if listing.success {
                let parsed: serde_json::Value = serde_json::from_str(&listing.stdout)
                    .with_context(|| {
                        format!("sessions --json not valid JSON: {}", listing.stdout)
                    })?;
                let child_present = parsed["result"]["sessions"].as_array().is_some_and(|s| {
                    s.iter()
                        .any(|x| x["controlPlanePort"].as_u64() == Some(child_port as u64))
                });
                if !child_present {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        Ok(())
    }
    .await;

    // The graceful stop above is the normal teardown path; this is a safety net
    // for early-return cases (an assertion failed before the stop), so the parent
    // — and, via PR_SET_PDEATHSIG, the child — never leak on a slow host.
    child.kill().await.ok();
    drain.abort();
    result
}
