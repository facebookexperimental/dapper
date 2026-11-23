// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! What callers of a dapper session's control plane may do. Each call needs
//! some [`Operation`]s.

#![warn(clippy::all)]

mod operation;

pub use operation::Operation;
pub use operation::operation_names;
