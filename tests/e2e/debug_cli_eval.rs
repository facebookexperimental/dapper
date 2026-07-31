// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

#[tokio::test]
async fn debug_cli_eval_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-eval-json")?;

    let result = run_debug_command(Some(scope_id), &["eval", "1 + 1", "--json"]).await?;
    assert!(result.success, "stderr: {}", result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout)?;
    let eval_result = parsed["result"]
        .as_str()
        .expect("result should be a string");
    assert_eq!(eval_result, "2");

    dap_client.kill()?;
    Ok(())
}
