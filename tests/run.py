#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.
# pyre-strict

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import cast


REPOSITORY_ROOT: Path = Path(__file__).resolve().parent.parent
TEST_MANIFEST: Path = REPOSITORY_ROOT / "tests" / "Cargo.toml"
TEST_NAMES: tuple[str, ...] = ("help_topics", "mcp_server_info")


def _render_compiler_message(message: dict[object, object]) -> None:
    if message.get("reason") != "compiler-message":
        return
    compiler_message = message.get("message")
    if not isinstance(compiler_message, dict):
        return
    rendered = compiler_message.get("rendered")
    if isinstance(rendered, str):
        sys.stderr.write(rendered)


def _artifact_executable(message: dict[object, object]) -> Path | None:
    if message.get("reason") != "compiler-artifact":
        return None
    target = message.get("target")
    if not isinstance(target, dict) or target.get("name") != "dapper":
        return None
    executable = message.get("executable")
    return Path(executable) if isinstance(executable, str) else None


def _build_dapper() -> Path:
    command = [
        "cargo",
        "build",
        "--package",
        "dapper_cli",
        "--bin",
        "dapper",
        "--message-format=json-render-diagnostics",
    ]
    # @lint-ignore FIXIT1 NoUnsafeExecRule
    process = subprocess.Popen(
        command,
        cwd=REPOSITORY_ROOT,
        stdout=subprocess.PIPE,
        text=True,
    )
    if process.stdout is None:
        process.kill()
        raise RuntimeError("Cargo build stdout was not captured")

    executable: Path | None = None
    for line in process.stdout:
        try:
            decoded: object = json.loads(line)
        except json.JSONDecodeError:
            sys.stderr.write(line)
            continue
        if not isinstance(decoded, dict):
            continue
        _render_compiler_message(decoded)
        artifact = _artifact_executable(decoded)
        if artifact is not None:
            executable = artifact

    return_code = process.wait()
    if return_code != 0:
        raise subprocess.CalledProcessError(return_code, command)
    if executable is None:
        raise RuntimeError("Cargo did not report the `dapper` executable artifact")
    return executable.resolve(strict=True)


def _run_test(test_name: str, dapper: Path) -> int:
    environment = os.environ.copy()
    environment["DAPPER_TEST_EXECUTABLE"] = str(dapper)
    command = [
        "cargo",
        "test",
        "--manifest-path",
        str(TEST_MANIFEST),
        "--features",
        "e2e-tests",
        "--test",
        test_name,
    ]
    # @lint-ignore FIXIT1 NoUnsafeExecRule
    return subprocess.run(
        command,
        cwd=REPOSITORY_ROOT,
        env=environment,
        check=False,
    ).returncode


def _parse_test_name() -> str:
    parser = argparse.ArgumentParser(description="Run Dapper end-to-end tests")
    parser.add_argument(
        "--test",
        choices=TEST_NAMES,
        default="help_topics",
        help="E2E test target to run",
    )
    return cast(str, parser.parse_args().test)


def main() -> int:
    test_name = _parse_test_name()
    try:
        dapper = _build_dapper()
        return _run_test(test_name, dapper)
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (OSError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
