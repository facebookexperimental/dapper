// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

#[tokio::test]
async fn debug_cli_stop() -> anyhow::Result<()> {
    let (scope_id, _dap_client) = setup_stopped_debug_session("debug-cli-stop")?;

    let stop_result = run_debug_command(Some(scope_id.clone()), &["stop"]).await?;

    assert!(
        stop_result.success,
        "stop command should succeed, stderr: {}",
        stop_result.stderr
    );

    let threads_result = run_debug_command(Some(scope_id), &["threads"]).await?;

    assert!(
        !threads_result.success,
        "threads command after stop should fail, stdout: {}, stderr: {}",
        threads_result.stdout, threads_result.stderr
    );

    Ok(())
}
