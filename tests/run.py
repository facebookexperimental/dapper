#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.
# pyre-strict

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import cast


REPOSITORY_ROOT: Path = Path(__file__).resolve().parent.parent
TEST_MANIFEST: Path = REPOSITORY_ROOT / "tests" / "Cargo.toml"
DEBUGPY_FIXTURE: Path = REPOSITORY_ROOT / "tests" / "fixtures" / "python_example.py"
LLDB_FIXTURE: Path = REPOSITORY_ROOT / "tests" / "fixtures" / "cpp_example.cpp"
DEBUGPY_SETUP_COMMAND: str = (
    "uv run --project tests/profiles/debugpy --frozen python tests/run.py "
    "--test launch --adapter debugpy"
)
LLDB_SETUP_COMMAND: str = (
    "python3 tests/run.py --test launch --adapter lldb "
    "--lldb-dap /path/to/lldb-dap --cxx /path/to/clang++"
)


class AdapterProfile(Enum):
    NONE = "none"
    FAKE = "fake"
    DEBUGPY = "debugpy"
    LLDB = "lldb"


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
    adapter_profile: AdapterProfile | None
    debugpy_python: Path | None
    debugpy_adapter: Path | None
    lldb_dap: Path | None
    cxx: Path | None
    required_profiles: frozenset[AdapterProfile]


@dataclass(frozen=True)
class ResolvedProfiles:
    test_specs: tuple[TestSpec, ...]
    debugpy_adapter: AdapterCommand | None
    lldb_adapter: AdapterCommand | None
    lldb_compiler: Path | None


TEST_SPECS: tuple[TestSpec, ...] = (
    TestSpec("debug_cli_dap", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_dap", AdapterProfile.LLDB),
    TestSpec("debug_cli_reverse_navigate", AdapterProfile.FAKE),
    TestSpec("debug_cli_eval", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_scopes", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_scopes", AdapterProfile.LLDB),
    TestSpec("debug_cli_sessions", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_sessions", AdapterProfile.LLDB),
    TestSpec("debug_cli_set_variable", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_set_variable", AdapterProfile.LLDB),
    TestSpec("debug_cli_stack_trace", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_stack_trace", AdapterProfile.LLDB),
    TestSpec("debug_cli_status", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_status", AdapterProfile.LLDB),
    TestSpec("debug_cli_stop", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_stop", AdapterProfile.LLDB),
    TestSpec("debug_cli_threads", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_threads", AdapterProfile.LLDB),
    TestSpec("debug_cli_variable_inspection", AdapterProfile.DEBUGPY),
    TestSpec("debug_cli_variable_inspection", AdapterProfile.LLDB),
    TestSpec("error_recovery", AdapterProfile.DEBUGPY),
    TestSpec("error_recovery", AdapterProfile.LLDB),
    TestSpec("headless_child_session", AdapterProfile.FAKE),
    TestSpec("help_topics", AdapterProfile.NONE),
    TestSpec("launch", AdapterProfile.DEBUGPY),
    TestSpec("launch", AdapterProfile.LLDB),
    TestSpec("mcp_error_paths", AdapterProfile.DEBUGPY),
    TestSpec("mcp_error_paths", AdapterProfile.LLDB),
    TestSpec("mcp_list_tools", AdapterProfile.NONE),
    TestSpec("mcp_session_fallback", AdapterProfile.DEBUGPY),
    TestSpec("mcp_session_fallback", AdapterProfile.LLDB),
    TestSpec("mcp_server_info", AdapterProfile.NONE),
    TestSpec("mcp_tool_dap_request", AdapterProfile.DEBUGPY),
    TestSpec("mcp_tool_dap_request", AdapterProfile.LLDB),
    TestSpec("mcp_tool_evaluate", AdapterProfile.DEBUGPY),
    TestSpec("mcp_tool_evaluate", AdapterProfile.LLDB),
    TestSpec("mcp_tool_memory", AdapterProfile.LLDB),
    TestSpec("mcp_tool_reverse_navigate", AdapterProfile.FAKE),
    TestSpec("mcp_tool_scopes", AdapterProfile.DEBUGPY),
    TestSpec("mcp_tool_scopes", AdapterProfile.LLDB),
    TestSpec("mcp_tool_stack_trace", AdapterProfile.DEBUGPY),
    TestSpec("mcp_tool_stack_trace", AdapterProfile.LLDB),
    TestSpec("mcp_tool_threads", AdapterProfile.DEBUGPY),
    TestSpec("mcp_tool_threads", AdapterProfile.LLDB),
    TestSpec("proxy_from_config", AdapterProfile.DEBUGPY),
    TestSpec("proxy_from_config", AdapterProfile.LLDB),
    TestSpec("proxy_response_filtering", AdapterProfile.DEBUGPY),
    TestSpec("proxy_response_filtering", AdapterProfile.LLDB),
    TestSpec("session_scoping", AdapterProfile.DEBUGPY),
    TestSpec("session_scoping", AdapterProfile.LLDB),
    TestSpec("variable_inspection", AdapterProfile.DEBUGPY),
    TestSpec("variable_inspection", AdapterProfile.LLDB),
)
TEST_NAMES: tuple[str, ...] = tuple(dict.fromkeys(spec.name for spec in TEST_SPECS))


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


def _resolve_command(
    configured: Path | None,
    environment_variable: str,
    candidates: tuple[str, ...],
    description: str,
) -> tuple[Path | None, str | None]:
    if configured is None:
        environment_path = os.environ.get(environment_variable)
        configured = Path(environment_path) if environment_path else None
    if configured is not None:
        discovered = shutil.which(str(configured))
        try:
            return _validate_executable(
                Path(discovered) if discovered else configured, description
            ), None
        except RuntimeError as error:
            return None, str(error)

    for candidate in candidates:
        discovered = shutil.which(candidate)
        if discovered is not None:
            return _validate_executable(Path(discovered), description), None
    return None, f"{description} was not found on PATH"


def _resolve_lldb_adapter(
    options: RunnerOptions,
) -> tuple[AdapterCommand | None, str | None]:
    executable, failure = _resolve_command(
        options.lldb_dap,
        "DAPPER_TEST_LLDB_DAP",
        ("lldb-dap", "lldb-vscode"),
        "the LLDB DAP adapter",
    )
    return (
        (AdapterCommand(executable), None)
        if executable is not None
        else (None, failure)
    )


def _resolve_lldb_compiler(options: RunnerOptions) -> tuple[Path | None, str | None]:
    return _resolve_command(
        options.cxx,
        "DAPPER_TEST_CXX",
        ("clang++", "c++", "g++"),
        "the C++ compiler",
    )


def _compile_lldb_fixture(compiler: Path, output_directory: Path) -> Path:
    output = output_directory / (
        "cpp_example.exe" if os.name == "nt" else "cpp_example"
    )
    command = [
        str(compiler),
        "-std=c++17",
        "-g",
        "-O0",
        str(LLDB_FIXTURE.resolve(strict=True)),
        "-o",
        str(output),
    ]
    # @lint-ignore FIXIT1 NoUnsafeExecRule
    subprocess.run(command, cwd=REPOSITORY_ROOT, check=True)
    return _validate_executable(output, "the compiled LLDB test fixture")


def _run_test(
    test_spec: TestSpec,
    dapper: Path,
    adapter: AdapterCommand | None,
    debuggee: Path | None,
) -> int:
    environment = os.environ.copy()
    environment["DAPPER_TEST_EXECUTABLE"] = str(dapper)
    environment.pop("DAPPER_TEST_ADAPTER_EXECUTABLE", None)
    environment.pop("DAPPER_TEST_ADAPTER_ARGUMENTS", None)
    environment.pop("DAPPER_TEST_ADAPTER_PROFILE", None)
    environment.pop("DAPPER_TEST_DEBUGGEE", None)
    environment.pop("DAPPER_TEST_DEBUGGEE_DEBUGINFO", None)
    environment.pop("DAPPER_TEST_LAUNCH_ARGUMENT_OVERRIDES", None)
    environment.pop("DAPPER_TEST_SOURCE", None)
    if adapter is not None:
        environment["DAPPER_TEST_ADAPTER_EXECUTABLE"] = str(adapter.executable)
        environment["DAPPER_TEST_ADAPTER_ARGUMENTS"] = json.dumps(adapter.arguments)
    if test_spec.profile in (AdapterProfile.DEBUGPY, AdapterProfile.LLDB):
        environment["DAPPER_TEST_ADAPTER_PROFILE"] = test_spec.profile.value
    if debuggee is not None:
        environment["DAPPER_TEST_DEBUGGEE"] = str(debuggee)
    if test_spec.profile is AdapterProfile.DEBUGPY:
        environment["DAPPER_TEST_SOURCE"] = str(DEBUGPY_FIXTURE.resolve(strict=True))
        environment["DAPPER_TEST_LAUNCH_ARGUMENT_OVERRIDES"] = json.dumps(
            {"type": "debugpy"}
        )
    elif test_spec.profile is AdapterProfile.LLDB:
        environment["DAPPER_TEST_SOURCE"] = str(LLDB_FIXTURE.resolve(strict=True))
        environment["DAPPER_TEST_LAUNCH_ARGUMENT_OVERRIDES"] = json.dumps(
            {
                "type": "lldb-dap",
                "stopOnEntry": False,
                "initCommands": ["breakpoint set --name sum_values"],
            }
        )
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
    parser.add_argument(
        "--adapter",
        choices=(AdapterProfile.DEBUGPY.value, AdapterProfile.LLDB.value),
        help="Run only variants for this external adapter",
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
        "--lldb-dap",
        type=Path,
        metavar="PATH",
        help="Existing lldb-dap executable",
    )
    parser.add_argument(
        "--cxx",
        type=Path,
        metavar="PATH",
        help="C++ compiler used to build LLDB fixtures",
    )
    parser.add_argument(
        "--require-profile",
        action="append",
        choices=(AdapterProfile.DEBUGPY.value, AdapterProfile.LLDB.value),
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
        adapter_profile=(
            AdapterProfile(cast(str, arguments.adapter))
            if arguments.adapter is not None
            else None
        ),
        debugpy_python=cast(Path | None, arguments.debugpy_python),
        debugpy_adapter=cast(Path | None, arguments.debugpy_adapter),
        lldb_dap=cast(Path | None, arguments.lldb_dap),
        cxx=cast(Path | None, arguments.cxx),
        required_profiles=required_profiles,
    )


def _selected_tests(options: RunnerOptions) -> tuple[TestSpec, ...]:
    return tuple(
        spec
        for spec in TEST_SPECS
        if (options.test_name is None or spec.name == options.test_name)
        and (options.adapter_profile is None or spec.profile is options.adapter_profile)
    )


def _profile_is_required(profile: AdapterProfile, options: RunnerOptions) -> bool:
    return (
        options.test_name is not None
        or options.adapter_profile is profile
        or profile in options.required_profiles
        or (
            profile is AdapterProfile.DEBUGPY
            and (
                options.debugpy_python is not None
                or options.debugpy_adapter is not None
                or "DAPPER_TEST_DEBUGPY_PYTHON" in os.environ
            )
        )
        or (
            profile is AdapterProfile.LLDB
            and (
                options.lldb_dap is not None
                or options.cxx is not None
                or "DAPPER_TEST_LLDB_DAP" in os.environ
                or "DAPPER_TEST_CXX" in os.environ
            )
        )
    )


def _skip_unavailable_profile(
    test_specs: tuple[TestSpec, ...],
    profile: AdapterProfile,
    reason: str | None,
    setup_command: str,
    options: RunnerOptions,
) -> tuple[TestSpec, ...]:
    if _profile_is_required(profile, options):
        raise RuntimeError(f"{reason}\nMake it available with:\n  {setup_command}")
    print(
        f"\n==> {profile.value} tests skipped: {reason}\n"
        f"    Make them available with: {setup_command}",
        file=sys.stderr,
    )
    return tuple(spec for spec in test_specs if spec.profile is not profile)


def _resolve_profiles(
    options: RunnerOptions, test_specs: tuple[TestSpec, ...]
) -> ResolvedProfiles:
    debugpy_adapter: AdapterCommand | None = None
    if any(spec.profile is AdapterProfile.DEBUGPY for spec in test_specs):
        debugpy_adapter, reason = _resolve_debugpy_adapter(options)
        if debugpy_adapter is None:
            test_specs = _skip_unavailable_profile(
                test_specs,
                AdapterProfile.DEBUGPY,
                reason,
                DEBUGPY_SETUP_COMMAND,
                options,
            )

    lldb_adapter: AdapterCommand | None = None
    lldb_compiler: Path | None = None
    if any(spec.profile is AdapterProfile.LLDB for spec in test_specs):
        lldb_adapter, reason = _resolve_lldb_adapter(options)
        if lldb_adapter is not None:
            lldb_compiler, reason = _resolve_lldb_compiler(options)
        if lldb_adapter is None or lldb_compiler is None:
            test_specs = _skip_unavailable_profile(
                test_specs, AdapterProfile.LLDB, reason, LLDB_SETUP_COMMAND, options
            )

    return ResolvedProfiles(test_specs, debugpy_adapter, lldb_adapter, lldb_compiler)


def _run_selected_tests(profiles: ResolvedProfiles, dapper: Path) -> int:
    fake_adapter = (
        AdapterCommand(_build_fake_adapter())
        if any(spec.profile is AdapterProfile.FAKE for spec in profiles.test_specs)
        else None
    )
    with tempfile.TemporaryDirectory(prefix="dapper-e2e-lldb-") as temporary_directory:
        lldb_debuggee = (
            _compile_lldb_fixture(
                cast(Path, profiles.lldb_compiler), Path(temporary_directory)
            )
            if any(spec.profile is AdapterProfile.LLDB for spec in profiles.test_specs)
            else None
        )
        return_code = 0
        for test_spec in profiles.test_specs:
            adapter = None
            debuggee = None
            if test_spec.profile is AdapterProfile.FAKE:
                adapter = fake_adapter
            elif test_spec.profile is AdapterProfile.DEBUGPY:
                adapter = profiles.debugpy_adapter
                debuggee = DEBUGPY_FIXTURE.resolve(strict=True)
            elif test_spec.profile is AdapterProfile.LLDB:
                adapter = profiles.lldb_adapter
                debuggee = lldb_debuggee
            print(
                f"\n==> {test_spec.name} ({test_spec.profile.value})", file=sys.stderr
            )
            test_return_code = _run_test(test_spec, dapper, adapter, debuggee)
            if test_return_code != 0:
                return_code = test_return_code
        return return_code


def main() -> int:
    options = _parse_options()
    test_specs = _selected_tests(options)
    try:
        if not test_specs:
            raise RuntimeError("no tests match the requested test and adapter")
        profiles = _resolve_profiles(options, test_specs)
        return _run_selected_tests(profiles, _build_dapper())
    except subprocess.CalledProcessError as error:
        return error.returncode
    except (OSError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
