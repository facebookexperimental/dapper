#!/bin/bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Find lldb-dap in PATH
LLDB_DAP=$(which lldb-dap || which lldb-vscode || echo "lldb-dap")

if ! command -v "$LLDB_DAP" &> /dev/null; then
    echo "Error: lldb-dap not found in PATH"
    echo "Please install LLDB with DAP support"
    exit 1
fi

cd "$PROJECT_ROOT"
cargo build

exec "$PROJECT_ROOT/target/debug/dapper" proxy process "$LLDB_DAP" "$@"
