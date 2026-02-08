// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Large output tests: ring buffer integrity, wrap-around, HTTP/WS endpoints.

use std::sync::Arc;

use coop::ring::RingBuffer;
use coop::test_support::AppStateBuilder;
use coop::transport::build_router;
use coop::transport::http::OutputResponse;

use axum::http::StatusCode;

// ---------------------------------------------------------------------------
// large_output_ring_buffer_integrity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn large_output_ring_buffer_integrity() -> anyhow::Result<()> {
    let capacity = 1_048_576; // 1MB
    let mut ring = RingBuffer::new(capacity);

    // Write 128KB of known-pattern data
    let pattern: Vec<u8> = (0..128 * 1024).map(|i| (i % 256) as u8).collect();
    ring.write(&pattern);

    assert_eq!(ring.total_written(), pattern.len() as u64);

    // Read from offset 0 — should get all data
    let (a, b) = ring
        .read_from(0)
        .ok_or_else(|| anyhow::anyhow!("no data"))?;
    let mut data = a.to_vec();
    data.extend_from_slice(b);

    assert_eq!(data.len(), pattern.len());
    assert_eq!(data, pattern);

    Ok(())
}

// ---------------------------------------------------------------------------
// ring_buffer_wrap_preserves_recent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ring_buffer_wrap_preserves_recent() -> anyhow::Result<()> {
    let capacity = 1_048_576; // 1MB
    let mut ring = RingBuffer::new(capacity);

    // Write 2MB — oldest 1MB should be overwritten
    let block1: Vec<u8> = vec![0xAA; capacity];
    let block2: Vec<u8> = vec![0xBB; capacity];
    ring.write(&block1);
    ring.write(&block2);

    assert_eq!(ring.total_written(), 2 * capacity as u64);

    // Reading from offset 0 should fail (overwritten)
    assert!(
        ring.read_from(0).is_none(),
        "offset 0 should be overwritten"
    );

    // Reading from offset = capacity should succeed and return block2
    let (a, b) = ring
        .read_from(capacity as u64)
        .ok_or_else(|| anyhow::anyhow!("expected data at offset {capacity}"))?;
    let mut data = a.to_vec();
    data.extend_from_slice(b);

    assert_eq!(data.len(), capacity);
    assert!(data.iter().all(|&byte| byte == 0xBB));

    Ok(())
}

// ---------------------------------------------------------------------------
// http_output_endpoint_large_response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_output_endpoint_large_response() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(1_048_576).build();

    // Write 128KB of known data directly to the ring buffer
    let pattern: Vec<u8> = (0..128 * 1024).map(|i| (i % 256) as u8).collect();
    {
        let mut ring = app_state.terminal.ring.write().await;
        ring.write(&pattern);
    }

    let router = build_router(Arc::clone(&app_state));
    let server = axum_test::TestServer::new(router)?;

    let resp = server.get("/api/v1/output?offset=0").await;
    resp.assert_status(StatusCode::OK);
    let output: OutputResponse = resp.json();

    // Decode base64
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &output.data)?;

    assert_eq!(decoded.len(), pattern.len());
    assert_eq!(decoded, pattern);
    assert_eq!(output.offset, 0);
    assert_eq!(output.next_offset, pattern.len() as u64);

    Ok(())
}

// ---------------------------------------------------------------------------
// ws_replay_large_offset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_replay_large_offset() -> anyhow::Result<()> {
    use coop::test_support::spawn_http_server;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let (app_state, _rx) = AppStateBuilder::new().ring_size(1_048_576).build();

    // Write 256KB
    let data: Vec<u8> = (0..256 * 1024).map(|i| (i % 256) as u8).collect();
    {
        let mut ring = app_state.terminal.ring.write().await;
        ring.write(&data);
    }

    let (addr, _handle) = spawn_http_server(Arc::clone(&app_state)).await?;

    // Connect WebSocket
    let url = format!("ws://{addr}/ws");
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| anyhow::anyhow!("ws connect: {e}"))?;
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Send replay from offset 128000
    let replay = serde_json::json!({"type": "replay", "offset": 128000});
    ws_tx
        .send(WsMessage::Text(replay.to_string().into()))
        .await
        .map_err(|e| anyhow::anyhow!("ws send: {e}"))?;

    // Read the response
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws_rx.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("ws stream closed"))?
        .map_err(|e| anyhow::anyhow!("ws recv: {e}"))?;

    let text = match msg {
        WsMessage::Text(t) => t.to_string(),
        other => anyhow::bail!("expected Text, got {other:?}"),
    };

    let parsed: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(
        parsed.get("type").and_then(|t| t.as_str()),
        Some("output"),
        "response: {text}"
    );
    assert_eq!(
        parsed.get("offset").and_then(|o| o.as_u64()),
        Some(128000),
        "response: {text}"
    );

    // Decode and verify content matches
    let b64_data = parsed
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing data field"))?;
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64_data)?;
    let expected = &data[128000..];
    assert_eq!(decoded.len(), expected.len());
    assert_eq!(decoded, expected);

    Ok(())
}

// ---------------------------------------------------------------------------
// concurrent_readers_during_write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_readers_during_write() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(1_048_576).build();
    let state = Arc::clone(&app_state);

    // Writer task: pump data into ring buffer
    let writer_state = Arc::clone(&state);
    let writer = tokio::spawn(async move {
        for i in 0..100u32 {
            let chunk = vec![(i % 256) as u8; 1024];
            let mut ring = writer_state.terminal.ring.write().await;
            ring.write(&chunk);
            drop(ring);
            tokio::task::yield_now().await;
        }
    });

    // 3 reader tasks reading concurrently
    let mut reader_handles = Vec::new();
    for _ in 0..3 {
        let reader_state = Arc::clone(&state);
        reader_handles.push(tokio::spawn(async move {
            for _ in 0..50 {
                let ring = reader_state.terminal.ring.read().await;
                let total = ring.total_written();
                if total > 0 {
                    // Try to read some data — should not panic
                    let _data = ring.read_from(total.saturating_sub(1024));
                }
                drop(ring);
                tokio::task::yield_now().await;
            }
        }));
    }

    writer.await?;
    for handle in reader_handles {
        handle.await?;
    }

    // Verify final state is consistent
    let ring = state.terminal.ring.read().await;
    assert_eq!(ring.total_written(), 100 * 1024);

    Ok(())
}
