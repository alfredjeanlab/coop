// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use axum::http::HeaderMap;

use crate::error::ErrorCode;
use crate::transport::auth::{validate_bearer, validate_ws_auth, validate_ws_query};

#[test]
fn no_token_allows_all() -> anyhow::Result<()> {
    let headers = HeaderMap::new();
    assert!(validate_bearer(&headers, None).is_ok());
    Ok(())
}

#[test]
fn valid_bearer_passes() -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        "Bearer secret123"
            .parse()
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    assert!(validate_bearer(&headers, Some("secret123")).is_ok());
    Ok(())
}

#[test]
fn invalid_bearer_rejects() -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        "Bearer wrong".parse().map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    assert_eq!(
        validate_bearer(&headers, Some("secret123")).err(),
        Some(ErrorCode::Unauthorized)
    );
    Ok(())
}

#[test]
fn missing_header_rejects() -> anyhow::Result<()> {
    let headers = HeaderMap::new();
    assert_eq!(
        validate_bearer(&headers, Some("secret123")).err(),
        Some(ErrorCode::Unauthorized)
    );
    Ok(())
}

#[test]
fn non_bearer_scheme_rejects() -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        "Basic dXNlcjpwYXNz"
            .parse()
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    assert_eq!(
        validate_bearer(&headers, Some("secret123")).err(),
        Some(ErrorCode::Unauthorized)
    );
    Ok(())
}

#[test]
fn ws_query_token_valid() -> anyhow::Result<()> {
    assert!(validate_ws_query("token=secret123&mode=all", Some("secret123")).is_ok());
    Ok(())
}

#[test]
fn ws_query_token_invalid() -> anyhow::Result<()> {
    assert_eq!(
        validate_ws_query("token=wrong", Some("secret123")).err(),
        Some(ErrorCode::Unauthorized)
    );
    Ok(())
}

#[test]
fn ws_query_no_token_param() -> anyhow::Result<()> {
    assert_eq!(
        validate_ws_query("mode=all", Some("secret123")).err(),
        Some(ErrorCode::Unauthorized)
    );
    Ok(())
}

#[test]
fn ws_query_no_expected() -> anyhow::Result<()> {
    assert!(validate_ws_query("mode=all", None).is_ok());
    Ok(())
}

#[test]
fn ws_auth_valid() -> anyhow::Result<()> {
    assert!(validate_ws_auth("secret123", Some("secret123")).is_ok());
    Ok(())
}

#[test]
fn ws_auth_invalid() -> anyhow::Result<()> {
    assert_eq!(
        validate_ws_auth("wrong", Some("secret123")).err(),
        Some(ErrorCode::Unauthorized)
    );
    Ok(())
}

#[test]
fn ws_auth_no_expected() -> anyhow::Result<()> {
    assert!(validate_ws_auth("anything", None).is_ok());
    Ok(())
}
