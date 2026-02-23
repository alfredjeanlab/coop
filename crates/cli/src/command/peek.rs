// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! `coop peek` — view mux session screens from the CLI.
//!
//! Talks to the mux server's session endpoints via `COOP_MUX_URL`.

/// CLI arguments for `coop peek`.
#[derive(Debug, clap::Args)]
pub struct PeekArgs {
    /// Session ID to peek at. Omit to list all sessions.
    pub session: Option<String>,

    /// Show ANSI-colored output (default: true if terminal).
    #[arg(long)]
    pub ansi: Option<bool>,

    /// Show plain text (no ANSI escape codes).
    #[arg(long, conflicts_with = "ansi")]
    pub plain: bool,
}

/// Run the `coop peek` subcommand. Returns a process exit code.
pub async fn run(args: &PeekArgs) -> i32 {
    let mux_url = match std::env::var("COOP_MUX_URL") {
        Ok(u) => u.trim_end_matches('/').to_owned(),
        Err(_) => {
            eprintln!("error: COOP_MUX_URL is not set");
            return 2;
        }
    };

    let mux_token = std::env::var("COOP_MUX_TOKEN").ok();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    match &args.session {
        Some(id) => cmd_screen(&client, &mux_url, mux_token.as_deref(), id, args).await,
        None => cmd_list(&client, &mux_url, mux_token.as_deref()).await,
    }
}

fn apply_auth(req: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    match token {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

async fn cmd_list(client: &reqwest::Client, mux_url: &str, token: Option<&str>) -> i32 {
    let url = format!("{mux_url}/api/v1/sessions");
    let resp = match apply_auth(client.get(&url), token).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if status.is_success() {
        match serde_json::from_str::<Vec<serde_json::Value>>(&text) {
            Ok(sessions) => {
                if sessions.is_empty() {
                    println!("No sessions registered.");
                } else {
                    println!("{:<38} {:<30} {:<12} HEALTH", "SESSION ID", "POD", "STATE");
                    println!("{}", "-".repeat(88));
                    for s in &sessions {
                        let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                        let pod = s
                            .get("metadata")
                            .and_then(|m| m.get("k8s"))
                            .and_then(|k| k.get("pod"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("-");
                        let state =
                            s.get("cached_state").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let failures =
                            s.get("health_failures").and_then(|v| v.as_u64()).unwrap_or(0);
                        let health = if failures == 0 {
                            "ok".to_string()
                        } else {
                            format!("{failures} failures")
                        };
                        println!("{id:<38} {pod:<30} {state:<12} {health}");
                    }
                    println!("\n{} session(s)", sessions.len());
                }
                0
            }
            Err(_) => {
                println!("{text}");
                0
            }
        }
    } else {
        eprintln!("error ({status}): {text}");
        1
    }
}

async fn cmd_screen(
    client: &reqwest::Client,
    mux_url: &str,
    token: Option<&str>,
    session_id: &str,
    args: &PeekArgs,
) -> i32 {
    // Allow partial session ID matching: if input is short, list sessions and find a match.
    let resolved_id = if session_id.len() < 36 {
        match resolve_session_id(client, mux_url, token, session_id).await {
            Ok(id) => id,
            Err(code) => return code,
        }
    } else {
        session_id.to_owned()
    };

    let url = format!("{mux_url}/api/v1/sessions/{resolved_id}/screen");
    let resp = match apply_auth(client.get(&url), token).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        eprintln!("error ({status}): {text}");
        return 1;
    }

    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(screen) => {
            let use_ansi = match (args.plain, args.ansi) {
                (true, _) => false,
                (_, Some(v)) => v,
                _ => std::io::IsTerminal::is_terminal(&std::io::stdout()),
            };

            let lines_key = if use_ansi { "ansi" } else { "lines" };
            if let Some(lines) = screen.get(lines_key).and_then(|v| v.as_array()) {
                for line in lines {
                    if let Some(s) = line.as_str() {
                        println!("{s}");
                    }
                }
            } else if let Some(lines) = screen.get("lines").and_then(|v| v.as_array()) {
                // Fallback to plain lines if ansi not available.
                for line in lines {
                    if let Some(s) = line.as_str() {
                        println!("{s}");
                    }
                }
            } else {
                println!("{text}");
            }

            // Show dimensions in stderr so they don't pollute piped output.
            let cols = screen.get("cols").and_then(|v| v.as_u64()).unwrap_or(0);
            let rows = screen.get("rows").and_then(|v| v.as_u64()).unwrap_or(0);
            if cols > 0 && rows > 0 {
                eprintln!("[{cols}x{rows}]");
            }
            0
        }
        Err(_) => {
            println!("{text}");
            0
        }
    }
}

/// Resolve a partial session ID or pod name to a full session ID.
async fn resolve_session_id(
    client: &reqwest::Client,
    mux_url: &str,
    token: Option<&str>,
    partial: &str,
) -> Result<String, i32> {
    let url = format!("{mux_url}/api/v1/sessions");
    let resp = match apply_auth(client.get(&url), token).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error listing sessions: {e}");
            return Err(1);
        }
    };

    let text = resp.text().await.unwrap_or_default();
    let sessions: Vec<serde_json::Value> = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing sessions: {e}");
            return Err(1);
        }
    };

    let partial_lower = partial.to_lowercase();
    let mut matches: Vec<String> = Vec::new();

    for s in &sessions {
        let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let pod = s
            .get("metadata")
            .and_then(|m| m.get("k8s"))
            .and_then(|k| k.get("pod"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if id.starts_with(&partial_lower) || pod.to_lowercase().contains(&partial_lower) {
            matches.push(id.to_owned());
        }
    }

    match matches.len() {
        0 => {
            eprintln!("error: no session matching '{partial}'");
            Err(1)
        }
        1 => Ok(matches.into_iter().next().unwrap_or_default()),
        n => {
            eprintln!("error: '{partial}' matches {n} sessions:");
            for id in &matches {
                // Find the pod name for this id.
                let pod = sessions
                    .iter()
                    .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(id))
                    .and_then(|s| s.get("metadata"))
                    .and_then(|m| m.get("k8s"))
                    .and_then(|k| k.get("pod"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                eprintln!("  {id}  ({pod})");
            }
            Err(1)
        }
    }
}
