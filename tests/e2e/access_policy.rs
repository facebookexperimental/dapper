// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::generate_test_scope_id;
use dapper_e2e_support::run_debug_command;
use dapper_e2e_support::start_stopped_debug_session_with_proxy_args;

#[tokio::test]
async fn a_proxy_refuses_what_its_allow_list_does_not_allow() -> anyhow::Result<()> {
    let scope_id = generate_test_scope_id("access-policy-refuses");
    let mut dap_client =
        start_stopped_debug_session_with_proxy_args(scope_id.clone(), &["--allow", "structure"])?;

    let threads = run_debug_command(Some(scope_id.clone()), &["threads"]).await?;
    assert!(
        threads.success,
        "structure allows listing threads; stderr: {}",
        threads.stderr
    );

    for (args, refusal) in [
        (&["eval", "1 + 1"][..], "`eval_repl` needs evaluate"),
        (&["variables", "1"], "`variables` needs read_values"),
        (
            &[
                "dap",
                "evaluate",
                "--arguments",
                r#"{"expression": "1 + 1"}"#,
            ],
            "`evaluate` needs evaluate",
        ),
        (&["stop"], "`stop` needs stop"),
    ] {
        let result = run_debug_command(Some(scope_id.clone()), args).await?;
        assert!(!result.success, "{args:?} should be refused");
        assert!(
            result.stderr.contains(refusal),
            "{args:?} should be refused with `{refusal}`; stderr: {}",
            result.stderr
        );
    }

    dap_client.kill()?;
    Ok(())
}
