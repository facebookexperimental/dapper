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

mod breakpoint_info;
pub use breakpoint_info::BreakpointInfo;

mod exception_filter_entry;
pub use exception_filter_entry::ExceptionFilterEntry;

mod output_event;
pub use output_event::BufferedOutput;
pub use output_event::OutputEvent;

mod envelope;

mod response_context_output;

mod execution_state_summary;
pub use execution_state_summary::ExecutionStateSummary;
pub use execution_state_summary::ExecutionStatus;
pub use execution_state_summary::VersionedExecutionStateSummary;

mod response_context;
pub use response_context::ResponseContext;

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

mod navigation_type;
pub use navigation_type::NavigationType;

pub mod response;
pub use response::NavigateResult;
pub use response::NavigationResult;
pub use response::RawDapResult;
pub use response::ScopesResult;
pub use response::SessionsResult;
pub use response::SetBreakpointsResult;
pub use response::SetExceptionBreakpointsResult;
pub use response::SetVariableResult;
pub use response::StackTraceResult;
pub use response::StatusResult;
pub use response::ThreadsResult;
pub use response::VariablesResult;
pub use response::WaitedEvent;
