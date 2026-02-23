// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Docker end-to-end tests exercising coop running as a real container.
//!
//! Gated behind `COOP_DOCKER_TESTS=1`. Requires `docker` in PATH and the
//! `coop:test` image (built by `make docker-claudeless`).
//!
//! Run: `COOP_DOCKER_TESTS=1 cargo test --test docker_e2e -- --test-threads=1`

use std::net::TcpListener;
use std::process::Command;
use std::sync::Once;
use std::time::Duration;

use futures_util::StreamExt;

/// Skip the test if `COOP_DOCKER_TESTS` is not set.
macro_rules! skip_unless_docker {
    () => {
        if std::env::var("COOP_DOCKER_TESTS").is_err() {
            eprintln!("skipping docker test (set COOP_DOCKER_TESTS=1 to enable)");
            return Ok(());
        }
    };
}

static BUILD_ONCE: Once = Once::new();

/// Build the `coop:test` Docker image exactly once per test run.
fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|p| p.join("Dockerfile").exists())
        .expect("could not find workspace root with Dockerfile")
        .to_path_buf()
}

fn ensure_image_built() {
    BUILD_ONCE.call_once(|| {
        let root = workspace_root();
        let status = Command::new("docker")
            .args(["build", "--target", "claudeless", "-t", "coop:test", "."])
            .current_dir(&root)
            .status()
            .expect("failed to run docker build");
        assert!(status.success(), "docker build failed");
    });
}

/// Find a free TCP port by binding to :0 then releasing.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().expect("local_addr").port()
}

/// A running Docker container that is removed on drop.
struct DockerContainer {
    id: String,
    port: u16,
}

impl DockerContainer {
    /// Start coop in a container with the given scenario file.
    fn start(scenario: &str) -> anyhow::Result<Self> {
        ensure_image_built();
        let port = free_port();
        let output = Command::new("docker")
            .args([
                "run",
                "-d",
                "-p",
                &format!("{port}:7070"),
                "coop:test",
                "--port",
                "7070",
                "--log-format",
                "text",
                "--agent",
                "claude",
                "--",
                "claudeless",
                "--scenario",
                &format!("/scenarios/{scenario}"),
                "hello",
            ])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let id = String::from_utf8(output.stdout)?.trim().to_owned();
        Ok(Self { id, port })
    }

    fn base_url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Poll the health endpoint until it succeeds or timeout.
    async fn wait_healthy(&self, timeout: Duration) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/v1/health", self.base_url());
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            if tokio::time::Instant::now() > deadline {
                // Dump logs for debugging
                let logs = Command::new("docker")
                    .args(["logs", &self.id])
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default();
                anyhow::bail!("container did not become healthy within {timeout:?}\nlogs:\n{logs}");
            }
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

impl Drop for DockerContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.id]).output();
    }
}

#[tokio::test]
async fn docker_health_endpoint() -> anyhow::Result<()> {
    skip_unless_docker!();

    let container = DockerContainer::start("claude_hello.toml")?;
    container.wait_healthy(Duration::from_secs(30)).await?;

    let resp: serde_json::Value =
        reqwest::get(format!("{}/api/v1/health", container.base_url())).await?.json().await?;

    assert_eq!(resp["status"], "running");
    assert_eq!(resp["agent"], "claude");
    assert!(resp["terminal"]["cols"].is_number());
    assert!(resp["terminal"]["rows"].is_number());

    Ok(())
}

#[tokio::test]
async fn docker_agent_state_transitions() -> anyhow::Result<()> {
    skip_unless_docker!();

    let container = DockerContainer::start("claude_hello.toml")?;
    container.wait_healthy(Duration::from_secs(30)).await?;

    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/agent", container.base_url());

    // Poll until we see a non-initializing state
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_meaningful_state = false;

    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                let state = body["state"].as_str().unwrap_or("");
                if state != "initializing" && !state.is_empty() {
                    saw_meaningful_state = true;
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    assert!(saw_meaningful_state, "never saw a non-initializing agent state");

    Ok(())
}

#[tokio::test]
async fn docker_screen_endpoint() -> anyhow::Result<()> {
    skip_unless_docker!();

    let container = DockerContainer::start("claude_hello.toml")?;
    container.wait_healthy(Duration::from_secs(30)).await?;

    let resp: serde_json::Value =
        reqwest::get(format!("{}/api/v1/screen", container.base_url())).await?.json().await?;

    assert!(resp["cols"].is_number());
    assert!(resp["rows"].is_number());
    assert!(resp["lines"].is_array());

    Ok(())
}

#[tokio::test]
async fn docker_websocket_connects() -> anyhow::Result<()> {
    skip_unless_docker!();

    let container = DockerContainer::start("claude_hello.toml")?;
    container.wait_healthy(Duration::from_secs(30)).await?;

    let ws_url = format!("ws://localhost:{}/ws?subscribe=state", container.port);
    let (mut stream, _) = tokio_tungstenite::connect_async(&ws_url).await?;

    // We should receive at least one message (initial state snapshot)
    let msg = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for ws message"))?
        .ok_or_else(|| anyhow::anyhow!("ws stream ended"))??;

    assert!(msg.is_text() || msg.is_binary(), "expected text or binary ws message, got: {msg:?}");

    Ok(())
}
