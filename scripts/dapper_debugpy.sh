#!/bin/bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Check if debugpy is installed
if ! python3 -c "import debugpy" 2>/dev/null; then
    echo "Error: debugpy not found"
    echo "Please install debugpy: pip install debugpy"
    exit 1
fi

cd "$PROJECT_ROOT"
cargo build

# Run debugpy as DAP adapter
exec "$PROJECT_ROOT/target/debug/dapper" proxy process python3 -m debugpy.adapter "$@"
