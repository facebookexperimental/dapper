// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use anyhow::Context;
use dapper_e2e_support::get_frame_ids;
use dapper_e2e_support::get_thread_ids;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::setup_stopped_debug_session;
use serde::Deserialize;

#[derive(Deserialize)]
struct ScopeResult {
    name: String,
    #[serde(rename = "variablesReference")]
    variables_reference: i64,
    #[serde(rename = "expensive")]
    _expensive: bool,
    #[serde(rename = "presentationHint")]
    _presentation_hint: Option<String>,
    #[serde(rename = "namedVariables")]
    _named_variables: Option<i64>,
    #[serde(rename = "indexedVariables")]
    _indexed_variables: Option<i64>,
    #[serde(rename = "source")]
    _source: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(rename = "line")]
    _line: Option<i64>,
    #[serde(rename = "column")]
    _column: Option<i64>,
    #[serde(rename = "endLine")]
    _end_line: Option<i64>,
    #[serde(rename = "endColumn")]
    _end_column: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopesResult {
    scopes: Vec<ScopeResult>,
    #[serde(rename = "locals")]
    _locals: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
    #[serde(rename = "frameId")]
    frame_id: i64,
}

#[tokio::test]
async fn debug_cli_scopes_json() -> anyhow::Result<()> {
    let (scope_id, mut dap_client) = setup_stopped_debug_session("debug-cli-scopes-json")?;

    let thread_id = get_thread_ids(&scope_id).await?[0];
    let frame_id = get_frame_ids(&scope_id, thread_id).await?[0];

    let result =
        run_debug_command(Some(scope_id), &["--json", "scopes", &frame_id.to_string()]).await?;

    assert!(
        result.success,
        "scopes --json command should succeed, stderr: {}",
        result.stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap_or_else(|e| {
        panic!(
            "scopes --json output should be valid JSON, got error: {e}, stdout: {}",
            result.stdout
        )
    });

    let scopes_result: ScopesResult = serde_json::from_value(parsed["result"].clone())
        .context("JSON result should match the scopes response shape")?;
    assert_eq!(scopes_result.frame_id, frame_id);
    assert!(
        !scopes_result.scopes.is_empty(),
        "scopes should not be empty for a valid frame"
    );
    assert!(
        scopes_result
            .scopes
            .iter()
            .all(|scope| !scope.name.is_empty() && scope.variables_reference >= 0),
        "each scope should have a name and nonnegative variables reference"
    );

    dap_client.kill()?;
    Ok(())
}
