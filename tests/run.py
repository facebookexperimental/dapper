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
TEST_NAMES: tuple[str, ...] = (
    "debug_cli_reverse_navigate",
    "headless_child_session",
    "help_topics",
    "mcp_server_info",
    "mcp_tool_reverse_navigate",
)
FAKE_ADAPTER_TESTS: frozenset[str] = frozenset(
    {
        "debug_cli_reverse_navigate",
        "headless_child_session",
        "mcp_tool_reverse_navigate",
    }
)


def _render_compiler_message(message: dict[object, object]) -> None:
    if message.get("reason") != "compiler-message":
        return
    compiler_message = message.get("message")
    if not isinstance(compiler_message, dict):
        return
    rendered = compiler_message.get("rendered")
    if isinstance(rendered, str):
        sys.stderr.write(rendered)


def _artifact_executable(
    message: dict[object, object], executable_name: str
) -> Path | None:
    if message.get("reason") != "compiler-artifact":
        return None
    target = message.get("target")
    if not isinstance(target, dict) or target.get("name") != executable_name:
        return None
    executable = message.get("executable")
    return Path(executable) if isinstance(executable, str) else None


def _build_executable(command: list[str], executable_name: str) -> Path:
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
        artifact = _artifact_executable(decoded, executable_name)
        if artifact is not None:
            executable = artifact

    return_code = process.wait()
    if return_code != 0:
        raise subprocess.CalledProcessError(return_code, command)
    if executable is None:
        raise RuntimeError(
            f"Cargo did not report the `{executable_name}` executable artifact"
        )
    return executable.resolve(strict=True)


def _build_dapper() -> Path:
    return _build_executable(
        [
            "cargo",
            "build",
            "--package",
            "dapper_cli",
            "--bin",
            "dapper",
            "--message-format=json-render-diagnostics",
        ],
        "dapper",
    )


def _build_fake_adapter() -> Path:
    return _build_executable(
        [
            "cargo",
            "build",
            "--manifest-path",
            str(TEST_MANIFEST),
            "--features",
            "e2e-tests",
            "--bin",
            "fake_dap_adapter",
            "--message-format=json-render-diagnostics",
        ],
        "fake_dap_adapter",
    )


def _run_test(test_name: str, dapper: Path, adapter: Path | None) -> int:
    environment = os.environ.copy()
    environment["DAPPER_TEST_EXECUTABLE"] = str(dapper)
    if adapter is not None:
        environment["DAPPER_TEST_ADAPTER_EXECUTABLE"] = str(adapter)
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


def _parse_test_name() -> str | None:
    parser = argparse.ArgumentParser(description="Run Dapper end-to-end tests")
    parser.add_argument(
        "--test",
        choices=TEST_NAMES,
        help="Run only this E2E test target",
    )
    return cast(str | None, parser.parse_args().test)


def main() -> int:
    selected_test = _parse_test_name()
    test_names = (selected_test,) if selected_test is not None else TEST_NAMES
    try:
        dapper = _build_dapper()
        adapter = (
            _build_fake_adapter()
            if any(test_name in FAKE_ADAPTER_TESTS for test_name in test_names)
            else None
        )
        return_code = 0
        for test_name in test_names:
            print(f"\n==> {test_name}", file=sys.stderr)
            test_return_code = _run_test(test_name, dapper, adapter)
            if test_return_code != 0:
                return_code = test_return_code
        return return_code
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (OSError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
