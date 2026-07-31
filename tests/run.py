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
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import cast


REPOSITORY_ROOT: Path = Path(__file__).resolve().parent.parent
TEST_MANIFEST: Path = REPOSITORY_ROOT / "tests" / "Cargo.toml"
DEBUGPY_FIXTURE: Path = REPOSITORY_ROOT / "tests" / "fixtures" / "python_example.py"
DEBUGPY_SETUP_COMMAND: str = (
    "uv run --project tests/profiles/debugpy --frozen python tests/run.py --test launch"
)


class AdapterProfile(Enum):
    NONE = "none"
    FAKE = "fake"
    DEBUGPY = "debugpy"


@dataclass(frozen=True)
class TestSpec:
    name: str
    profile: AdapterProfile


@dataclass(frozen=True)
class AdapterCommand:
    executable: Path
    arguments: tuple[str, ...] = ()


@dataclass(frozen=True)
class RunnerOptions:
    test_name: str | None
    debugpy_python: Path | None
    debugpy_adapter: Path | None
    required_profiles: frozenset[AdapterProfile]


TEST_SPECS: tuple[TestSpec, ...] = (
    TestSpec("debug_cli_reverse_navigate", AdapterProfile.FAKE),
    TestSpec("headless_child_session", AdapterProfile.FAKE),
    TestSpec("help_topics", AdapterProfile.NONE),
    TestSpec("launch", AdapterProfile.DEBUGPY),
    TestSpec("mcp_list_tools", AdapterProfile.NONE),
    TestSpec("mcp_server_info", AdapterProfile.NONE),
    TestSpec("mcp_tool_reverse_navigate", AdapterProfile.FAKE),
    TestSpec("proxy_from_config", AdapterProfile.DEBUGPY),
)
TEST_NAMES: tuple[str, ...] = tuple(spec.name for spec in TEST_SPECS)


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


def _validate_executable(path: Path, description: str) -> Path:
    executable = path.expanduser().absolute()
    if not executable.exists():
        raise RuntimeError(f"{description} does not exist: {path}")
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise RuntimeError(f"{description} is not executable: {executable}")
    return executable


def _check_debugpy(python: Path) -> str | None:
    try:
        # @lint-ignore FIXIT1 NoUnsafeExecRule
        process = subprocess.run(
            [
                str(python),
                "-c",
                "import debugpy, debugpy.adapter; print(debugpy.__version__)",
            ],
            cwd=REPOSITORY_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return str(error)
    if process.returncode == 0:
        return None
    stderr_lines = process.stderr.strip().splitlines()
    return (
        stderr_lines[-1]
        if stderr_lines
        else f"Python exited with status {process.returncode}"
    )


def _resolve_debugpy_adapter(
    options: RunnerOptions,
) -> tuple[AdapterCommand | None, str | None]:
    if options.debugpy_adapter is not None:
        executable = _validate_executable(
            options.debugpy_adapter, "the debugpy adapter executable"
        )
        return AdapterCommand(executable), None

    configured_python = options.debugpy_python
    if configured_python is None:
        environment_python = os.environ.get("DAPPER_TEST_DEBUGPY_PYTHON")
        configured_python = Path(environment_python) if environment_python else None
    python = _validate_executable(
        configured_python or Path(sys.executable), "the debugpy Python interpreter"
    )
    failure = _check_debugpy(python)
    if failure is not None:
        return None, f"debugpy is not importable by {python}: {failure}"
    return AdapterCommand(python, ("-m", "debugpy.adapter")), None


def _run_test(
    test_spec: TestSpec,
    dapper: Path,
    adapter: AdapterCommand | None,
) -> int:
    environment = os.environ.copy()
    environment["DAPPER_TEST_EXECUTABLE"] = str(dapper)
    environment.pop("DAPPER_TEST_ADAPTER_EXECUTABLE", None)
    environment.pop("DAPPER_TEST_ADAPTER_ARGUMENTS", None)
    environment.pop("DAPPER_TEST_DEBUGGEE", None)
    if adapter is not None:
        environment["DAPPER_TEST_ADAPTER_EXECUTABLE"] = str(adapter.executable)
        environment["DAPPER_TEST_ADAPTER_ARGUMENTS"] = json.dumps(adapter.arguments)
    if test_spec.profile is AdapterProfile.DEBUGPY:
        environment["DAPPER_TEST_DEBUGGEE"] = str(DEBUGPY_FIXTURE.resolve(strict=True))
    command = [
        "cargo",
        "test",
        "--manifest-path",
        str(TEST_MANIFEST),
        "--features",
        "e2e-tests",
        "--test",
        test_spec.name,
    ]
    # @lint-ignore FIXIT1 NoUnsafeExecRule
    return subprocess.run(
        command,
        cwd=REPOSITORY_ROOT,
        env=environment,
        check=False,
    ).returncode


def _parse_options() -> RunnerOptions:
    parser = argparse.ArgumentParser(description="Run Dapper end-to-end tests")
    parser.add_argument(
        "--test",
        choices=TEST_NAMES,
        help="Run only this E2E test target",
    )
    debugpy_source = parser.add_mutually_exclusive_group()
    debugpy_source.add_argument(
        "--debugpy-python",
        type=Path,
        metavar="PATH",
        help="Python interpreter that provides the debugpy module",
    )
    debugpy_source.add_argument(
        "--debugpy-adapter",
        type=Path,
        metavar="PATH",
        help="Prebuilt debugpy DAP adapter executable",
    )
    parser.add_argument(
        "--require-profile",
        action="append",
        choices=(AdapterProfile.DEBUGPY.value,),
        default=[],
        help="Fail rather than skip when this adapter profile is unavailable",
    )
    arguments = parser.parse_args()
    required_profiles = frozenset(
        AdapterProfile(profile)
        for profile in cast(list[str], arguments.require_profile)
    )
    return RunnerOptions(
        test_name=cast(str | None, arguments.test),
        debugpy_python=cast(Path | None, arguments.debugpy_python),
        debugpy_adapter=cast(Path | None, arguments.debugpy_adapter),
        required_profiles=required_profiles,
    )


def _selected_tests(test_name: str | None) -> tuple[TestSpec, ...]:
    if test_name is None:
        return TEST_SPECS
    return tuple(spec for spec in TEST_SPECS if spec.name == test_name)


def main() -> int:
    options = _parse_options()
    test_specs = _selected_tests(options.test_name)
    try:
        debugpy_adapter: AdapterCommand | None = None
        if any(spec.profile is AdapterProfile.DEBUGPY for spec in test_specs):
            debugpy_adapter, unavailable_reason = _resolve_debugpy_adapter(options)
            debugpy_required = (
                options.test_name is not None
                or AdapterProfile.DEBUGPY in options.required_profiles
                or options.debugpy_python is not None
                or options.debugpy_adapter is not None
                or "DAPPER_TEST_DEBUGPY_PYTHON" in os.environ
            )
            if debugpy_adapter is None and debugpy_required:
                raise RuntimeError(
                    f"{unavailable_reason}\nProvision it with:\n  {DEBUGPY_SETUP_COMMAND}"
                )
            if debugpy_adapter is None:
                print(
                    f"\n==> debugpy tests skipped: {unavailable_reason}\n"
                    f"    Provision them with: {DEBUGPY_SETUP_COMMAND}",
                    file=sys.stderr,
                )
                test_specs = tuple(
                    spec
                    for spec in test_specs
                    if spec.profile is not AdapterProfile.DEBUGPY
                )

        dapper = _build_dapper()
        fake_adapter = (
            AdapterCommand(_build_fake_adapter())
            if any(spec.profile is AdapterProfile.FAKE for spec in test_specs)
            else None
        )
        return_code = 0
        for test_spec in test_specs:
            adapter = None
            if test_spec.profile is AdapterProfile.FAKE:
                adapter = fake_adapter
            elif test_spec.profile is AdapterProfile.DEBUGPY:
                adapter = debugpy_adapter
            print(f"\n==> {test_spec.name}", file=sys.stderr)
            test_return_code = _run_test(test_spec, dapper, adapter)
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
