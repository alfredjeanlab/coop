// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context};
use bytes::Bytes;
use nix::libc;
use nix::pty::{forkpty, ForkptyResult, Winsize};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{execvp, ForkResult, Pid};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

use super::nbio::{read_chunk, set_nonblocking, write_all, PtyFd};
use super::Backend;
use crate::driver::ExitStatus;

/// Native PTY backend that spawns a child process via `forkpty`.
pub struct NativePty {
    master: AsyncFd<PtyFd>,
    child_pid: Pid,
    cols: Arc<AtomicU16>,
    rows: Arc<AtomicU16>,
}

impl NativePty {
    /// Spawn a child process on a new PTY.
    ///
    /// `command` must have at least one element (the program to run).
    // forkpty requires unsafe: post-fork child is partially initialized
    #[allow(unsafe_code)]
    pub fn spawn(command: &[String], cols: u16, rows: u16) -> anyhow::Result<Self> {
        let winsize = Winsize {
            ws_col: cols,
            ws_row: rows,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: forkpty is unsafe because the child is in a
        // partially-initialized state after fork. We immediately exec.
        let result = unsafe { forkpty(&winsize, None) }.context("forkpty failed")?;
        let ForkptyResult {
            master,
            fork_result,
        } = result;

        match fork_result {
            ForkResult::Child => {
                // Child process: set env and exec
                std::env::set_var("TERM", "xterm-256color");
                std::env::set_var("COOP", "1");

                let c_args: Vec<CString> = command
                    .iter()
                    .map(|s| CString::new(s.as_bytes()))
                    .collect::<Result<_, _>>()
                    .context("invalid command argument")?;

                execvp(&c_args[0], &c_args).context("execvp failed")?;
                unreachable!();
            }
            ForkResult::Parent { child } => {
                set_nonblocking(&master)?;
                let afd = AsyncFd::new(PtyFd(master)).context("AsyncFd::new failed")?;
                Ok(Self {
                    master: afd,
                    child_pid: child,
                    cols: Arc::new(AtomicU16::new(cols)),
                    rows: Arc::new(AtomicU16::new(rows)),
                })
            }
        }
    }
}

impl Backend for NativePty {
    fn run(
        &mut self,
        output_tx: mpsc::Sender<Bytes>,
        mut input_rx: mpsc::Receiver<Bytes>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<ExitStatus>> + Send + '_>>
    {
        let pid = self.child_pid;
        Box::pin(async move {
            let mut buf = vec![0u8; 8192];
            let mut input_closed = false;

            loop {
                if input_closed {
                    // Only read output once input is closed
                    match read_chunk(&self.master, &mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = Bytes::copy_from_slice(&buf[..n]);
                            if output_tx.send(data).await.is_err() {
                                break;
                            }
                        }
                        Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                        Err(e) => return Err(e.into()),
                    }
                } else {
                    tokio::select! {
                        result = read_chunk(&self.master, &mut buf) => {
                            match result {
                                Ok(0) => break,
                                Ok(n) => {
                                    let data = Bytes::copy_from_slice(&buf[..n]);
                                    if output_tx.send(data).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                                Err(e) => return Err(e.into()),
                            }
                        }
                        input = input_rx.recv() => {
                            match input {
                                Some(data) => {
                                    write_all(&self.master, &data).await?;
                                }
                                None => input_closed = true,
                            }
                        }
                    }
                }
            }

            // Reap child on a blocking thread to avoid blocking the runtime
            let status = tokio::task::spawn_blocking(move || wait_for_exit(pid))
                .await
                .context("join wait thread")??;
            Ok(status)
        })
    }

    // TIOCSWINSZ ioctl requires unsafe for the libc::ioctl call
    #[allow(unsafe_code)]
    fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.cols.store(cols, Ordering::Relaxed);
        self.rows.store(rows, Ordering::Relaxed);

        let ws = Winsize {
            ws_col: cols,
            ws_row: rows,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: TIOCSWINSZ is a well-defined ioctl that sets the window
        // size on the PTY master fd. The Winsize struct is properly
        // initialized.
        let ret = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        if ret < 0 {
            bail!(
                "TIOCSWINSZ ioctl failed: {}",
                std::io::Error::last_os_error()
            );
        }

        Ok(())
    }

    fn child_pid(&self) -> Option<u32> {
        Some(self.child_pid.as_raw() as u32)
    }
}

impl Drop for NativePty {
    fn drop(&mut self) {
        // Best-effort graceful shutdown: SIGHUP then SIGKILL.
        let _ = kill(self.child_pid, Signal::SIGHUP);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = kill(self.child_pid, Signal::SIGKILL);
        let _ = waitpid(self.child_pid, Some(WaitPidFlag::WNOHANG));
    }
}

/// Block until the child exits and convert to our `ExitStatus`.
fn wait_for_exit(pid: Pid) -> anyhow::Result<ExitStatus> {
    loop {
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_, code)) => {
                return Ok(ExitStatus {
                    code: Some(code),
                    signal: None,
                });
            }
            Ok(WaitStatus::Signaled(_, sig, _)) => {
                return Ok(ExitStatus {
                    code: None,
                    signal: Some(sig as i32),
                });
            }
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => bail!("waitpid failed: {e}"),
        }
    }
}
