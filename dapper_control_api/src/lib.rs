// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

#![warn(clippy::all)]

mod grpc;
pub use grpc::ControlPlaneServer;
pub use grpc::DapperControlPlaneClient;
pub use grpc::resolve_unique_session;
pub use grpc::serve;

mod envelope;

mod response_context_output;

mod control_plane_result;
pub use control_plane_result::ControlPlaneResult;

pub mod render;
pub use render::render;
pub use render::render_json;
pub use render::render_plaintext;

mod rendered_response;
pub use rendered_response::RenderedResponse;

mod protocol;
pub use protocol::DapperControlPlane;
