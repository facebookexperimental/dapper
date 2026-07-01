// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#![warn(clippy::all)]

pub mod capabilities;
pub mod data_types;
pub mod display;
pub mod enums;
pub mod events;
pub mod protocol;
pub mod requests;
pub mod responses;

#[cfg(test)]
mod tests;
