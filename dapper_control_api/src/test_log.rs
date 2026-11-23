// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! What `tracing` formats while a test runs, for asserting what code logs.

use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

/// Everything `run` logs at info level and above, one event per line.
pub(crate) fn logged_by(run: impl FnOnce()) -> String {
    let logged = Logged::default();
    let writer = logged.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = logged
        .0
        .lock()
        .expect("the log buffer should not be poisoned")
        .clone();
    String::from_utf8(bytes).expect("the log should be UTF-8")
}

#[derive(Clone, Default)]
struct Logged(Arc<Mutex<Vec<u8>>>);

impl Write for Logged {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("the log buffer should not be poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
