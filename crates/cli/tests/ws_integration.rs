// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! WebSocket integration tests using real connections against an in-process
//! axum server.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use coop::driver::AgentState;
use coop::event::{OutputEvent, StateChangeEvent};
use coop::test_support::{spawn_http_server, AppStateBuilder, StubNudgeEncoder};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsTx = futures_util::stream::SplitSink<WsStream, WsMessage>;
type WsRx = futures_util::stream::SplitStream<WsStream>;

/// Send a JSON message over the WebSocket.
async fn ws_send(stream: &mut WsTx, value: &serde_json::Value) -> anyhow::Result<()> {
    let text = serde_json::to_string(value)?;
    stream
        .send(WsMessage::Text(text.into()))
        .await
        .map_err(|e| anyhow::anyhow!("ws send: {e}"))?;
    Ok(())
}

/// Receive a JSON message from the WebSocket with timeout.
async fn ws_recv(stream: &mut WsRx, timeout: Duration) -> anyhow::Result<serde_json::Value> {
    let msg = tokio::time::timeout(timeout, stream.next())
        .await
        .map_err(|_| anyhow::anyhow!("ws recv timeout"))?
        .ok_or_else(|| anyhow::anyhow!("ws stream closed"))?
        .map_err(|e| anyhow::anyhow!("ws recv: {e}"))?;

    match msg {
        WsMessage::Text(text) => {
            let parsed: serde_json::Value = serde_json::from_str(&text)?;
            Ok(parsed)
        }
        other => anyhow::bail!("expected Text message, got {other:?}"),
    }
}

/// Connect a WebSocket to the given server address with optional query params.
async fn ws_connect(addr: &std::net::SocketAddr, query: &str) -> anyhow::Result<(WsTx, WsRx)> {
    let url = if query.is_empty() {
        format!("ws://{addr}/ws")
    } else {
        format!("ws://{addr}/ws?{query}")
    };
    let (stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| anyhow::anyhow!("ws connect: {e}"))?;
    Ok(stream.split())
}

const RECV_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// ws_connect_and_receive_pong
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_connect_and_receive_pong() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    let (addr, _handle) = spawn_http_server(app_state).await?;

    let (mut tx, mut rx) = ws_connect(&addr, "").await?;

    // Send Ping
    ws_send(&mut tx, &serde_json::json!({"type": "ping"})).await?;

    // Should receive Pong
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("type").and_then(|t| t.as_str()),
        Some("pong"),
        "response: {resp}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_auth_query_param
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_auth_query_param() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().auth_token("test-secret").build();
    let (addr, _handle) = spawn_http_server(app_state).await?;

    // Connect with correct token
    let (mut tx, mut rx) = ws_connect(&addr, "token=test-secret").await?;
    ws_send(&mut tx, &serde_json::json!({"type": "ping"})).await?;
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(resp.get("type").and_then(|t| t.as_str()), Some("pong"));

    // Connect with wrong token — should get 401 HTTP response (connection refused)
    let url = format!("ws://{addr}/ws?token=wrong");
    let result = tokio_tungstenite::connect_async(&url).await;
    assert!(result.is_err(), "should reject connection with wrong token");

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_auth_message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_auth_message() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().auth_token("auth-secret").build();
    let (addr, _handle) = spawn_http_server(app_state).await?;

    // Connect without token (WS upgrade succeeds; needs auth via message)
    let (mut tx, mut rx) = ws_connect(&addr, "").await?;

    // Send wrong auth — should get error
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "auth", "token": "wrong"}),
    )
    .await?;
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("type").and_then(|t| t.as_str()),
        Some("error"),
        "wrong auth: {resp}"
    );

    // Send correct auth — should succeed (no error response)
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "auth", "token": "auth-secret"}),
    )
    .await?;

    // Verify subsequent operations work (ping/pong)
    ws_send(&mut tx, &serde_json::json!({"type": "ping"})).await?;
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(resp.get("type").and_then(|t| t.as_str()), Some("pong"));

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_lock_acquire_release
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_lock_acquire_release() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    let (addr, _handle) = spawn_http_server(app_state).await?;

    let (mut tx, mut rx) = ws_connect(&addr, "").await?;

    // Acquire lock — no error response means success
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "lock", "action": "acquire"}),
    )
    .await?;

    // Send input (should work since we hold the lock)
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "input", "text": "test"}),
    )
    .await?;

    // Release lock
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "lock", "action": "release"}),
    )
    .await?;

    // After release, input should fail with WRITER_BUSY
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "input", "text": "fail"}),
    )
    .await?;
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("code").and_then(|c| c.as_str()),
        Some("WRITER_BUSY"),
        "expected WRITER_BUSY after release: {resp}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_lock_conflict_two_clients
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_lock_conflict_two_clients() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    let (addr, _handle) = spawn_http_server(app_state).await?;

    // Client A acquires lock
    let (mut tx_a, _rx_a) = ws_connect(&addr, "").await?;
    ws_send(
        &mut tx_a,
        &serde_json::json!({"type": "lock", "action": "acquire"}),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Client B tries to acquire — should get WRITER_BUSY
    let (mut tx_b, mut rx_b) = ws_connect(&addr, "").await?;
    ws_send(
        &mut tx_b,
        &serde_json::json!({"type": "lock", "action": "acquire"}),
    )
    .await?;
    let resp = ws_recv(&mut rx_b, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("code").and_then(|c| c.as_str()),
        Some("WRITER_BUSY"),
        "client B should be blocked: {resp}"
    );

    // Client A releases
    ws_send(
        &mut tx_a,
        &serde_json::json!({"type": "lock", "action": "release"}),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Client B can now acquire
    ws_send(
        &mut tx_b,
        &serde_json::json!({"type": "lock", "action": "acquire"}),
    )
    .await?;

    // Verify by sending input (should succeed — no error response)
    ws_send(
        &mut tx_b,
        &serde_json::json!({"type": "input", "text": "from_b"}),
    )
    .await?;

    // Ping to verify connection still healthy
    ws_send(&mut tx_b, &serde_json::json!({"type": "ping"})).await?;
    let resp = ws_recv(&mut rx_b, RECV_TIMEOUT).await?;
    assert_eq!(resp.get("type").and_then(|t| t.as_str()), Some("pong"));

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_subscription_mode_raw
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_subscription_mode_raw() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();
    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;

    let (mut _tx, mut rx) = ws_connect(&addr, "mode=raw").await?;

    // Push raw output via broadcast
    let data = bytes::Bytes::from("hello raw");
    {
        let mut ring = app_state.terminal.ring.write().await;
        ring.write(&data);
    }
    let _ = app_state.channels.output_tx.send(OutputEvent::Raw(data));

    // Should receive Output message
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("type").and_then(|t| t.as_str()),
        Some("output"),
        "raw mode should receive output: {resp}"
    );

    // Push a ScreenUpdate — should NOT be forwarded in raw mode
    let _ = app_state
        .channels
        .output_tx
        .send(OutputEvent::ScreenUpdate { seq: 1 });

    // Try to read — should timeout (no message)
    let result =
        tokio::time::timeout(Duration::from_millis(200), ws_recv(&mut rx, RECV_TIMEOUT)).await;
    assert!(
        result.is_err(),
        "raw mode should not receive screen updates"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_subscription_mode_state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_subscription_mode_state() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();
    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;

    let (mut _tx, mut rx) = ws_connect(&addr, "mode=state").await?;

    // Push state change
    let _ = app_state.channels.state_tx.send(StateChangeEvent {
        prev: AgentState::Starting,
        next: AgentState::Working,
        seq: 1,
    });

    // Should receive StateChange
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("type").and_then(|t| t.as_str()),
        Some("state_change"),
        "state mode should receive state changes: {resp}"
    );
    assert_eq!(resp.get("next").and_then(|n| n.as_str()), Some("working"));

    // Push raw output — should NOT be forwarded in state mode
    let _ = app_state
        .channels
        .output_tx
        .send(OutputEvent::Raw(bytes::Bytes::from("ignored")));

    let result =
        tokio::time::timeout(Duration::from_millis(200), ws_recv(&mut rx, RECV_TIMEOUT)).await;
    assert!(result.is_err(), "state mode should not receive raw output");

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_subscription_mode_screen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_subscription_mode_screen() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();
    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;

    let (mut _tx, mut rx) = ws_connect(&addr, "mode=screen").await?;

    // Push screen update
    let _ = app_state
        .channels
        .output_tx
        .send(OutputEvent::ScreenUpdate { seq: 42 });

    // Should receive Screen message
    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("type").and_then(|t| t.as_str()),
        Some("screen"),
        "screen mode should receive screen updates: {resp}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_replay_from_offset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_replay_from_offset() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();

    // Write known data to ring buffer
    {
        let mut ring = app_state.terminal.ring.write().await;
        ring.write(b"replay-data-here");
    }

    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;
    let (mut tx, mut rx) = ws_connect(&addr, "").await?;

    // Send replay from offset 0
    ws_send(&mut tx, &serde_json::json!({"type": "replay", "offset": 0})).await?;

    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(resp.get("type").and_then(|t| t.as_str()), Some("output"));
    assert_eq!(resp.get("offset").and_then(|o| o.as_u64()), Some(0));

    // Decode data
    let b64 = resp
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing data"))?;
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)?;
    assert_eq!(decoded, b"replay-data-here");

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_concurrent_readers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_concurrent_readers() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();
    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;

    // Connect 5 clients with state mode
    let mut clients = Vec::new();
    for _ in 0..5 {
        let (tx, rx) = ws_connect(&addr, "mode=state").await?;
        clients.push((tx, rx));
    }

    // Push one state change
    let _ = app_state.channels.state_tx.send(StateChangeEvent {
        prev: AgentState::Starting,
        next: AgentState::Working,
        seq: 1,
    });

    // All 5 should receive the state change
    for (_tx, ref mut rx) in &mut clients {
        let resp = ws_recv(rx, RECV_TIMEOUT).await?;
        assert_eq!(
            resp.get("type").and_then(|t| t.as_str()),
            Some("state_change"),
            "all clients should receive state change"
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_disconnect_releases_lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_disconnect_releases_lock() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();
    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;

    // Client acquires lock then disconnects
    {
        let (mut tx, _rx) = ws_connect(&addr, "").await?;
        ws_send(
            &mut tx,
            &serde_json::json!({"type": "lock", "action": "acquire"}),
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Drop the connection
    }

    // Wait for cleanup
    tokio::time::sleep(Duration::from_millis(200)).await;

    // New client should be able to acquire lock
    let (mut tx2, mut _rx2) = ws_connect(&addr, "").await?;
    ws_send(
        &mut tx2,
        &serde_json::json!({"type": "lock", "action": "acquire"}),
    )
    .await?;

    // Verify by sending input (should succeed — no error)
    ws_send(
        &mut tx2,
        &serde_json::json!({"type": "input", "text": "after_reconnect"}),
    )
    .await?;
    ws_send(&mut tx2, &serde_json::json!({"type": "ping"})).await?;

    // Read responses — should get pong, not error
    // (input has no response on success; ping gives pong)
    let mut got_pong = false;
    for _ in 0..5 {
        match ws_recv(&mut _rx2, Duration::from_millis(500)).await {
            Ok(resp) => {
                if resp.get("type").and_then(|t| t.as_str()) == Some("pong") {
                    got_pong = true;
                    break;
                }
                // Error with WRITER_BUSY means lock wasn't released
                if resp.get("code").and_then(|c| c.as_str()) == Some("WRITER_BUSY") {
                    anyhow::bail!("lock was not released after disconnect");
                }
            }
            Err(_) => break,
        }
    }
    assert!(got_pong, "should be able to operate after reconnect");

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_resize_no_lock_required
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_resize_no_lock_required() -> anyhow::Result<()> {
    let (app_state, mut rx) = AppStateBuilder::new().build();
    let (addr, _handle) = spawn_http_server(app_state).await?;

    let (mut tx, _ws_rx) = ws_connect(&addr, "").await?;

    // Send resize without acquiring lock — should succeed
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "resize", "cols": 120, "rows": 40}),
    )
    .await?;

    // Verify resize event received
    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await?;
    match event {
        Some(coop::event::InputEvent::Resize { cols, rows }) => {
            assert_eq!(cols, 120);
            assert_eq!(rows, 40);
        }
        other => anyhow::bail!("expected Resize event, got {other:?}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_nudge_requires_lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_nudge_requires_lock() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new()
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .agent_state(AgentState::WaitingForInput)
        .build();
    app_state
        .ready
        .store(true, std::sync::atomic::Ordering::Release);

    let (addr, _handle) = spawn_http_server(app_state).await?;

    let (mut tx, mut rx) = ws_connect(&addr, "").await?;

    // Send nudge without lock — should get WRITER_BUSY
    ws_send(
        &mut tx,
        &serde_json::json!({"type": "nudge", "message": "hello"}),
    )
    .await?;

    let resp = ws_recv(&mut rx, RECV_TIMEOUT).await?;
    assert_eq!(
        resp.get("code").and_then(|c| c.as_str()),
        Some("WRITER_BUSY"),
        "nudge without lock should fail: {resp}"
    );

    Ok(())
}
