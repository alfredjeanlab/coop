// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Top-level session runner — shared by `main` and integration tests.

/// Capacity for the input (keypresses / API writes) mpsc channel.
pub(crate) const INPUT_CHANNEL_CAPACITY: usize = 256;

/// Capacity for the output (PTY bytes) broadcast channel.
pub(crate) const OUTPUT_CHANNEL_CAPACITY: usize = 256;

/// Capacity for the agent-state-change broadcast channel.
pub(crate) const STATE_CHANNEL_CAPACITY: usize = 64;

/// Capacity for the prompt-action broadcast channel.
pub(crate) const PROMPT_CHANNEL_CAPACITY: usize = 64;

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use tracing_subscriber::EnvFilter;

use crate::config::{self, Config, GroomLevel};
use crate::driver::claude::resume;
use crate::driver::claude::setup::{self as claude_setup, ClaudeSessionSetup};
use crate::driver::claude::ClaudeDriver;
use crate::driver::gemini::setup::{self as gemini_setup, GeminiSessionSetup};
use crate::driver::gemini::GeminiDriver;
use crate::driver::process::ProcessMonitor;
use crate::driver::AgentType;
use crate::driver::{AgentState, Detector, NudgeEncoder, OptionParser, RespondEncoder};
use crate::pty::adapter::{AttachSpec, TmuxBackend};
use crate::pty::spawn::NativePty;
use crate::pty::Backend;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::session::Session;
use crate::start::StartState;
use crate::stop::StopState;
use crate::transport::grpc::CoopGrpc;
use crate::transport::state::{
    DetectionInfo, DriverState, LifecycleState, SessionSettings, TerminalState, TransportChannels,
};
use crate::transport::{build_health_router, build_router, AppState};

/// Result of a completed session.
pub struct RunResult {
    pub status: crate::driver::ExitStatus,
    pub app_state: Arc<AppState>,
}

/// A fully-prepared session ready to run.
///
/// Returned by [`prepare`] so callers (e.g. integration tests) can access
/// [`AppState`] — including broadcast channels and the shutdown token — before
/// the blocking session loop starts.
pub struct PreparedSession {
    pub app_state: Arc<AppState>,
    session: Session,
    config: Config,
}

impl PreparedSession {
    /// Run the session loop to completion.
    ///
    /// After the agent process exits, coop stays alive (servers keep running)
    /// until the shutdown token is cancelled by SIGTERM, SIGINT, or a Shutdown
    /// API call.
    pub async fn run(self) -> anyhow::Result<RunResult> {
        let status = self.session.run(&self.config).await?;

        // Agent exited — wait for explicit shutdown signal.
        // Skip if shutdown was already triggered (SIGTERM/SIGINT/idle timeout).
        if !self.app_state.lifecycle.shutdown.is_cancelled() {
            info!(
                "agent exited (code={:?}, signal={:?}), awaiting shutdown",
                status.code, status.signal
            );
            self.app_state.lifecycle.shutdown.cancelled().await;
        }

        Ok(RunResult { status, app_state: self.app_state })
    }
}

/// Run a coop session to completion.
///
/// This is the full production codepath: prepare claude session, build driver,
/// spawn backend, start servers, and run the session loop.
pub async fn run(config: Config) -> anyhow::Result<RunResult> {
    prepare(config).await?.run().await
}

/// Initialize tracing/logging from config.
///
/// Uses `try_init` so it's safe to call multiple times (e.g. from tests).
pub fn init_tracing(config: &Config) {
    use tracing_subscriber::fmt;

    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    let result = match config.log_format.as_str() {
        "json" => fmt::fmt().with_env_filter(filter).json().try_init(),
        _ => fmt::fmt().with_env_filter(filter).try_init(),
    };
    drop(result);
}

/// Prepare a coop session: set up driver, spawn backend, start servers.
///
/// Returns a [`PreparedSession`] whose [`AppState`] is accessible before
/// calling [`PreparedSession::run`] to enter the session loop.
pub async fn prepare(config: Config) -> anyhow::Result<PreparedSession> {
    init_tracing(&config);

    let shutdown = CancellationToken::new();
    let agent_enum = config.agent_enum()?;

    // 0. Load agent config file if provided.
    let agent_file_config = match config.agent_config {
        Some(ref path) => Some(config::load_agent_config(path)?),
        None => None,
    };
    let stop_config = agent_file_config.as_ref().and_then(|c| c.stop.clone()).unwrap_or_default();
    let start_config = agent_file_config.as_ref().and_then(|c| c.start.clone()).unwrap_or_default();
    let base_settings = agent_file_config.as_ref().and_then(|c| c.settings.clone());
    let mcp_config = agent_file_config.as_ref().and_then(|c| c.mcp.clone());

    // 1. Handle --resume: discover session log and build resume state.
    let (resume_state, resume_log_path) = if let Some(ref resume_hint) = config.resume {
        let log_path = resume::discover_session_log(resume_hint)?
            .ok_or_else(|| anyhow::anyhow!("no session log found for: {resume_hint}"))?;
        info!("resuming from session log: {}", log_path.display());
        let state = resume::parse_resume_state(&log_path)?;
        (Some(state), Some(log_path))
    } else {
        (None, None)
    };

    // 2. Prepare agent session setup. In pristine mode, skip FIFO creation and
    //    hook injection but still generate a session-id and log path for Tier 2.
    let working_dir = std::env::current_dir()?;
    // Compute coop_url early so it's available for setup.
    // Uses port 0 as placeholder if no HTTP port configured.
    let coop_url_for_setup = format!("http://127.0.0.1:{}", config.port.unwrap_or(0));
    let pristine = config.groom_level()? == GroomLevel::Pristine;

    let claude_setup: Option<ClaudeSessionSetup> = if agent_enum == AgentType::Claude && !pristine {
        let setup = if let (Some(ref state), Some(ref log_path)) = (&resume_state, &resume_log_path)
        {
            claude_setup::prepare_claude_resume(
                state,
                log_path,
                &coop_url_for_setup,
                base_settings.as_ref(),
            )?
        } else {
            claude_setup::prepare_claude_session(
                &working_dir,
                &coop_url_for_setup,
                base_settings.as_ref(),
            )?
        };
        Some(setup)
    } else {
        None
    };

    // 2b. Prepare Gemini session setup (creates FIFO pipe path, writes settings).
    let gemini_setup: Option<GeminiSessionSetup> = if agent_enum == AgentType::Gemini && !pristine {
        Some(gemini_setup::prepare_gemini_session(
            &working_dir,
            &coop_url_for_setup,
            base_settings.as_ref(),
            mcp_config.as_ref(),
        )?)
    } else {
        None
    };

    // 2c. Pristine extras: session-id + log path (Tier 2) for Claude,
    //     settings/MCP passthrough for any agent — but no FIFO or hooks.
    let (pristine_extra_args, pristine_extra_env, pristine_log_path) = if pristine {
        prepare_pristine_extras(
            agent_enum,
            &working_dir,
            &coop_url_for_setup,
            base_settings.as_ref(),
            mcp_config.as_ref(),
        )?
    } else {
        (vec![], vec![], None)
    };

    // 2d. Write MCP config for Claude (file in session dir + --mcp-config arg).
    // The `mcp` field holds the server map; Claude expects `{"mcpServers": ...}`.
    // (Pristine handles MCP in prepare_pristine_extras instead.)
    if let (Some(ref setup), Some(ref mcp)) = (&claude_setup, &mcp_config) {
        let wrapped = serde_json::json!({ "mcpServers": mcp });
        let mcp_path = setup.session_dir.join("mcp.json");
        std::fs::write(&mcp_path, serde_json::to_string_pretty(&wrapped)?)?;
        info!("wrote Claude MCP config to {}", mcp_path.display());
    }

    // 3. Build the command with extra args from setup.
    let mut command = config.command.clone();
    if let Some(ref setup) = claude_setup {
        command.extend(setup.extra_args.clone());
        // Append --mcp-config if MCP was provided
        if mcp_config.is_some() {
            let mcp_path = setup.session_dir.join("mcp.json");
            command.push("--mcp-config".to_owned());
            command.push(mcp_path.display().to_string());
        }
    }
    if let Some(ref setup) = gemini_setup {
        command.extend(setup.extra_args.clone());
    }
    command.extend(pristine_extra_args);

    // 4. Build terminal state early so driver closures can reference its atomics.
    let terminal = Arc::new(TerminalState {
        screen: RwLock::new(Screen::new(config.cols, config.rows)),
        ring: RwLock::new(RingBuffer::new(config.ring_size)),
        ring_total_written: Arc::new(AtomicU64::new(0)),
        child_pid: AtomicU32::new(0),
        exit_status: RwLock::new(None),
    });

    // 5. Build driver (detectors + encoders). For Claude, uses real paths
    //    from the setup so detectors actually activate.
    let log_start_offset = resume_state.as_ref().map(|s| s.log_offset).unwrap_or(0);
    let pid_terminal = Arc::clone(&terminal);
    let rtw_for_driver = Arc::clone(&terminal.ring_total_written);
    let last_message: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let (nudge_encoder, respond_encoder, mut detectors, option_parser) = build_driver(
        &config,
        agent_enum,
        claude_setup.as_ref(),
        gemini_setup.as_ref(),
        Arc::new(move || {
            let v = pid_terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
            if v == 0 {
                None
            } else {
                Some(v)
            }
        }),
        Arc::new(move || rtw_for_driver.load(std::sync::atomic::Ordering::Relaxed)),
        log_start_offset,
        &last_message,
        pristine_log_path,
    )?;

    // Tier 5: Claude screen detector for idle prompt detection.
    if agent_enum == AgentType::Claude {
        let screen_terminal = Arc::clone(&terminal);
        let snapshot_fn: Arc<dyn Fn() -> crate::screen::ScreenSnapshot + Send + Sync> =
            Arc::new(move || {
                screen_terminal.screen.try_read().map(|s| s.snapshot()).unwrap_or_else(|_| {
                    crate::screen::ScreenSnapshot {
                        lines: vec![],
                        cols: 0,
                        rows: 0,
                        alt_screen: false,
                        cursor: crate::screen::CursorPosition { row: 0, col: 0 },
                        sequence: 0,
                    }
                })
            });
        detectors.push(Box::new(crate::driver::claude::screen::ClaudeScreenDetector::new(
            &config,
            snapshot_fn,
        )));
        detectors.sort_by_key(|d| d.tier());
    }

    // 6. Spawn backend AFTER driver is built (FIFO must exist before child starts).
    let extra_env: Vec<(String, String)> = if pristine {
        pristine_extra_env
    } else {
        claude_setup
            .as_ref()
            .map(|s| s.env_vars.clone())
            .or_else(|| gemini_setup.as_ref().map(|s| s.env_vars.clone()))
            .unwrap_or_default()
    };
    let extra_env = extra_env.as_slice();
    let backend: Box<dyn Backend> = if let Some(ref attach_spec) = config.attach {
        let spec: AttachSpec = attach_spec.parse()?;
        match spec {
            AttachSpec::Tmux { session } => {
                Box::new(TmuxBackend::new(session)?.with_poll_interval(config.tmux_poll()))
            }
            AttachSpec::Screen { session: _ } => {
                anyhow::bail!("screen attach is not yet implemented");
            }
        }
    } else {
        if command.is_empty() {
            anyhow::bail!("no command specified");
        }
        Box::new(
            NativePty::spawn(&command, config.cols, config.rows, extra_env)?
                .with_reap_interval(config.reap_poll()),
        )
    };

    // Create shared channels
    let (input_tx, consumer_input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
    let (output_tx, _) = broadcast::channel(OUTPUT_CHANNEL_CAPACITY);
    let (state_tx, _) = broadcast::channel(STATE_CHANNEL_CAPACITY);
    let (prompt_tx, _) = broadcast::channel(PROMPT_CHANNEL_CAPACITY);

    let resolve_url = format!("{coop_url_for_setup}/api/v1/hooks/stop/resolve");
    let stop_state = Arc::new(StopState::new(stop_config, resolve_url));
    let start_state = Arc::new(StartState::new(start_config));

    let app_state = Arc::new(AppState {
        terminal,
        driver: Arc::new(DriverState {
            agent_state: RwLock::new(AgentState::Starting),
            state_seq: AtomicU64::new(0),
            detection: RwLock::new(DetectionInfo { tier: u8::MAX, cause: String::new() }),
            error: RwLock::new(None),
            last_message,
        }),
        channels: TransportChannels { input_tx, output_tx, state_tx, prompt_tx },
        config: SessionSettings {
            started_at: Instant::now(),
            agent: agent_enum,
            auth_token: config.auth_token.clone(),
            nudge_encoder,
            respond_encoder,
            nudge_timeout: config.nudge_timeout(),
            groom: config.groom_level()?,
        },
        lifecycle: LifecycleState {
            shutdown: shutdown.clone(),
            ws_client_count: AtomicI32::new(0),
            bytes_written: AtomicU64::new(0),
        },
        ready: Arc::new(AtomicBool::new(false)),
        delivery_gate: Arc::new(crate::transport::state::DeliveryGate::new(config.input_delay())),
        stop: stop_state,
        start: start_state,
        input_activity: Arc::new(tokio::sync::Notify::new()),
    });

    // Spawn HTTP server
    if let Some(port) = config.port {
        let router = build_router(Arc::clone(&app_state));
        let addr = format!("{}:{}", config.host, port);
        let listener = TcpListener::bind(&addr).await?;
        info!("HTTP listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result =
                axum::serve(listener, router).with_graceful_shutdown(sd.cancelled_owned()).await;
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
    if let Some(grpc_port) = config.port_grpc {
        let grpc = CoopGrpc::new(Arc::clone(&app_state));
        let addr = format!("{}:{}", config.host, grpc_port).parse()?;
        info!("gRPC listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = grpc.into_router().serve_with_shutdown(addr, sd.cancelled_owned()).await;
            if let Err(e) = result {
                error!("gRPC server error: {e}");
            }
        });
    }

    // Spawn health probe
    if let Some(health_port) = config.port_health {
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

    // Build session (but don't run yet — caller may need app_state first)
    let mut session_config =
        crate::session::SessionConfig::new(Arc::clone(&app_state), backend, consumer_input_rx)
            .with_detectors(detectors)
            .with_shutdown(shutdown);
    if let Some(parser) = option_parser {
        session_config = session_config.with_option_parser(parser);
    }
    let session = Session::new(&config, session_config);

    Ok(PreparedSession { app_state, session, config })
}

type DriverComponents = (
    Option<Arc<dyn NudgeEncoder>>,
    Option<Arc<dyn RespondEncoder>>,
    Vec<Box<dyn Detector>>,
    Option<OptionParser>,
);

// TODO(refactor): group build_driver params into a struct when adding more
#[allow(clippy::too_many_arguments)]
fn build_driver(
    config: &Config,
    agent: AgentType,
    claude_setup: Option<&ClaudeSessionSetup>,
    gemini_setup: Option<&GeminiSessionSetup>,
    child_pid_fn: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    ring_total_written_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
    log_start_offset: u64,
    last_message: &Arc<RwLock<Option<String>>>,
    pristine_log_path: Option<std::path::PathBuf>,
) -> anyhow::Result<DriverComponents> {
    match agent {
        AgentType::Claude => {
            // In pristine mode: no hook pipe (Tier 1), but keep session log (Tier 2).
            let hook_pipe = claude_setup.map(|s| s.hook_pipe_path.as_path());
            let log_path = claude_setup.map(|s| s.session_log_path.clone()).or(pristine_log_path);
            let driver = ClaudeDriver::new(
                config,
                hook_pipe,
                log_path,
                None,
                log_start_offset,
                Some(Arc::clone(last_message)),
            )?;
            let nudge: Arc<dyn NudgeEncoder> = Arc::new(driver.nudge);
            let respond: Arc<dyn RespondEncoder> = Arc::new(driver.respond);
            let detectors = driver.detectors;
            let option_parser: OptionParser =
                Arc::new(crate::driver::claude::screen::parse_options_from_screen);
            Ok((Some(nudge), Some(respond), detectors, Some(option_parser)))
        }
        AgentType::Gemini => {
            let driver =
                GeminiDriver::new(config, gemini_setup.map(|s| s.hook_pipe_path.as_path()), None)?;
            let nudge: Arc<dyn NudgeEncoder> = Arc::new(driver.nudge);
            let respond: Arc<dyn RespondEncoder> = Arc::new(driver.respond);
            let mut detectors = driver.detectors;
            // Tier 4: ProcessMonitor fallback for basic Working/Exited detection
            detectors.push(Box::new(
                ProcessMonitor::new(child_pid_fn, ring_total_written_fn)
                    .with_poll_interval(config.process_poll()),
            ));
            detectors.sort_by_key(|d| d.tier());
            let option_parser: OptionParser =
                Arc::new(crate::driver::gemini::screen::parse_options_from_screen);
            Ok((Some(nudge), Some(respond), detectors, Some(option_parser)))
        }
        AgentType::Unknown => {
            let detectors = crate::driver::unknown::build_detectors(
                config,
                child_pid_fn,
                ring_total_written_fn,
                None,
            )?;
            Ok((None, None, detectors, None))
        }
        AgentType::Codex => {
            anyhow::bail!("{agent:?} driver is not yet implemented");
        }
    }
}

/// Pristine-mode extras: CLI args, env vars, and optional session log path for Tier 2.
type PristineExtras = (Vec<String>, Vec<(String, String)>, Option<std::path::PathBuf>);

/// Prepare extras for pristine mode: no FIFO, no hooks, but session-id + log
/// path (Tier 2) for Claude and settings/MCP passthrough for any agent.
fn prepare_pristine_extras(
    agent: AgentType,
    working_dir: &std::path::Path,
    coop_url: &str,
    base_settings: Option<&serde_json::Value>,
    mcp_config: Option<&serde_json::Value>,
) -> anyhow::Result<PristineExtras> {
    let mut extra_args = Vec::new();
    let mut extra_env = vec![("COOP_URL".to_string(), coop_url.to_string())];
    let mut session_log_path = None;

    match agent {
        AgentType::Claude => {
            let session_id = uuid::Uuid::new_v4().to_string();
            let log_path = claude_setup::session_log_path(working_dir, &session_id);
            let session_dir = claude_setup::coop_session_dir(&session_id)?;

            extra_args.push("--session-id".to_owned());
            extra_args.push(session_id);

            // Write orchestrator settings as-is (no coop hooks merged).
            if let Some(settings) = base_settings {
                let path = session_dir.join("coop-settings.json");
                std::fs::write(&path, serde_json::to_string_pretty(settings)?)?;
                extra_args.push("--settings".to_owned());
                extra_args.push(path.display().to_string());
            }

            // Write MCP config (same format as non-pristine).
            if let Some(mcp) = mcp_config {
                let wrapped = serde_json::json!({ "mcpServers": mcp });
                let mcp_path = session_dir.join("mcp.json");
                std::fs::write(&mcp_path, serde_json::to_string_pretty(&wrapped)?)?;
                extra_args.push("--mcp-config".to_owned());
                extra_args.push(mcp_path.display().to_string());
            }

            session_log_path = Some(log_path);
        }
        AgentType::Gemini => {
            // Only create a session dir if we need to write files.
            if base_settings.is_some() || mcp_config.is_some() {
                let dir_id = uuid::Uuid::new_v4().to_string();
                let session_dir = claude_setup::coop_session_dir(&dir_id)?;

                // Build settings with MCP embedded (no hooks).
                let mut settings = base_settings.cloned().unwrap_or(serde_json::json!({}));
                if let Some(mcp) = mcp_config {
                    if let Some(obj) = settings.as_object_mut() {
                        obj.insert("mcpServers".to_string(), mcp.clone());
                    }
                }
                let path = session_dir.join("coop-gemini-settings.json");
                std::fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
                extra_env.push((
                    "GEMINI_CLI_SYSTEM_SETTINGS_PATH".to_string(),
                    path.display().to_string(),
                ));
            }
        }
        _ => {}
    }

    Ok((extra_args, extra_env, session_log_path))
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
