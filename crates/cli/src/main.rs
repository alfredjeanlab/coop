// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::builder::styling::Styles;
use clap::Parser;
use tracing::error;

use coop::config::Config;

/// ANSI 256-color codes matching wok, quench, and oddjobs conventions.
mod colors {
    /// Section headers: pastel cyan / steel blue
    pub const HEADER: u8 = 74;
    /// Commands and literals: light grey
    pub const LITERAL: u8 = 250;
    /// Placeholders and context: medium grey
    pub const CONTEXT: u8 = 245;
}

fn use_color() -> bool {
    if std::env::var("NO_COLOR").is_ok_and(|v| v == "1") {
        return false;
    }
    if std::env::var("COLOR").is_ok_and(|v| v == "1") {
        return true;
    }
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

/// Returns (header, literal, context, reset) ANSI sequences, or empty strings when color is off.
fn color_codes() -> (String, String, String, String) {
    if use_color() {
        (
            format!("\x1b[38;5;{}m", colors::HEADER),
            format!("\x1b[38;5;{}m", colors::LITERAL),
            format!("\x1b[38;5;{}m", colors::CONTEXT),
            "\x1b[0m".to_string(),
        )
    } else {
        (String::new(), String::new(), String::new(), String::new())
    }
}

fn usage() -> String {
    let (_, l, c, r) = color_codes();
    format!(
        "\
{l}coop{r} {c}[OPTIONS]{r} {l}<AGENT>{r}
       {l}coop{r} {l}<SUBCOMMAND>{r}"
    )
}

fn after_help() -> String {
    let (h, l, c, r) = color_codes();
    format!(
        "\
{h}Examples:{r}
  {l}coop --port {c}8080 {l}claude{r}
  {l}coop --port {c}8080 {l}claude --dangerously-skip-permissions{r}
  {l}coop --socket {c}/tmp/coop.sock {l}gemini
  {l}coop attach {c}ws://localhost:8080{r}"
    )
}

fn styles() -> Styles {
    use clap::builder::styling::{Ansi256Color, Color, Style};

    let header = Style::new().fg_color(Some(Color::Ansi256(Ansi256Color(colors::HEADER))));
    let literal = Style::new().fg_color(Some(Color::Ansi256(Ansi256Color(colors::LITERAL))));
    let placeholder = Style::new().fg_color(Some(Color::Ansi256(Ansi256Color(colors::CONTEXT))));

    Styles::styled()
        .header(header)
        .usage(header)
        .literal(literal)
        .valid(literal)
        .placeholder(placeholder)
}

#[derive(Parser)]
#[command(
    name = "coop",
    version,
    about = "Terminal session manager for AI coding agents.",
    styles = styles(),
    override_usage = usage(),
    subcommand_value_name = "SUBCOMMAND",
    after_help = after_help(),
)]
struct Cli {
    #[command(flatten)]
    config: Config,

    #[command(subcommand)]
    subcommand: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Attach an interactive terminal to a running coop server.
    Attach(coop::command::attach::AttachArgs),
    /// Open the web terminal UI in a browser.
    Open(coop::command::open::OpenArgs),
    /// Resolve a stop hook from inside the PTY.
    Send(coop::command::send::SendArgs),
    /// Manage mux credentials (list, seed, reauth).
    Cred(coop::command::cred::CredArgs),
    /// Peek at session screens from the mux.
    Peek(coop::command::peek::PeekArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.subcommand {
        Some(Commands::Attach(args)) => {
            std::process::exit(coop::command::attach::run(args).await);
        }
        Some(Commands::Open(args)) => {
            std::process::exit(coop::command::open::run(&args));
        }
        Some(Commands::Send(args)) => {
            std::process::exit(coop::command::send::run(&args));
        }
        Some(Commands::Cred(args)) => {
            std::process::exit(coop::command::cred::run(&args).await);
        }
        Some(Commands::Peek(args)) => {
            std::process::exit(coop::command::peek::run(&args).await);
        }
        None => {
            let config = cli.config;

            if let Err(e) = config.validate() {
                eprintln!("error: {e}");
                std::process::exit(2);
            }

            match coop::run::run(config).await {
                Ok(result) => {
                    std::process::exit(result.status.code.unwrap_or(1));
                }
                Err(e) => {
                    error!("fatal: {e:#}");
                    std::process::exit(1);
                }
            }
        }
    }
}
