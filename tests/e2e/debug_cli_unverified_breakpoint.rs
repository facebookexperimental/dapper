// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Unverified breakpoints carry the adapter's reason. Meta's debugpy treats
//! any path under `third-party/` as library code, so with the default
//! `justMyCode` a breakpoint there is rejected with an explanation.

use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;

const EXCLUDED_REASON: &str = "Breakpoint in file excluded by filters.";

fn library_file(dir: &tempfile::TempDir) -> anyhow::Result<String> {
    let library_dir = dir.path().join("third-party");
    std::fs::create_dir(&library_dir)?;
    let path = library_dir.join("library.py");
    std::fs::write(&path, "def answer():\n    return 42\n")?;
    Ok(path.to_string_lossy().into_owned())
}

#[tokio::test]
async fn unverified_breakpoint_shows_the_adapter_reason() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("unverified-bp-reason")?;
    let dir = tempfile::tempdir()?;
    let path = library_file(&dir)?;

    let json = run_debug_command(
        Some(scope_id.clone()),
        &["set-breakpoints", &path, "-b", "2", "--json"],
    )
    .await?;
    assert!(
        json.success,
        "set-breakpoints should succeed: {}",
        json.stderr
    );
    let parsed: serde_json::Value = serde_json::from_str(&json.stdout)?;
    let breakpoint = &parsed["result"]["breakpoints"][0];
    assert_eq!(breakpoint["verified"], false, "{breakpoint}");
    let message = breakpoint["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an unverified breakpoint should carry a message: {breakpoint}"));
    assert!(message.starts_with(EXCLUDED_REASON), "{message}");
    assert!(
        message.contains("justMyCode"),
        "the reason should name the setting to change: {message}"
    );

    let text = run_debug_command(
        Some(scope_id),
        &["set-breakpoints", "--clear-existing", &path, "-b", "2"],
    )
    .await?;
    assert!(
        text.success,
        "set-breakpoints should succeed: {}",
        text.stderr
    );
    assert!(
        text.stdout
            .contains(&format!("  Not Verified: Line 2\n    {EXCLUDED_REASON}\n")),
        "the reason should appear under the breakpoint:\n{}",
        text.stdout
    );

    dap_client.kill()?;
    Ok(())
}
