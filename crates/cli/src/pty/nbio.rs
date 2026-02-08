// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use tokio::io::unix::AsyncFd;

/// Newtype wrapper around `OwnedFd` for use with `AsyncFd`.
#[derive(Debug)]
pub struct PtyFd(pub OwnedFd);

impl AsRawFd for PtyFd {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

/// Set the given file descriptor to non-blocking mode.
pub fn set_nonblocking(fd: &impl AsRawFd) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    let flags = fcntl(raw, FcntlArg::F_GETFL).map_err(io_err)?;
    let flags = OFlag::from_bits_truncate(flags);
    fcntl(raw, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map_err(io_err)?;
    Ok(())
}

/// Read a chunk of data from the async PTY fd.
pub async fn read_chunk(afd: &AsyncFd<PtyFd>, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let mut guard = afd.readable().await?;
        match guard.try_io(|inner| {
            let n = nix::unistd::read(inner.as_raw_fd(), buf).map_err(io_err)?;
            Ok(n)
        }) {
            Ok(result) => return result,
            Err(_would_block) => continue,
        }
    }
}

/// Write all data to the async PTY fd.
pub async fn write_all(afd: &AsyncFd<PtyFd>, data: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        let mut guard = afd.writable().await?;
        match guard.try_io(|inner| {
            let n = nix::unistd::write(inner, &data[offset..]).map_err(io_err)?;
            Ok(n)
        }) {
            Ok(Ok(n)) => offset += n,
            Ok(Err(e)) => return Err(e),
            Err(_would_block) => continue,
        }
    }
    Ok(())
}

fn io_err(e: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(e as i32)
}
