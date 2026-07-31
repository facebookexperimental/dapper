// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_e2e_support::create_mcp_client;

#[tokio::test]
async fn server_advertises_capabilities() -> anyhow::Result<()> {
    let mcp_client = create_mcp_client(None).await?;

    let server_info = mcp_client.peer_info().expect("MCP peer info should be set");

    assert!(server_info.capabilities.tools.is_some());
    assert!(server_info.instructions.is_some());
    assert!(
        server_info
            .instructions
            .as_ref()
            .is_some_and(|instructions| instructions.contains("DAP proxy"))
    );

    mcp_client.cancel().await?;
    Ok(())
}
