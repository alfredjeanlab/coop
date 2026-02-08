// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::driver::ExitStatus;
use crate::pty::Backend;

/// Specifies which terminal multiplexer session to attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachSpec {
    Tmux { session: String },
    Screen { session: String },
}

impl FromStr for AttachSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, name) = s.split_once(':').ok_or_else(|| {
            anyhow::anyhow!("invalid attach spec: expected 'tmux:NAME' or 'screen:NAME'")
        })?;
        if name.is_empty() {
            anyhow::bail!("invalid attach spec: session name cannot be empty");
        }
        match prefix {
            "tmux" => Ok(AttachSpec::Tmux {
                session: name.to_string(),
            }),
            "screen" => Ok(AttachSpec::Screen {
                session: name.to_string(),
            }),
            other => anyhow::bail!("invalid attach spec: unknown backend '{other}'"),
        }
    }
}

/// Compatibility backend that attaches to an existing tmux session.
pub struct TmuxBackend {
    session: String,
    target: String,
    poll_interval: Duration,
}

impl TmuxBackend {
    /// Create a new `TmuxBackend` for the given tmux session.
    ///
    /// Validates the session exists via `tmux has-session`.
    pub fn new(session: String) -> anyhow::Result<Self> {
        let status = std::process::Command::new("tmux")
            .args(["has-session", "-t", &session])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(_) => anyhow::bail!("tmux session '{session}' does not exist"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("tmux is not installed or not in PATH")
            }
            Err(e) => return Err(anyhow::Error::new(e).context("failed to check tmux session")),
        }

        let target = session.clone();
        Ok(Self {
            session,
            target,
            poll_interval: Duration::from_secs(1),
        })
    }

    /// Returns the session name.
    pub fn session(&self) -> &str {
        &self.session
    }
}

impl Backend for TmuxBackend {
    fn run(
        &mut self,
        output_tx: mpsc::Sender<Bytes>,
        mut input_rx: mpsc::Receiver<Bytes>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ExitStatus>> + Send + '_>> {
        Box::pin(async move {
            let mut interval = tokio::time::interval(self.poll_interval);
            let mut prev_capture = String::new();

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let output = tokio::process::Command::new("tmux")
                            .args(["capture-pane", "-p", "-e", "-t", &self.target])
                            .output()
                            .await;

                        match output {
                            Ok(out) if out.status.success() => {
                                let capture = String::from_utf8_lossy(&out.stdout)
                                    .into_owned();
                                if capture != prev_capture {
                                    prev_capture = capture.clone();
                                    let frame = format!("\x1b[H\x1b[2J{capture}");
                                    if output_tx.send(Bytes::from(frame)).await.is_err() {
                                        return Ok(ExitStatus {
                                            code: None,
                                            signal: None,
                                        });
                                    }
                                }
                            }
                            _ => {
                                // Session is gone
                                return Ok(ExitStatus {
                                    code: None,
                                    signal: None,
                                });
                            }
                        }
                    }
                    data = input_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                let text = String::from_utf8_lossy(&bytes);
                                let status = tokio::process::Command::new("tmux")
                                    .args([
                                        "send-keys", "-l", "-t", &self.target, &text,
                                    ])
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .status()
                                    .await;
                                if status.is_err() {
                                    return Ok(ExitStatus {
                                        code: None,
                                        signal: None,
                                    });
                                }
                            }
                            None => {
                                return Ok(ExitStatus {
                                    code: None,
                                    signal: None,
                                });
                            }
                        }
                    }
                }
            }
        })
    }

    fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let status = std::process::Command::new("tmux")
            .args([
                "resize-pane",
                "-t",
                &self.target,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;

        if !status.success() {
            anyhow::bail!("tmux resize-pane failed");
        }
        Ok(())
    }

    fn child_pid(&self) -> Option<u32> {
        let output = std::process::Command::new("tmux")
            .args(["display-message", "-p", "-t", &self.target, "#{pane_pid}"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&output.stdout);
        text.trim().parse().ok()
    }
}

#[cfg(test)]
#[path = "attach_tests.rs"]
mod tests;
