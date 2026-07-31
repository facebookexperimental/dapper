// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

fn run_dapper(args: &[&str]) -> Result<String> {
    let binary = std::env::var("DAPPER_TEST_EXECUTABLE")
        .context("DAPPER_TEST_EXECUTABLE must name the Dapper binary")?;
    let output = Command::new(&binary)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{binary}` with {args:?}"))?;

    if !output.status.success() {
        bail!(
            "`{binary}` with {args:?} exited {}; stdout={:?}; stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn debug_topic_includes_example_outputs() -> Result<()> {
    // The debug topic embeds rendered example outputs alongside the
    // command examples so an agent can pattern-match without a live
    // session. These fragments come straight from the renderers and
    // would drift if a Display impl changed without updating the topic.
    let out = run_dapper(&["help", "debug"])?;
    assert!(
        out.contains("Threads:"),
        "debug topic should embed `dapper debug threads` example output; got:\n{out}"
    );
    assert!(
        out.contains("Stack trace (frames"),
        "debug topic should embed `dapper debug stack-trace` example output; got:\n{out}"
    );
    assert!(
        out.contains("Variables for reference"),
        "debug topic should embed `dapper debug variables` example output; got:\n{out}"
    );
    assert!(
        out.contains("Set 2 breakpoints"),
        "debug topic should embed `dapper debug set-breakpoints` example output; got:\n{out}"
    );
    Ok(())
}
