// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Write lock stress tests: concurrent HTTP and WebSocket writer patterns.

use std::sync::Arc;

use axum::http::StatusCode;

use coop::test_support::{spawn_http_server, AppStateBuilder};
use coop::transport::build_router;
use coop::transport::http::InputRequest;

// ---------------------------------------------------------------------------
// concurrent_http_requests_serialize
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_http_requests_serialize() -> anyhow::Result<()> {
    let (app_state, mut input_rx) = AppStateBuilder::new().ring_size(65536).build();
    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/input");

    // Send 10 parallel input requests
    let mut handles = Vec::new();
    for i in 0..10 {
        let c = client.clone();
        let u = url.clone();
        handles.push(tokio::spawn(async move {
            let resp = c
                .post(&u)
                .json(&InputRequest {
                    text: format!("msg{i}"),
                    enter: false,
                })
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("request failed: {e}"))?;
            Ok::<_, anyhow::Error>(resp.status())
        }));
    }

    let mut ok_count = 0u32;
    let mut conflict_count = 0u32;
    for handle in handles {
        let status = handle.await??;
        if status == reqwest::StatusCode::OK {
            ok_count += 1;
        } else if status == reqwest::StatusCode::CONFLICT {
            conflict_count += 1;
        }
    }

    // At least 1 should succeed; the rest may be either OK or CONFLICT
    // (HTTP lock is per-request, so fast sequential requests may all succeed)
    assert!(
        ok_count >= 1,
        "expected at least 1 success: ok={ok_count}, conflict={conflict_count}"
    );
    assert_eq!(ok_count + conflict_count, 10);

    // Drain input events
    let mut delivered = 0u32;
    while let Ok(Some(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(100), input_rx.recv()).await
    {
        delivered += 1;
    }
    assert_eq!(
        delivered, ok_count,
        "delivered events should match OK count"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_holds_lock_blocks_http
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_holds_lock_blocks_http() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();

    // Simulate WS client acquiring the lock
    app_state
        .lifecycle
        .write_lock
        .acquire_ws("ws-client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let router = build_router(Arc::clone(&app_state));
    let server = axum_test::TestServer::new(router)?;

    // All HTTP input attempts should get 409
    for _ in 0..5 {
        let resp = server
            .post("/api/v1/input")
            .json(&InputRequest {
                text: "blocked".to_owned(),
                enter: false,
            })
            .await;
        resp.assert_status(StatusCode::CONFLICT);
    }

    // Release the WS lock
    app_state
        .lifecycle
        .write_lock
        .release_ws("ws-client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Now HTTP should succeed
    let resp = server
        .post("/api/v1/input")
        .json(&InputRequest {
            text: "unblocked".to_owned(),
            enter: false,
        })
        .await;
    resp.assert_status(StatusCode::OK);

    Ok(())
}

// ---------------------------------------------------------------------------
// http_guard_drop_unblocks_ws
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_guard_drop_unblocks_ws() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();

    // Acquire HTTP lock
    {
        let _guard = app_state
            .lifecycle
            .write_lock
            .acquire_http()
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // While held, WS acquire should fail
        let result = app_state.lifecycle.write_lock.acquire_ws("ws-client-1");
        assert!(
            result.is_err(),
            "WS should be blocked while HTTP holds lock"
        );
    }
    // Guard dropped here — lock released

    // Now WS can acquire
    app_state
        .lifecycle
        .write_lock
        .acquire_ws("ws-client-1")
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// force_release_on_disconnect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn force_release_on_disconnect() -> anyhow::Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();
    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;

    // Connect WS and acquire lock
    let url = format!("ws://{addr}/ws");
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| anyhow::anyhow!("ws connect: {e}"))?;
    let (mut ws_tx, _ws_rx) = ws_stream.split();

    // Send lock acquire
    let lock_msg = serde_json::json!({"type": "lock", "action": "acquire"});
    ws_tx
        .send(WsMessage::Text(lock_msg.to_string().into()))
        .await
        .map_err(|e| anyhow::anyhow!("ws send: {e}"))?;

    // Brief wait for lock to be acquired
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify lock is held (another WS client can't acquire)
    assert!(
        app_state.lifecycle.write_lock.is_held(),
        "lock should be held after WS acquire"
    );

    // Drop the WS connection (disconnect)
    let close_msg = WsMessage::Close(None);
    let _ = ws_tx.send(close_msg).await;
    drop(ws_tx);
    drop(_ws_rx);

    // Wait for server to process the disconnect
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Lock should be released after disconnect
    assert!(
        !app_state.lifecycle.write_lock.is_held(),
        "lock should be released after WS disconnect"
    );

    // New client should be able to acquire
    let result = app_state.lifecycle.write_lock.acquire_ws("new-client");
    assert!(result.is_ok(), "new client should acquire lock");

    Ok(())
}
