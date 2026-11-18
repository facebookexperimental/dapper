// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! How the real adapters handle requests sent after one has hit dapper's
//! `[request]` timeout while the adapter is still working on it.
//!
//! The timeout only stops dapper waiting: the adapter keeps running the slow
//! request. Dapper sends a DAP `cancel` when the adapter advertises
//! `supportsCancelRequest`, but that is best effort. Whether the next request
//! is answered depends on the adapter:
//! - debugpy can't cancel. It answers `threads` immediately, but runs
//!   `evaluate`s one at a time on the stopped thread, so a second `evaluate`
//!   waits for the first.
//! - lldb-dap handles one request at a time. It accepts the cancel, but can
//!   only interrupt work that checks for interrupts (stack unwinding, the IR
//!   interpreter, symbol loading), not the Python `sleep` used here. So every
//!   request still waits for the slow one, and each can burn its own timeout
//!   until the slow one finishes. Cancelled follow-ups are skipped rather than
//!   run late.
//!
//! Both recover once the slow request finishes. If an adapter upgrade changes
//! this, the expectations below fail and the timeout defaults need revisiting.

use std::time::Duration;
use std::time::Instant;

use dapper_e2e_support::DapClient;
use dapper_e2e_support::RealAdapterProfile;
use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::real_adapter_profile;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::start_stopped_debug_session_with_config_dir;
use dapper_session::ScopeId;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const SLOW_EVALUATE: Duration = Duration::from_secs(10);
/// Slack on top of the timeout for CLI startup and control-plane overhead.
const OVERHEAD: Duration = Duration::from_secs(2);

/// An `evaluate` that keeps the adapter busy for `duration`: debuggee code for
/// debugpy, an LLDB command (so a debugger-side delay) for lldb-dap.
fn slow_evaluate_expression(profile: RealAdapterProfile, duration: Duration) -> String {
    let secs = duration.as_secs();
    match profile {
        RealAdapterProfile::Debugpy => format!("__import__('time').sleep({secs})"),
        RealAdapterProfile::Lldb => format!("script import time; time.sleep({secs})"),
    }
}

struct Outcome {
    elapsed: Duration,
    result: Result<String, String>,
}

impl Outcome {
    fn timed_out(&self, command: &str) -> bool {
        matches!(&self.result, Err(stderr) if stderr.contains(&format!(
            "DAP request '{command}' timed out after {}s",
            REQUEST_TIMEOUT.as_secs()
        )))
    }
}

async fn run(scope_id: &ScopeId, args: &[&str]) -> anyhow::Result<Outcome> {
    let start = Instant::now();
    let output = run_debug_command(Some(scope_id.clone()), args).await?;
    Ok(Outcome {
        elapsed: start.elapsed(),
        result: if output.success {
            Ok(output.stdout)
        } else {
            Err(output.stderr)
        },
    })
}

fn start_session_with_short_request_timeout(
    test_name: &str,
) -> anyhow::Result<(ScopeId, DapClient, tempfile::TempDir)> {
    let config_dir = tempfile::tempdir()?;
    let secs = REQUEST_TIMEOUT.as_secs();
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("[request]\ntimeout_seconds = {secs}\nevaluate_timeout_seconds = {secs}\n"),
    )?;
    let scope_id = generate_test_scope_id(test_name);
    let dap_client =
        start_stopped_debug_session_with_config_dir(scope_id.clone(), config_dir.path())?;
    Ok((scope_id, dap_client, config_dir))
}

#[tokio::test]
async fn requests_after_a_timed_out_request() -> anyhow::Result<()> {
    let profile = real_adapter_profile()?;
    let (scope_id, mut dap_client, _config_dir) =
        start_session_with_short_request_timeout("request-timeout")?;

    let slow_started = Instant::now();
    let slow = run(
        &scope_id,
        &["eval", &slow_evaluate_expression(profile, SLOW_EVALUATE)],
    )
    .await?;
    assert!(
        slow.timed_out("evaluate"),
        "the slow evaluate should hit the request timeout, got {:?}",
        slow.result
    );
    assert!(
        slow.elapsed < REQUEST_TIMEOUT + OVERHEAD,
        "the timeout should fire at {REQUEST_TIMEOUT:?}, not wait for the adapter: took {:?}",
        slow.elapsed
    );
    let expected_cancel_note = match profile {
        RealAdapterProfile::Debugpy => "The adapter can't cancel requests",
        RealAdapterProfile::Lldb => "Asked the adapter to cancel it.",
    };
    assert!(
        matches!(&slow.result, Err(stderr) if stderr.contains(expected_cancel_note)),
        "the timeout error should say whether the request was cancelled \
         (expected {expected_cancel_note:?}), got {:?}",
        slow.result
    );

    // Sent while the adapter is still running the slow evaluate.
    let threads = run(&scope_id, &["threads"]).await?;
    let evaluate = run(&scope_id, &["eval", "1 + 1"]).await?;
    assert!(
        slow_started.elapsed() < SLOW_EVALUATE,
        "both follow-ups must be sent before the slow evaluate finishes"
    );
    match profile {
        RealAdapterProfile::Debugpy => {
            // A blocked threads would fail with a timeout, so success alone
            // shows it wasn't queued; `elapsed` also counts CLI startup.
            assert!(
                threads.result.is_ok(),
                "debugpy should answer threads while an evaluate runs, got {:?} after {:?}",
                threads.result,
                threads.elapsed
            );
            assert!(
                evaluate.timed_out("evaluate"),
                "debugpy runs evaluates one at a time, so the second should wait behind the \
                 first and time out, got {:?}",
                evaluate.result
            );
        }
        RealAdapterProfile::Lldb => {
            assert!(
                threads.timed_out("threads"),
                "lldb-dap handles one request at a time and can't interrupt a Python sleep, \
                 so threads should wait behind the slow evaluate and time out, got {:?}",
                threads.result
            );
            assert!(
                evaluate.timed_out("evaluate"),
                "lldb-dap should still be busy with the slow evaluate, got {:?}",
                evaluate.result
            );
        }
    }

    tokio::time::sleep((SLOW_EVALUATE + OVERHEAD).saturating_sub(slow_started.elapsed())).await;

    let threads = run(&scope_id, &["threads"]).await?;
    assert!(
        threads.result.is_ok(),
        "threads should work once the slow evaluate finishes, got {:?}",
        threads.result
    );
    let evaluate = run(&scope_id, &["eval", "1 + 1"]).await?;
    let result = evaluate.result.map_err(anyhow::Error::msg)?;
    assert!(
        result.contains('2'),
        "evaluate should get its own result, not a late response to an earlier request: {result}"
    );

    dap_client.kill()?;
    Ok(())
}
