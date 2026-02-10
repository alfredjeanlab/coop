// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! `coop attach` — interactive terminal client for a running coop server.
//!
//! Connects to a coop server via WebSocket, puts the local terminal in raw
//! mode, and proxies I/O between the user's terminal and the remote session.
//! Detach with Ctrl+] (0x1d).
//!
//! When a statusline is configured (via `--statusline-cmd` or the default
//! built-in), the bottom row of the terminal is reserved for a status bar
//! using DECSTBM scroll region margins.

use std::io::Write;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use base64::Engine;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use nix::sys::termios;
use tokio::sync::mpsc;

use crate::screen;
use crate::transport::ws::{ClientMessage, ServerMessage};

/// CLI arguments for `coop attach`.
#[derive(Debug, Parser)]
#[command(
    name = "coop-attach",
    about = "Attach an interactive terminal to a running coop server.\nDetach with Ctrl+]."
)]
struct AttachArgs {
    /// Server URL (e.g. http://127.0.0.1:8080).
    #[arg(env = "COOP_URL")]
    url: Option<String>,

    /// Unix socket path for local connection.
    #[arg(long, env = "COOP_SOCKET")]
    socket: Option<String>,

    /// Auth token for the coop server.
    #[arg(long, env = "COOP_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Disable the statusline.
    #[arg(long)]
    no_statusline: bool,

    /// Shell command for statusline content (default: built-in).
    #[arg(long, env = "COOP_STATUSLINE_CMD")]
    statusline_cmd: Option<String>,

    /// Statusline refresh interval in seconds.
    #[arg(long, env = "COOP_STATUSLINE_INTERVAL", default_value_t = DEFAULT_STATUSLINE_INTERVAL)]
    statusline_interval: u64,

    /// Maximum reconnection attempts (0 = disable).
    #[arg(long, default_value_t = 10)]
    max_reconnects: u32,
}

/// Detach key: Ctrl+] (ASCII 0x1d), same as telnet / docker attach.
const DETACH_KEY: u8 = 0x1d;

/// One-time panic hook installation guard.
static PANIC_HOOK_INSTALLED: Once = Once::new();

/// Saved terminal state for panic-time restoration.
/// Populated when entering raw mode, cleared on drop.
static PANIC_TERMIOS: Mutex<Option<(i32, nix::libc::termios)>> = Mutex::new(None);

/// Default statusline refresh interval in seconds.
const DEFAULT_STATUSLINE_INTERVAL: u64 = 5;

/// Ping keepalive interval.
const PING_INTERVAL: Duration = Duration::from_secs(30);

struct StatuslineConfig {
    /// Shell command to run for statusline content. None = built-in.
    cmd: Option<String>,
    /// Refresh interval.
    interval: Duration,
    /// Whether statusline is enabled at all.
    enabled: bool,
}

impl From<&AttachArgs> for StatuslineConfig {
    fn from(args: &AttachArgs) -> Self {
        Self {
            cmd: args.statusline_cmd.clone(),
            interval: Duration::from_secs(args.statusline_interval),
            enabled: !args.no_statusline,
        }
    }
}

/// Result of a single `connect_and_run` session.
enum SessionResult {
    /// Agent exited normally with a code.
    Exited(i32),
    /// User pressed the detach key.
    Detached,
    /// WebSocket connection was lost.
    Disconnected(String),
}

/// Mutable state tracked across connections (survives reconnects).
struct AttachState {
    agent_state: String,
    cols: u16,
    rows: u16,
    started: Instant,
    /// Byte offset into the output ring for smart replay.
    next_offset: u64,
}

impl AttachState {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            agent_state: "unknown".to_owned(),
            cols,
            rows,
            started: Instant::now(),
            next_offset: 0,
        }
    }

    fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// RAII guard that restores the original terminal attributes on drop.
///
/// Stores a raw fd (stdin) and the original termios state. The fd is valid
/// for the lifetime of the process (stdin never closes), so this is safe.
struct RawModeGuard {
    fd: i32,
    original: termios::Termios,
}

impl RawModeGuard {
    fn enter() -> anyhow::Result<Self> {
        let fd = std::io::stdin().as_raw_fd();
        let borrowed = borrow_fd(fd);
        let original = termios::tcgetattr(borrowed)?;
        let mut raw = original.clone();
        termios::cfmakeraw(&mut raw);
        termios::tcsetattr(borrowed, termios::SetArg::TCSAFLUSH, &raw)?;
        Ok(Self { fd, original })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // Clear the panic hook's termios state — we're restoring normally.
        if let Ok(mut guard) = PANIC_TERMIOS.lock() {
            *guard = None;
        }
        let borrowed = borrow_fd(self.fd);
        let _ = termios::tcsetattr(borrowed, termios::SetArg::TCSAFLUSH, &self.original);
    }
}

/// Create a `BorrowedFd` from a raw fd that we know is valid.
fn borrow_fd(fd: i32) -> BorrowedFd<'static> {
    // SAFETY: stdin fd 0 is valid for the lifetime of the process.
    #[allow(unsafe_code)]
    unsafe {
        BorrowedFd::borrow_raw(fd)
    }
}

fn terminal_size() -> Option<(u16, u16)> {
    let fd = std::io::stdout().as_raw_fd();
    let mut ws = nix::libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ ioctl reads terminal size into a winsize struct.
    // The fd is stdout which is valid, and ws is a properly-initialized stack
    // variable with the correct layout for this ioctl.
    #[allow(unsafe_code)]
    let ret = unsafe { nix::libc::ioctl(fd, nix::libc::TIOCGWINSZ, &mut ws) };
    if ret == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        Some((ws.ws_col, ws.ws_row))
    } else {
        None
    }
}

/// Set the scroll region to rows 1..content_rows (leaving the last row free
/// for the statusline). Moves cursor to home position.
fn set_scroll_region(stdout: &mut std::io::Stdout, content_rows: u16) {
    // ESC[1;Nr — set scroll region. ESC[H — move cursor to home.
    let _ = write!(stdout, "\x1b[1;{content_rows}r\x1b[H");
    let _ = stdout.flush();
}

/// Reset the scroll region to full terminal.
fn reset_scroll_region(stdout: &mut std::io::Stdout) {
    let _ = write!(stdout, "\x1b[r");
    let _ = stdout.flush();
}

/// Render the statusline on the bottom row of the terminal.
fn render_statusline(stdout: &mut std::io::Stdout, content: &str, cols: u16, rows: u16) {
    // Truncate to column width at a valid char boundary.
    let max = cols as usize;
    let truncated =
        if content.len() > max { &content[..content.floor_char_boundary(max)] } else { content };
    // Save cursor, move to last row col 1, reverse video, write padded content, restore.
    let _ = write!(
        stdout,
        "\x1b7\x1b[{rows};1H\x1b[7m{truncated:<width$}\x1b[0m\x1b8",
        width = cols as usize
    );
    let _ = stdout.flush();
}

/// Build the default built-in statusline string.
fn builtin_statusline(state: &AttachState) -> String {
    format!(
        " [coop] {} | {}s | {}x{}",
        state.agent_state,
        state.uptime_secs(),
        state.cols,
        state.rows
    )
}

/// Run a shell command and capture its stdout as a statusline string.
async fn run_statusline_cmd(cmd: &str, state: &AttachState) -> String {
    // Expand template variables.
    let expanded = cmd
        .replace("{state}", &state.agent_state)
        .replace("{cols}", &state.cols.to_string())
        .replace("{rows}", &state.rows.to_string())
        .replace("{uptime}", &state.uptime_secs().to_string());

    let output = tokio::process::Command::new("sh")
        .args(["-c", &expanded])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => format!(" [coop] statusline cmd failed: {cmd}"),
    }
}

/// Run the `coop attach` subcommand. Returns a process exit code.
///
/// `args` contains everything after "attach" on the command line.
///
/// This is async because the attach loop uses tokio for WebSocket I/O,
/// signal handling, and timers. It must be called from within a tokio
/// runtime (e.g. from `#[tokio::main]` in main.rs).
pub async fn run(args: &[String]) -> i32 {
    // Build argv as ["coop-attach", ...args] for clap.
    let argv: Vec<&str> =
        std::iter::once("coop-attach").chain(args.iter().map(|s| s.as_str())).collect();
    let parsed = match AttachArgs::try_parse_from(argv) {
        Ok(a) => a,
        Err(e) => {
            // Clap prints help/version to stdout, errors to stderr.
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };

    if parsed.url.is_none() && parsed.socket.is_none() {
        eprintln!("error: COOP_URL is not set and no URL or --socket argument provided");
        return 2;
    }

    let sl_cfg = StatuslineConfig::from(&parsed);
    attach(
        parsed.url.as_deref(),
        parsed.socket.as_deref(),
        parsed.auth_token.as_deref(),
        &sl_cfg,
        parsed.max_reconnects,
    )
    .await
}

/// Build the WebSocket URL and subscription mode query param.
fn build_ws_url(base_url: &str, sl_enabled: bool) -> String {
    let mode = if sl_enabled { "all" } else { "raw" };
    let base = base_url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/ws?mode={mode}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}/ws?mode={mode}")
    } else {
        format!("ws://{base}/ws?mode={mode}")
    }
}

/// Establish a WebSocket connection over TCP or Unix socket.
async fn connect_ws(
    url: Option<&str>,
    socket: Option<&str>,
    sl_enabled: bool,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    // Unix socket takes priority when both are provided.
    if let Some(_path) = socket {
        // TODO: Unix socket support requires `client_async` with a raw stream.
        // For now, fall through to TCP if a URL is also available.
        if url.is_none() {
            return Err("Unix socket support not yet implemented without a URL fallback".to_owned());
        }
    }

    let base_url = url.ok_or("no URL or socket provided")?;
    let ws_url = build_ws_url(base_url, sl_enabled);
    let (stream, _response) =
        tokio_tungstenite::connect_async(&ws_url).await.map_err(|e| format!("{e}"))?;
    Ok(stream)
}

async fn attach(
    url: Option<&str>,
    socket: Option<&str>,
    auth_token: Option<&str>,
    sl_cfg: &StatuslineConfig,
    max_reconnects: u32,
) -> i32 {
    // Enter raw mode (persists across reconnects).
    let raw_guard = match RawModeGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: failed to enter raw mode: {e}");
            return 1;
        }
    };

    // Install a panic hook (once) to restore the terminal even on unwind.
    {
        let raw_termios: nix::libc::termios = raw_guard.original.clone().into();
        if let Ok(mut guard) = PANIC_TERMIOS.lock() {
            *guard = Some((raw_guard.fd, raw_termios));
        }
    }
    PANIC_HOOK_INSTALLED.call_once(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Ok(mut guard) = PANIC_TERMIOS.lock() {
                if let Some((fd, ref termios)) = *guard {
                    // SAFETY: Restoring terminal attributes in panic hook; fd is
                    // stdin which remains valid for the lifetime of the process.
                    #[allow(unsafe_code)]
                    unsafe {
                        nix::libc::tcsetattr(fd, nix::libc::TCSAFLUSH, termios);
                    }
                    *guard = None;
                }
            }
            prev_hook(info);
        }));
    });

    let mut stdout = std::io::stdout();

    // Determine initial terminal size.
    let (init_cols, init_rows) =
        terminal_size().unwrap_or((screen::DEFAULT_COLS, screen::DEFAULT_ROWS));
    let mut state = AttachState::new(init_cols, init_rows);
    let mut sl_active = sl_cfg.enabled && init_rows > 2;

    // Spawn a blocking thread to read stdin (lives across reconnects).
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        use std::io::Read;
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        let mut buf = [0u8; 4096];
        loop {
            match handle.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // SIGWINCH handler for terminal resize (lives across reconnects).
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).ok();

    let mut attempt: u32 = 0;
    let exit_code;

    loop {
        // Connect WebSocket.
        let ws_stream = match connect_ws(url, socket, sl_cfg.enabled).await {
            Ok(s) => s,
            Err(e) => {
                if attempt == 0 {
                    // First connection failure — no reconnect.
                    reset_scroll_region_if(&mut stdout, sl_active);
                    drop(raw_guard);
                    eprintln!("error: WebSocket connection failed: {e}");
                    return 1;
                }
                // Reconnect failure — treat as disconnected.
                if max_reconnects > 0 && attempt >= max_reconnects {
                    reset_scroll_region_if(&mut stdout, sl_active);
                    drop(raw_guard);
                    eprintln!("\r\ncoop attach: max reconnects reached, giving up.");
                    return 1;
                }
                let backoff = reconnect_backoff(attempt);
                let _ = write!(
                    stdout,
                    "\r\ncoop attach: connection failed, retrying in {:.1}s...\r\n",
                    backoff.as_secs_f64()
                );
                let _ = stdout.flush();
                tokio::time::sleep(backoff).await;
                attempt += 1;
                continue;
            }
        };

        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        // Post-connect handshake: Auth → Resize → Replay → GetAgent.
        if let Some(token) = auth_token {
            let _ = send_msg(&mut ws_tx, &ClientMessage::Auth { token: token.to_owned() }).await;
        }

        if sl_active && state.rows > 2 {
            set_scroll_region(&mut stdout, state.rows - 1);
            let _ = send_msg(
                &mut ws_tx,
                &ClientMessage::Resize { cols: state.cols, rows: state.rows - 1 },
            )
            .await;
        } else {
            let _ =
                send_msg(&mut ws_tx, &ClientMessage::Resize { cols: state.cols, rows: state.rows })
                    .await;
        }

        let _ = send_msg(
            &mut ws_tx,
            &ClientMessage::GetReplay { offset: state.next_offset, limit: None },
        )
        .await;

        if sl_active {
            let _ = send_msg(&mut ws_tx, &ClientMessage::GetAgent {}).await;
            let content = match &sl_cfg.cmd {
                Some(cmd) => run_statusline_cmd(cmd, &state).await,
                None => builtin_statusline(&state),
            };
            render_statusline(&mut stdout, &content, state.cols, state.rows);
        }

        let mut ctx = AttachContext {
            state: &mut state,
            sl_active: &mut sl_active,
            sl_cfg,
            stdin_rx: &mut stdin_rx,
            sigwinch: &mut sigwinch,
            stdout: &mut stdout,
        };
        let result = connect_and_run(&mut ws_tx, &mut ws_rx, &mut ctx).await;

        // Send close frame (best-effort).
        let _ = ws_tx.send(tokio_tungstenite::tungstenite::Message::Close(None)).await;

        match result {
            SessionResult::Exited(code) => {
                exit_code = code;
                break;
            }
            SessionResult::Detached => {
                exit_code = 0;
                break;
            }
            SessionResult::Disconnected(reason) => {
                if max_reconnects == 0 {
                    reset_scroll_region_if(&mut stdout, sl_active);
                    drop(raw_guard);
                    eprintln!("\r\ncoop attach: disconnected: {reason}");
                    return 1;
                }
                attempt += 1;
                if attempt > max_reconnects {
                    reset_scroll_region_if(&mut stdout, sl_active);
                    drop(raw_guard);
                    eprintln!("\r\ncoop attach: max reconnects reached, giving up.");
                    return 1;
                }
                reset_scroll_region_if(&mut stdout, sl_active);
                let backoff = reconnect_backoff(attempt);
                let _ = write!(
                    stdout,
                    "\r\ncoop attach: reconnecting ({attempt}/{max_reconnects}) in {:.1}s...\r\n",
                    backoff.as_secs_f64()
                );
                let _ = stdout.flush();
                tokio::time::sleep(backoff).await;
                continue;
            }
        }
    }

    // Clean up: reset scroll region, restore terminal.
    reset_scroll_region_if(&mut stdout, sl_active);
    drop(raw_guard);
    eprintln!("\r\ndetached from coop session.");
    exit_code
}

/// Compute reconnect backoff: 500ms * 2^attempt, capped at 10s.
fn reconnect_backoff(attempt: u32) -> Duration {
    let ms = 500u64.saturating_mul(1u64 << attempt.min(20));
    Duration::from_millis(ms.min(10_000))
}

fn reset_scroll_region_if(stdout: &mut std::io::Stdout, sl_active: bool) {
    if sl_active {
        reset_scroll_region(stdout);
    }
}

/// Mutable context passed to `connect_and_run`, grouping resources that
/// persist across reconnects.
struct AttachContext<'a> {
    state: &'a mut AttachState,
    sl_active: &'a mut bool,
    sl_cfg: &'a StatuslineConfig,
    stdin_rx: &'a mut mpsc::Receiver<Vec<u8>>,
    sigwinch: &'a mut Option<tokio::signal::unix::Signal>,
    stdout: &'a mut std::io::Stdout,
}

/// Inner event loop for a single WebSocket connection. Returns when the
/// session ends, the user detaches, or the connection is lost.
async fn connect_and_run<WsTx, WsRx>(
    ws_tx: &mut WsTx,
    ws_rx: &mut WsRx,
    ctx: &mut AttachContext<'_>,
) -> SessionResult
where
    WsTx: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
    WsRx: StreamExt<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    // Statusline refresh timer.
    let mut sl_interval = tokio::time::interval(ctx.sl_cfg.interval);
    sl_interval.tick().await; // Consume the immediate first tick.

    // Ping keepalive timer.
    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.tick().await; // Consume the immediate first tick.

    loop {
        tokio::select! {
            // Incoming WebSocket messages.
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(ServerMessage::Output { data, offset, .. })
                            | Ok(ServerMessage::Replay { data, offset, .. }) => {
                                if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&data) {
                                    ctx.state.next_offset = offset + decoded.len() as u64;
                                    let _ = ctx.stdout.write_all(&decoded);
                                    let _ = ctx.stdout.flush();
                                }
                            }
                            Ok(ServerMessage::Exit { code, .. }) => {
                                let exit_code = code.unwrap_or(0);
                                // Drain remaining output with a short deadline.
                                let drain_deadline = tokio::time::Instant::now() + Duration::from_millis(200);
                                while let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) =
                                    tokio::time::timeout_at(drain_deadline, ws_rx.next()).await
                                {
                                    if let Ok(ServerMessage::Output { data, offset, .. } | ServerMessage::Replay { data, offset, .. }) = serde_json::from_str(&text) {
                                        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&data) {
                                            ctx.state.next_offset = offset + decoded.len() as u64;
                                            let _ = ctx.stdout.write_all(&decoded);
                                            let _ = ctx.stdout.flush();
                                        }
                                    }
                                }
                                return SessionResult::Exited(exit_code);
                            }
                            Ok(ServerMessage::Error { code, message }) => {
                                return SessionResult::Disconnected(format!("[{code}] {message}"));
                            }
                            Ok(ServerMessage::Transition { next, .. }) => {
                                ctx.state.agent_state = next;
                                if *ctx.sl_active {
                                    let content = match &ctx.sl_cfg.cmd {
                                        Some(cmd) => run_statusline_cmd(cmd, ctx.state).await,
                                        None => builtin_statusline(ctx.state),
                                    };
                                    render_statusline(ctx.stdout, &content, ctx.state.cols, ctx.state.rows);
                                }
                            }
                            Ok(_) => {}
                            Err(_) => {}
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                        return SessionResult::Disconnected("connection closed".to_owned());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        return SessionResult::Disconnected(format!("{e}"));
                    }
                }
            }

            // Local stdin input.
            data = ctx.stdin_rx.recv() => {
                match data {
                    Some(bytes) => {
                        if let Some(pos) = bytes.iter().position(|&b| b == DETACH_KEY) {
                            if pos > 0 {
                                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes[..pos]);
                                let _ = send_msg(ws_tx, &ClientMessage::SendInputRaw { data: encoded }).await;
                            }
                            return SessionResult::Detached;
                        }
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        if send_msg(ws_tx, &ClientMessage::SendInputRaw { data: encoded }).await.is_err() {
                            return SessionResult::Disconnected("send failed".to_owned());
                        }
                    }
                    None => return SessionResult::Disconnected("stdin closed".to_owned()),
                }
            }

            // Terminal resize.
            _ = async {
                match ctx.sigwinch.as_mut() {
                    Some(s) => { s.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some((cols, rows)) = terminal_size() {
                    ctx.state.cols = cols;
                    ctx.state.rows = rows;

                    let was_active = *ctx.sl_active;
                    *ctx.sl_active = ctx.sl_cfg.enabled && rows > 2;

                    if *ctx.sl_active {
                        reset_scroll_region(ctx.stdout);
                        let content_rows = rows - 1;
                        set_scroll_region(ctx.stdout, content_rows);
                        let _ = send_msg(ws_tx, &ClientMessage::Resize { cols, rows: content_rows }).await;
                        let content = match &ctx.sl_cfg.cmd {
                            Some(cmd) => run_statusline_cmd(cmd, ctx.state).await,
                            None => builtin_statusline(ctx.state),
                        };
                        render_statusline(ctx.stdout, &content, cols, rows);
                    } else {
                        if was_active {
                            reset_scroll_region(ctx.stdout);
                        }
                        let _ = send_msg(ws_tx, &ClientMessage::Resize { cols, rows }).await;
                    }
                }
            }

            // Statusline refresh timer.
            _ = sl_interval.tick(), if *ctx.sl_active => {
                let content = match &ctx.sl_cfg.cmd {
                    Some(cmd) => run_statusline_cmd(cmd, ctx.state).await,
                    None => builtin_statusline(ctx.state),
                };
                render_statusline(ctx.stdout, &content, ctx.state.cols, ctx.state.rows);
            }

            // Ping keepalive.
            _ = ping_interval.tick() => {
                let _ = send_msg(ws_tx, &ClientMessage::Ping {}).await;
            }
        }
    }
}

/// Serialize and send a JSON text message over WebSocket.
async fn send_msg<S>(tx: &mut S, msg: &ClientMessage) -> Result<(), String>
where
    S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
{
    let text = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    tx.send(tokio_tungstenite::tungstenite::Message::Text(text))
        .await
        .map_err(|_| "WebSocket send failed".to_owned())
}

#[cfg(test)]
#[path = "attach_tests.rs"]
mod tests;
