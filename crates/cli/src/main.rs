// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use coop::config::{AgentType, Config};
use coop::driver::claude::resume;
use coop::driver::claude::{ClaudeDriver, ClaudeDriverConfig};
use coop::driver::{AgentState, Detector, NudgeEncoder, RespondEncoder};
use coop::pty::attach::{AttachSpec, TmuxBackend};
use coop::pty::spawn::NativePty;
use coop::pty::Backend;
use coop::ring::RingBuffer;
use coop::screen::Screen;
use coop::session::Session;
use coop::transport::grpc::CoopGrpc;
use coop::transport::state::WriteLock;
use coop::transport::{build_health_router, build_router, AppState};

#[tokio::main]
async fn main() {
    let config = Config::parse();

    if let Err(e) = config.validate() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }

    init_tracing(&config);

    match run(config).await {
        Ok(status) => {
            std::process::exit(status.code.unwrap_or(1));
        }
        Err(e) => {
            error!("fatal: {e:#}");
            std::process::exit(1);
        }
    }
}

fn init_tracing(config: &Config) {
    use tracing_subscriber::fmt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    match config.log_format.as_str() {
        "json" => {
            fmt::fmt().with_env_filter(filter).json().init();
        }
        _ => {
            fmt::fmt().with_env_filter(filter).init();
        }
    }
}

async fn run(config: Config) -> anyhow::Result<coop::driver::ExitStatus> {
    let shutdown = CancellationToken::new();

    // Handle --resume: discover session log and build resume state.
    let resume_state = if let Some(ref resume_hint) = config.resume {
        let log_path = resume::discover_session_log(resume_hint)?
            .ok_or_else(|| anyhow::anyhow!("no session log found for: {resume_hint}"))?;
        info!("resuming from session log: {}", log_path.display());
        Some(resume::parse_resume_state(&log_path)?)
    } else {
        None
    };

    // If resuming, append --continue (and optionally --session-id) to the command.
    let mut command = config.command.clone();
    if let Some(ref state) = resume_state {
        let extra = resume::resume_args(state);
        command.extend(extra);
    }

    // Build backend
    let backend: Box<dyn Backend> = if let Some(ref attach_spec) = config.attach {
        let spec: AttachSpec = attach_spec.parse()?;
        match spec {
            AttachSpec::Tmux { session } => Box::new(TmuxBackend::new(session)?),
            AttachSpec::Screen { session: _ } => {
                anyhow::bail!("screen attach is not yet implemented");
            }
        }
    } else {
        if command.is_empty() {
            anyhow::bail!("no command specified");
        }
        Box::new(NativePty::spawn(&command, config.cols, config.rows)?)
    };

    // Shared atomics created early so closures can reference them.
    let child_pid = Arc::new(AtomicU32::new(0));
    let ring_total_written = Arc::new(AtomicU64::new(0));

    // Build driver (detectors + encoders)
    let agent_type_enum = config.agent_type_enum()?;
    let log_start_offset = resume_state.as_ref().map(|s| s.log_offset).unwrap_or(0);
    let pid_for_driver = Arc::clone(&child_pid);
    let rtw_for_driver = Arc::clone(&ring_total_written);
    let (nudge_encoder, respond_encoder, detectors) = build_driver(
        &config,
        agent_type_enum,
        Arc::new(move || {
            let v = pid_for_driver.load(std::sync::atomic::Ordering::Relaxed);
            if v == 0 {
                None
            } else {
                Some(v)
            }
        }),
        Arc::new(move || rtw_for_driver.load(std::sync::atomic::Ordering::Relaxed)),
        log_start_offset,
    )?;

    let idle_grace = Duration::from_secs(config.idle_grace);
    let idle_timeout = Duration::from_secs(config.idle_timeout);

    // Create shared channels
    let (input_tx, consumer_input_rx) = mpsc::channel(256);
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);

    let app_state = Arc::new(AppState {
        started_at: Instant::now(),
        agent_type: config.agent_type.clone(),
        screen: Arc::new(RwLock::new(Screen::new(config.cols, config.rows))),
        ring: Arc::new(RwLock::new(RingBuffer::new(config.ring_size))),
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        input_tx,
        output_tx,
        state_tx,
        child_pid,
        exit_status: Arc::new(RwLock::new(None)),
        write_lock: Arc::new(WriteLock::new()),
        ws_client_count: Arc::new(AtomicI32::new(0)),
        bytes_written: AtomicU64::new(0),
        auth_token: config.auth_token.clone(),
        nudge_encoder,
        respond_encoder,
        shutdown: shutdown.clone(),
        nudge_mutex: Arc::new(tokio::sync::Mutex::new(())),
        ready: Arc::new(AtomicBool::new(false)),
        state_seq: AtomicU64::new(0),
        detection_tier: std::sync::atomic::AtomicU8::new(u8::MAX),
        idle_grace_deadline: Arc::new(std::sync::Mutex::new(None)),
        idle_grace_duration: idle_grace,
        ring_total_written,
    });

    // Spawn HTTP server
    if let Some(port) = config.port {
        let router = build_router(Arc::clone(&app_state));
        let addr = format!("{}:{}", config.host, port);
        let listener = TcpListener::bind(&addr).await?;
        info!("HTTP listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(sd.cancelled_owned())
                .await;
            if let Err(e) = result {
                error!("HTTP server error: {e}");
            }
        });
    }

    // Spawn Unix socket server
    if let Some(ref socket_path) = config.socket {
        let router = build_router(Arc::clone(&app_state));
        let path = socket_path.clone();
        // Remove stale socket
        let _ = std::fs::remove_file(&path);
        let uds_listener = tokio::net::UnixListener::bind(&path)?;
        info!("Unix socket listening on {path}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let mut make_svc = router.into_make_service();
            loop {
                tokio::select! {
                    _ = sd.cancelled() => break,
                    accept = uds_listener.accept() => {
                        match accept {
                            Ok((stream, _)) => {
                                // IntoMakeService implements Service<T> for any T
                                let svc_future = <_ as tower::Service<_>>::call(&mut make_svc, ());
                                tokio::spawn(async move {
                                    let Ok(svc) = svc_future.await;
                                    let io = hyper_util::rt::TokioIo::new(stream);
                                    let hyper_svc = hyper_util::service::TowerToHyperService::new(svc);
                                    let _ = hyper_util::server::conn::auto::Builder::new(
                                        hyper_util::rt::TokioExecutor::new(),
                                    )
                                    .serve_connection_with_upgrades(io, hyper_svc)
                                    .await;
                                });
                            }
                            Err(e) => {
                                tracing::debug!("unix socket accept error: {e}");
                            }
                        }
                    }
                }
            }
        });
    }

    // Spawn gRPC server
    if let Some(grpc_port) = config.grpc_port {
        let grpc = CoopGrpc::new(Arc::clone(&app_state));
        let addr = format!("{}:{}", config.host, grpc_port).parse()?;
        info!("gRPC listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = grpc
                .into_router()
                .serve_with_shutdown(addr, sd.cancelled_owned())
                .await;
            if let Err(e) = result {
                error!("gRPC server error: {e}");
            }
        });
    }

    // Spawn health probe
    if let Some(health_port) = config.health_port {
        let health_router = build_health_router(Arc::clone(&app_state));
        let addr = format!("{}:{}", config.host, health_port);
        let listener = TcpListener::bind(&addr).await?;
        info!("health probe listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = axum::serve(listener, health_router)
                .with_graceful_shutdown(sd.cancelled_owned())
                .await;
            if let Err(e) = result {
                error!("health server error: {e}");
            }
        });
    }

    // Spawn signal handler
    {
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();

            tokio::select! {
                _ = async {
                    if let Some(ref mut s) = sigterm { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGTERM");
                    sd.cancel();
                }
                _ = async {
                    if let Some(ref mut s) = sigint { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGINT");
                    sd.cancel();
                }
            }
        });
    }

    // Run session loop
    let session = Session::new(coop::session::SessionConfig {
        backend,
        detectors,
        app_state,
        consumer_input_rx,
        cols: config.cols,
        rows: config.rows,
        idle_grace,
        idle_timeout,
        shutdown,
        skip_startup_prompts: config.effective_skip_startup_prompts(),
    });

    session.run().await
}

type DriverComponents = (
    Option<Arc<dyn NudgeEncoder>>,
    Option<Arc<dyn RespondEncoder>>,
    Vec<Box<dyn Detector>>,
);

fn build_driver(
    config: &Config,
    agent_type: AgentType,
    child_pid_fn: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    ring_total_written_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
    log_start_offset: u64,
) -> anyhow::Result<DriverComponents> {
    match agent_type {
        AgentType::Claude => {
            let driver = ClaudeDriver::new(ClaudeDriverConfig {
                session_log_path: None,
                hook_pipe_path: None,
                stdout_rx: None,
                log_start_offset,
            })?;
            let nudge: Arc<dyn NudgeEncoder> = Arc::new(driver.nudge);
            let respond: Arc<dyn RespondEncoder> = Arc::new(driver.respond);
            let detectors = driver.detectors;
            Ok((Some(nudge), Some(respond), detectors))
        }
        AgentType::Unknown => {
            // Unknown driver: no nudge/respond, minimal detectors
            let detectors = if let Some(ref agent_config) = config.agent_config {
                coop::driver::unknown::build_detectors(
                    child_pid_fn,
                    ring_total_written_fn,
                    Some(agent_config.as_path()),
                    None,
                )?
            } else {
                coop::driver::unknown::build_detectors(
                    child_pid_fn,
                    ring_total_written_fn,
                    None,
                    None,
                )?
            };
            Ok((None, None, detectors))
        }
        AgentType::Codex | AgentType::Gemini => {
            anyhow::bail!("{agent_type:?} driver is not yet implemented");
        }
    }
}
