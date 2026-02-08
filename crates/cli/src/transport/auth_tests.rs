// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use axum::http::HeaderMap;

use crate::error::ErrorCode;
use crate::test_support::AnyhowExt;
use crate::transport::auth::{validate_bearer, validate_ws_auth, validate_ws_query};

#[yare::parameterized(
    no_token_allows_all = { None, None, true },
    valid_bearer        = { Some("secret123"), Some("Bearer secret123"), true },
    invalid_bearer      = { Some("secret123"), Some("Bearer wrong"), false },
    missing_header      = { Some("secret123"), None, false },
    wrong_scheme        = { Some("secret123"), Some("Basic dXNlcjpwYXNz"), false },
)]
fn bearer_validation(
    expected_token: Option<&str>,
    header_value: Option<&str>,
    should_pass: bool,
) -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    if let Some(val) = header_value {
        headers.insert("authorization", val.parse().anyhow()?);
    }
    let result = validate_bearer(&headers, expected_token);
    if should_pass {
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    } else {
        assert_eq!(result.err(), Some(ErrorCode::Unauthorized));
    }
    Ok(())
}

#[yare::parameterized(
    valid_token    = { "token=secret123&mode=all", Some("secret123"), true },
    invalid_token  = { "token=wrong", Some("secret123"), false },
    no_token_param = { "mode=all", Some("secret123"), false },
    no_expected    = { "mode=all", None, true },
)]
fn ws_query_validation(
    query: &str,
    expected: Option<&str>,
    should_pass: bool,
) -> anyhow::Result<()> {
    let result = validate_ws_query(query, expected);
    if should_pass {
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    } else {
        assert_eq!(result.err(), Some(ErrorCode::Unauthorized));
    }
    Ok(())
}

#[yare::parameterized(
    valid       = { "secret123", Some("secret123"), true },
    invalid     = { "wrong", Some("secret123"), false },
    no_expected = { "anything", None, true },
)]
fn ws_auth_validation(
    token: &str,
    expected: Option<&str>,
    should_pass: bool,
) -> anyhow::Result<()> {
    let result = validate_ws_auth(token, expected);
    if should_pass {
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    } else {
        assert_eq!(result.err(), Some(ErrorCode::Unauthorized));
    }
    Ok(())
}
