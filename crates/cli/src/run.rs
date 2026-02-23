// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Top-level session runner — shared by `main` and integration tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use tracing_subscriber::EnvFilter;

use crate::backend::adapter::{AdapterSpec, TmuxBackend};
use crate::backend::spawn::NativePty;
use crate::backend::Backend;
use crate::config::{self, Config, GroomLevel};
use crate::driver::claude::resume;
use crate::driver::claude::setup as claude_setup;
use crate::driver::gemini::setup as gemini_setup;
use crate::driver::AgentType;
use crate::driver::{
    build_claude_driver, build_gemini_driver, AgentState, DetectorSinks, DriverContext,
    SessionSetup,
};
use crate::event::InputEvent;
use crate::event_log::EventLog;
use crate::profile::ProfileState;
use crate::record::RecordingState;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::session::{Session, SessionConfig, SessionOutcome};
use crate::start::StartState;
use crate::stop::StopState;
use crate::switch::{SwitchRequest, SwitchState};
use crate::transcript::TranscriptState;
#[cfg(not(debug_assertions))]
use crate::transport::build_router;
#[cfg(debug_assertions)]
use crate::transport::build_router_hot;
use crate::transport::grpc::CoopGrpc;
use crate::transport::state::{
    DetectionInfo, DriverState, LifecycleState, SessionSettings, TerminalState, TransportChannels,
};
use crate::transport::{build_health_router, Store};
use crate::usage::UsageState;

pub struct RunResult {
    pub status: crate::driver::ExitStatus,
    pub store: Arc<Store>,
}

/// A fully-prepared session ready to run.
///
/// Returned by [`prepare`] so callers can access [`Store`]
/// including broadcast channels and the shutdown token.
pub struct PreparedSession {
    pub store: Arc<Store>,
    /// `Option` because `Session::run` takes ownership; after each run we
    /// build a new Session for the next switch iteration.
    session: Option<Session>,
    config: Config,
    /// Receivers that survive across switch iterations.
    consumer_input_rx: mpsc::Receiver<InputEvent>,
    switch_rx: mpsc::Receiver<SwitchRequest>,
}

impl PreparedSession {
    /// Run the session loop to completion, handling credential switches.
    ///
    /// After the agent process exits, coop waits for either a switch request
    /// (restart with new credentials) or a shutdown signal (SIGTERM/SIGINT/API).
    /// Transport connections survive across switches.
    pub async fn run(mut self) -> anyhow::Result<RunResult> {
        loop {
            let session =
                self.session.take().ok_or_else(|| anyhow::anyhow!("no session available"))?;
            let outcome =
                session.run(&self.config, &mut self.consumer_input_rx, &mut self.switch_rx).await?;

            let req = match outcome {
                SessionOutcome::Exit(status) => {
                    // Agent exited — wait for a switch or shutdown.
                    if self.store.lifecycle.shutdown.is_cancelled() {
                        return Ok(RunResult { status, store: self.store });
                    }
                    info!(
                        "agent exited (code={:?}, signal={:?}), awaiting switch or shutdown",
                        status.code, status.signal
                    );
                    let req = tokio::select! {
                        req = self.switch_rx.recv() => match req {
                            Some(req) => req,
                            None => return Ok(RunResult { status, store: self.store }),
                        },
                        _ = self.store.lifecycle.shutdown.cancelled() => {
                            return Ok(RunResult { status, store: self.store });
                        }
                    };
                    req
                }
                SessionOutcome::Switch(req) => req,
            };

            self.execute_switch(&req).await?;
        }
    }

    /// Execute a credential switch: reset store state, prepare a new agent
    /// setup, spawn a new backend, and build a new Session.
    async fn execute_switch(&mut self, request: &SwitchRequest) -> anyhow::Result<()> {
        let agent_enum = self.config.agent_enum()?;

        // 1. Reset Store state for the new session iteration.
        self.store.terminal.reset(self.config.cols, self.config.rows, self.config.ring_size).await;
        self.store.driver.reset().await;
        self.store.ready.store(false, std::sync::atomic::Ordering::Release);

        // 2. Prepare agent setup.
        let working_dir = std::env::current_dir()?;
        let coop_url = format!("http://127.0.0.1:{}", self.config.port.unwrap_or(0));
        let pristine = self.config.groom_level()? == GroomLevel::Pristine;
        let base_settings = self.store.switch.base_settings.as_ref();
        let mcp_config = self.store.switch.mcp_config.as_ref();

        let setup = match agent_enum {
            AgentType::Claude => {
                let log_path = self.store.switch.session_log_path.read().await;
                Some(claude_setup::prepare(
                    &working_dir,
                    &coop_url,
                    base_settings,
                    mcp_config,
                    pristine,
                    log_path.as_deref(),
                )?)
            }
            AgentType::Gemini => {
                Some(gemini_setup::prepare(&coop_url, base_settings, mcp_config, pristine)?)
            }
            _ => None,
        };

        // 4. Build command with extra args.
        let mut command = self.config.command.clone();
        if let Some(ref s) = setup {
            command.extend(s.extra_args.clone());
        }

        // 5. Merge credential env vars from request.
        let mut env_vars: Vec<(String, String)> =
            setup.as_ref().map(|s| s.env_vars.clone()).unwrap_or_default();
        if let Some(ref creds) = request.credentials {
            for (k, v) in creds {
                if let Some(existing) = env_vars.iter_mut().find(|(ek, _)| ek == k) {
                    existing.1 = v.clone();
                } else {
                    env_vars.push((k.clone(), v.clone()));
                }
            }

            // 5a. Write credentials file — Claude reads from ~/.claude/.credentials.json.
            // Check both CLAUDE_CODE_OAUTH_TOKEN and ANTHROPIC_API_KEY.
            if agent_enum == AgentType::Claude {
                let token =
                    creds.get("CLAUDE_CODE_OAUTH_TOKEN").or_else(|| creds.get("ANTHROPIC_API_KEY"));
                if let Some(token) = token {
                    if !token.is_empty() {
                        if let Err(e) = claude_setup::write_credentials_file(token) {
                            error!("failed to write OAuth credentials file: {e}");
                        }
                    }
                }
            }
        }

        // 6. Build driver (detectors only — encoders already on SessionSettings).
        let sinks = || {
            DetectorSinks::default()
                .with_last_message(Arc::clone(&self.store.driver.last_message))
                .with_hook_tx(self.store.channels.hook_tx.clone())
                .with_message_tx(self.store.channels.message_tx.clone())
                .with_usage(Arc::clone(&self.store.usage))
        };
        let driver = match agent_enum {
            AgentType::Claude => build_claude_driver(&self.config, setup.as_ref(), 0, sinks())?,
            AgentType::Gemini => build_gemini_driver(
                &self.config,
                setup.as_ref(),
                self.store.terminal.child_pid_fn(),
                self.store.terminal.ring_total_written_fn(),
                sinks(),
            )?,
            _ => DriverContext {
                nudge_encoder: None,
                respond_encoder: None,
                detectors: vec![],
                option_parser: None,
            },
        };

        // Add Tier 5 screen detector for Claude.
        let mut detectors = driver.detectors;
        if agent_enum == AgentType::Claude {
            detectors.push(Box::new(crate::driver::claude::screen::ClaudeScreenDetector::new(
                &self.config,
                self.store.terminal.snapshot_fn(),
            )));
            detectors.sort_by_key(|d| d.tier());
        }

        // 7. Spawn new backend.
        if command.is_empty() {
            anyhow::bail!("no command specified for switch");
        }
        let backend = NativePty::spawn(&command, self.config.cols, self.config.rows, &env_vars)?
            .with_reap_interval(self.config.reap_poll());

        // 8. Build new session config and Session.
        let shutdown = self.store.lifecycle.shutdown.clone();
        let mut session_config = SessionConfig::new(Arc::clone(&self.store), backend)
            .with_detectors(detectors)
            .with_shutdown(shutdown);
        if let Some(parser) = driver.option_parser {
            session_config = session_config.with_option_parser(parser);
        }
        self.session = Some(Session::new(&self.config, session_config));

        // 9. Update session log path and session ID for the next switch.
        if let Some(ref s) = setup {
            let mut log_path = self.store.switch.session_log_path.write().await;
            *log_path = s.session_log_path.clone();
            *self.store.session_id.write().await = s.session_id.clone();
        }

        // 10. Track active profile if this switch was profile-triggered.
        if let Some(ref name) = request.profile {
            self.store.profile.set_active(name).await;
        }

        // 11. Broadcast Starting transition.
        let cause = if request.credentials.is_some() { "switch" } else { "restart" };
        let last_message = self.store.driver.last_message.read().await.clone();
        let _ = self.store.channels.state_tx.send(crate::event::TransitionEvent {
            prev: AgentState::Restarting,
            next: AgentState::Starting,
            seq: 0,
            cause: cause.to_owned(),
            last_message,
        });

        info!("session {cause}ed");
        Ok(())
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

    // Priority: --log-level / COOP_LOG_LEVEL > RUST_LOG > default ("info").
    let filter = if std::env::var("COOP_LOG_LEVEL").is_err() && config.log_level == "info" {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level))
    } else {
        EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"))
    };

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
pub async fn prepare(mut config: Config) -> anyhow::Result<PreparedSession> {
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

    // 1b. Resolve port 0 → real port early, before agent setup needs the URL.
    //     Bind immediately so the OS assigns an ephemeral port; we keep the
    //     listener and hand it to axum later (no TOCTOU race).
    let mut pre_bound_http = None;
    if config.port == Some(0) {
        let addr = format!("{}:0", config.host);
        let listener = TcpListener::bind(&addr).await?;
        let real_port = listener.local_addr()?.port();
        config.port = Some(real_port);
        pre_bound_http = Some(listener);
        info!("resolved port 0 → {real_port}");
    }

    // 2. Prepare agent session setup. Each agent's setup module produces a
    //    unified `SessionSetup` containing env vars, CLI args, and paths.
    //    In pristine mode, hooks/FIFO are omitted but Tier 2 paths are kept.
    let working_dir = std::env::current_dir()?;
    let coop_url_for_setup = format!("http://127.0.0.1:{}", config.port.unwrap_or(0));
    let pristine = config.groom_level()? == GroomLevel::Pristine;

    let base_settings = base_settings.as_ref();
    let mcp_config = mcp_config.as_ref();

    let setup: Option<SessionSetup> = match agent_enum {
        AgentType::Claude => Some(claude_setup::prepare(
            &working_dir,
            &coop_url_for_setup,
            base_settings,
            mcp_config,
            pristine,
            resume_log_path.as_deref(),
        )?),
        AgentType::Gemini => {
            Some(gemini_setup::prepare(&coop_url_for_setup, base_settings, mcp_config, pristine)?)
        }
        _ => None,
    };

    // 3. Build the command with extra args from setup.
    let mut command = config.command.clone();
    if let Some(ref s) = setup {
        command.extend(s.extra_args.clone());
    }

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
    //    Create raw broadcast channels early so detectors can capture senders.
    let (hook_tx, _) = broadcast::channel(64);
    let (message_tx, _) = broadcast::channel(64);

    let last_message: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let usage_state = Arc::new(UsageState::new());
    let sinks = || {
        DetectorSinks::default()
            .with_last_message(Arc::clone(&last_message))
            .with_hook_tx(hook_tx.clone())
            .with_message_tx(message_tx.clone())
            .with_usage(Arc::clone(&usage_state))
    };
    let mut driver = match agent_enum {
        AgentType::Claude => {
            let log_start_offset = resume_state.as_ref().map(|s| s.log_offset).unwrap_or(0);
            build_claude_driver(&config, setup.as_ref(), log_start_offset, sinks())?
        }
        AgentType::Gemini => build_gemini_driver(
            &config,
            setup.as_ref(),
            terminal.child_pid_fn(),
            terminal.ring_total_written_fn(),
            sinks(),
        )?,
        AgentType::Unknown => DriverContext {
            nudge_encoder: None,
            respond_encoder: None,
            detectors: crate::driver::unknown::build_detectors(
                &config,
                terminal.child_pid_fn(),
                terminal.ring_total_written_fn(),
                None,
            )?,
            option_parser: None,
        },
        AgentType::Codex => {
            anyhow::bail!("{agent_enum:?} driver is not yet implemented");
        }
    };

    // Tier 5: Claude screen detector for idle prompt detection.
    if agent_enum == AgentType::Claude {
        driver.detectors.push(Box::new(crate::driver::claude::screen::ClaudeScreenDetector::new(
            &config,
            terminal.snapshot_fn(),
        )));
        driver.detectors.sort_by_key(|d| d.tier());
    }

    // 5a. Write credentials file if CLAUDE_CODE_OAUTH_TOKEN env var is set.
    // Claude Code reads OAuth tokens from ~/.claude/.credentials.json rather
    // than the env var, so we bridge the gap on initial startup too.
    if agent_enum == AgentType::Claude {
        if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
            if !token.is_empty() {
                if let Err(e) = claude_setup::write_credentials_file(&token) {
                    error!("failed to write OAuth credentials file on startup: {e}");
                }
            }
        }
    }

    // 6. Spawn backend AFTER driver is built (FIFO must exist before child starts).
    let extra_env = setup.as_ref().map(|s| s.env_vars.as_slice()).unwrap_or(&[]);
    let backend: Box<dyn Backend> = if let Some(ref attach_spec) = config.attach {
        let spec: AdapterSpec = attach_spec.parse()?;
        match spec {
            AdapterSpec::Tmux { session } => {
                Box::new(TmuxBackend::new(session)?.with_poll_interval(config.tmux_poll()))
            }
            AdapterSpec::Screen { session: _ } => {
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
    let (input_tx, consumer_input_rx) = mpsc::channel(256);
    let (output_tx, _) = broadcast::channel(256);
    let (screen_tx, _) = broadcast::channel::<u64>(16);
    let (state_tx, _) = broadcast::channel(64);
    let (prompt_tx, _) = broadcast::channel(64);

    // Switch channel: capacity 1 enforces single-switch-at-a-time.
    let (switch_tx, switch_rx) = mpsc::channel::<SwitchRequest>(1);

    let resolve_url = format!("{coop_url_for_setup}/api/v1/stop/resolve");
    let stop_state = Arc::new(StopState::new(stop_config, resolve_url));
    let start_state = Arc::new(StartState::new(start_config));
    let switch_state = Arc::new(SwitchState {
        switch_tx,
        session_log_path: RwLock::new(setup.as_ref().and_then(|s| s.session_log_path.clone())),
        base_settings: base_settings.cloned(),
        mcp_config: mcp_config.cloned(),
    });
    let transcript_state = Arc::new(TranscriptState::new(
        setup
            .as_ref()
            .map(|s| s.session_dir.join("transcripts"))
            .unwrap_or_else(|| PathBuf::from("/tmp/coop-transcripts")),
        setup.as_ref().and_then(|s| s.session_log_path.clone()),
    )?);

    let session_id = setup
        .as_ref()
        .map(|s| s.session_id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let profile_state = Arc::new(ProfileState::new());
    // Apply --profile mode from CLI/env.
    if let Ok(mode) = config.profile.parse::<crate::profile::ProfileMode>() {
        profile_state.set_mode(mode);
    }

    let event_log = Arc::new(EventLog::new(setup.as_ref().map(|s| s.session_dir.as_path())));

    let record_state = Arc::new(RecordingState::new(
        setup.as_ref().map(|s| s.session_dir.as_path()),
        config.cols,
        config.rows,
    ));

    let store = Arc::new(Store {
        terminal,
        driver: Arc::new(DriverState {
            agent_state: RwLock::new(AgentState::Starting),
            state_seq: AtomicU64::new(0),
            detection: RwLock::new(DetectionInfo { tier: u8::MAX, cause: String::new() }),
            error: RwLock::new(None),
            last_message,
        }),
        channels: TransportChannels {
            input_tx,
            output_tx,
            screen_tx,
            state_tx,
            prompt_tx,
            hook_tx,
            message_tx,
        },
        config: SessionSettings {
            started_at: Instant::now(),
            agent: agent_enum,
            auth_token: config.auth_token.clone(),
            nudge_encoder: driver.nudge_encoder,
            respond_encoder: driver.respond_encoder,
            nudge_timeout: config.nudge_timeout(),
            groom: config.groom_level()?,
        },
        lifecycle: LifecycleState {
            shutdown: shutdown.clone(),
            ws_client_count: AtomicI32::new(0),
            bytes_written: AtomicU64::new(0),
        },
        session_id: RwLock::new(session_id),
        ready: Arc::new(AtomicBool::new(false)),
        input_gate: Arc::new(crate::transport::state::InputGate::new(config.input_delay())),
        stop: stop_state,
        switch: switch_state,
        start: start_state,
        transcript: transcript_state,
        usage: usage_state,
        profile: profile_state,
        input_activity: Arc::new(tokio::sync::Notify::new()),
        event_log: Arc::clone(&event_log),
        record: Arc::clone(&record_state),
        session_dir: setup.as_ref().map(|s| s.session_dir.clone()),
    });

    // Enable recording if --record flag is set.
    if config.record {
        store.record.enable().await;
    }

    // Spawn event log subscriber — persists state/hook events to JSONL files.
    {
        let log = Arc::clone(&event_log);
        let mut state_rx = store.channels.state_tx.subscribe();
        let mut hook_rx = store.channels.hook_tx.subscribe();
        let sd = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = sd.cancelled() => break,
                    event = state_rx.recv() => {
                        match event {
                            Ok(e) => log.push_transition(&e),
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("event log: state subscriber lagged by {n}");
                            }
                            Err(_) => break,
                        }
                    }
                    event = hook_rx.recv() => {
                        match event {
                            Ok(e) => log.push_hook(&e),
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("event log: hook subscriber lagged by {n}");
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }

    // Spawn recording subscriber — captures screen snapshots at semantic events,
    // plus periodic screenshots 30s after the last hook event.
    crate::record::spawn_subscriber(
        Arc::clone(&store.record),
        Arc::clone(&store.terminal),
        &store.channels.state_tx,
        &store.channels.hook_tx,
        shutdown.clone(),
    );

    // Spawn NATS publisher if configured.
    if let Some(ref nats_url) = config.nats_url {
        let nats_auth = crate::transport::nats::NatsAuth {
            token: config.nats_token.clone(),
            user: config.nats_user.clone(),
            password: config.nats_password.clone(),
            creds_path: config.nats_creds.as_deref().map(Into::into),
        };
        let publisher = crate::transport::nats::NatsPublisher::connect(
            nats_url,
            &config.nats_prefix,
            &agent_enum.to_string(),
            &config.label,
            nats_auth,
        )
        .await?;
        let store_ref = Arc::clone(&store);
        let sd = shutdown.clone();
        tokio::spawn(async move {
            publisher.run(&store_ref, sd).await;
        });
    }

    // Spawn HTTP server
    if let Some(port) = config.port {
        #[cfg(debug_assertions)]
        let router = build_router_hot(Arc::clone(&store), config.hot);
        #[cfg(not(debug_assertions))]
        let router = build_router(Arc::clone(&store));
        let listener = match pre_bound_http.take() {
            Some(l) => l,
            None => {
                let addr = format!("{}:{}", config.host, port);
                TcpListener::bind(&addr).await?
            }
        };
        info!("HTTP listening on {}", listener.local_addr()?);
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
        #[cfg(debug_assertions)]
        let router = build_router_hot(Arc::clone(&store), config.hot);
        #[cfg(not(debug_assertions))]
        let router = build_router(Arc::clone(&store));
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
        let grpc = CoopGrpc::new(Arc::clone(&store));
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
        let health_router = build_health_router(Arc::clone(&store));
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

    // Spawn mux self-registration client if configured.
    // Placed after all server binds so we never register a session that isn't
    // reachable, and never orphan a registration if a bind fails.
    {
        let sid = store.session_id.read().await.clone();
        crate::mux_client::spawn_if_configured(
            &sid,
            config.port,
            config.auth_token.as_deref(),
            config.mux_url(),
            &agent_enum.to_string(),
            &config.label,
            shutdown.clone(),
        )
        .await;
    }

    // Spawn signal handler
    {
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();

            // First signal: graceful shutdown
            tokio::select! {
                _ = async {
                    if let Some(ref mut s) = sigterm { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGTERM");
                }
                _ = async {
                    if let Some(ref mut s) = sigint { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGINT");
                }
            }
            sd.cancel();

            // Second signal: force exit
            tokio::select! {
                _ = async {
                    if let Some(ref mut s) = sigterm { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGTERM again, forcing exit");
                }
                _ = async {
                    if let Some(ref mut s) = sigint { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGINT again, forcing exit");
                }
            }
            std::process::exit(130);
        });
    }

    // Build session (but don't run yet — caller may need store first).
    // Session::run borrows the receivers via &mut; PreparedSession retains
    // ownership so they survive across switch iterations.
    let mut session_config = SessionConfig::new(Arc::clone(&store), backend)
        .with_detectors(driver.detectors)
        .with_shutdown(shutdown);
    if let Some(parser) = driver.option_parser {
        session_config = session_config.with_option_parser(parser);
    }
    let session = Session::new(&config, session_config);

    // `setup` is intentionally dropped here — session artifacts live in
    // persistent XDG_STATE_HOME directories, not ephemeral temp dirs.
    drop(setup);

    Ok(PreparedSession { store, session: Some(session), config, consumer_input_rx, switch_rx })
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
