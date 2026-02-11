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
#[command(name = "coop", version, about = "Terminal session manager for AI coding agents.", styles = styles())]
struct Cli {
    #[command(flatten)]
    config: Config,

    #[command(subcommand)]
    subcommand: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Attach an interactive terminal to a running coop server.
    Attach(coop::attach::AttachArgs),
    /// Resolve a stop hook from inside the PTY.
    Send(coop::send::SendArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.subcommand {
        Some(Commands::Attach(args)) => {
            std::process::exit(coop::attach::run(args).await);
        }
        Some(Commands::Send(args)) => {
            std::process::exit(coop::send::run(&args));
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
