// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

fn main() -> std::io::Result<()> {
    let proto_file = "control.proto";
    println!("cargo:rerun-if-changed={}", proto_file);

    setup_protoc_env();

    let tonic = tonic_prost_build::configure();
    tonic.compile_protos(&[proto_file], &["."] /* includes */)
}

/// Setup process level env vars for tonic to find protoc etc
fn setup_protoc_env() {
    let protoc_bin = protoc_bin_vendored::protoc_bin_path().unwrap();
    // SAFETY: build.rs runs single-threaded, so set_var is safe here.
    unsafe {
        std::env::set_var("PROTOC", protoc_bin);
    }

    let protoc_inc = protoc_bin_vendored::include_path().unwrap();
    let protoc_inc = protoc_inc.canonicalize().unwrap(); // protoc wants canonicalized paths
    // SAFETY: build.rs runs single-threaded, so set_var is safe here.
    unsafe {
        std::env::set_var("PROTOC_INCLUDE", protoc_inc);
    }
}
