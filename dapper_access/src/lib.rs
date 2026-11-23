// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! What callers of a dapper session's control plane may do. Each call needs
//! some [`Operation`]s, and a [`Policy`] says which ones a session allows.

#![warn(clippy::all)]

mod operation;
mod policy;

pub use operation::Operation;
pub use operation::operation_names;
pub use policy::IncompletePolicy;
pub use policy::NotPermitted;
pub use policy::Policy;
