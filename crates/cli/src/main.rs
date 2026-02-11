// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::Parser;
use tracing::error;

use coop::config::Config;

#[derive(Parser)]
#[command(name = "coop", version, about = "Terminal session manager for AI coding agents.")]
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
