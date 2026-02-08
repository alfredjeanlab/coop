// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::error::ErrorCode;
use crate::transport::state::WriteLock;

#[test]
fn acquire_http_ok() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    let _guard = lock.acquire_http().map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(lock.is_held());
    Ok(())
}

#[test]
fn acquire_http_conflict() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    let _guard = lock.acquire_http().map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = lock.acquire_http();
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}

#[test]
fn http_guard_releases_on_drop() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    {
        let _guard = lock.acquire_http().map_err(|e| anyhow::anyhow!("{e}"))?;
        assert!(lock.is_held());
    }
    assert!(!lock.is_held());
    let _guard = lock.acquire_http().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[test]
fn acquire_ws_ok() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(lock.is_held());
    Ok(())
}

#[test]
fn acquire_ws_conflict() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = lock.acquire_ws("client-2");
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}

#[test]
fn acquire_ws_same_client_is_idempotent() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[test]
fn release_ws_then_acquire() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    lock.release_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(!lock.is_held());
    lock.acquire_ws("client-2")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[test]
fn wrong_owner_cannot_release() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = lock.release_ws("client-2");
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}

#[test]
fn force_release_ws_clears_lock() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    lock.force_release_ws("client-1");
    assert!(!lock.is_held());
    Ok(())
}

#[test]
fn force_release_ws_ignores_wrong_owner() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    lock.force_release_ws("client-2");
    assert!(lock.is_held());
    Ok(())
}

#[test]
fn http_blocks_ws() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    let _guard = lock.acquire_http().map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = lock.acquire_ws("client-1");
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}

#[test]
fn ws_blocks_http() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = lock.acquire_http();
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}

#[test]
fn check_ws_owner_ok() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    lock.check_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[test]
fn check_ws_wrong_owner() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    lock.acquire_ws("client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = lock.check_ws("client-2");
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}

#[test]
fn check_ws_not_held() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    let result = lock.check_ws("client-1");
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}

#[test]
fn check_ws_held_by_http() -> anyhow::Result<()> {
    let lock = WriteLock::new();
    let _guard = lock.acquire_http().map_err(|e| anyhow::anyhow!("{e}"))?;
    let result = lock.check_ws("client-1");
    assert_eq!(result.err(), Some(ErrorCode::WriterBusy));
    Ok(())
}
