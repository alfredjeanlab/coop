// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

pub mod attach;
pub mod nbio;
pub mod spawn;

use bytes::Bytes;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc;

use crate::driver::ExitStatus;

/// Terminal backend abstraction over PTY or compatibility layers.
///
/// Object-safe for use as `Box<dyn Backend>`.
pub trait Backend: Send + 'static {
    fn run(
        &mut self,
        output_tx: mpsc::Sender<Bytes>,
        input_rx: mpsc::Receiver<Bytes>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ExitStatus>> + Send + '_>>;

    fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()>;

    fn child_pid(&self) -> Option<u32>;
}
