// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::find_breakpoint_line_by_marker;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;
use dapper_e2e_support::test_source_path;

#[tokio::test]
async fn debug_cli_set_breakpoints_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-set-bp-json")?;

    let source_path = test_source_path()?;
    let bp_line = find_breakpoint_line_by_marker(&source_path, "breakpoint default_stop").await?;
    let source_path = source_path.to_string_lossy().into_owned();
    let bp_line_arg = bp_line.to_string();

    let result = run_debug_command(
        Some(scope_id),
        &[
            "set-breakpoints",
            &source_path,
            "-b",
            &bp_line_arg,
            "--json",
        ],
    )
    .await?;

    assert!(
        result.success,
        "set-breakpoints --json command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "set-breakpoints --json output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let result_json = &parsed["result"];
    let bp = &result_json["breakpoints"][0];
    assert_eq!(result_json["sourcePath"], source_path);
    assert_eq!(result_json["newCount"], 1);
    assert_eq!(result_json["existingCount"], 0);
    assert_eq!(bp["line"], bp_line);
    assert_eq!(bp["verified"], true);
    assert!(
        bp["id"].is_number(),
        "breakpoint ID should be numeric: {bp}"
    );

    dap_client.kill()?;
    Ok(())
}
