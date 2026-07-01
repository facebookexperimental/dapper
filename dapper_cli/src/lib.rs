// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#![warn(clippy::all)]

/// Child-session supervisor (Unix-only): spawns peer `dapper proxy from-config`
/// processes for headless `startDebugging` reverse requests.
#[cfg(unix)]
mod child_supervisor;
pub mod cli;
pub mod commands;
pub mod help;
pub mod program_name;

/// Default port for the control plane
const DAPPER_CONTROL_PLANE_PORT: u16 = 0;
