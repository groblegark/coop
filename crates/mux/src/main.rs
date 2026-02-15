// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::{Parser, Subcommand};
use tracing::error;

use coopmux::config::MuxConfig;

#[derive(Parser)]
#[command(name = "coopmux", version, about = "PTY multiplexing proxy for coop instances.")]
struct Cli {
    #[command(flatten)]
    config: MuxConfig,

    /// NATS server URL for credential event publishing (e.g. "nats://nats:4222").
    /// When set, credential events are published to NATS.
    #[arg(long, env = "COOP_MUX_NATS_URL")]
    nats_url: Option<String>,

    /// Auth token for the NATS connection.
    #[arg(long, env = "COOP_MUX_NATS_TOKEN")]
    nats_token: Option<String>,

    /// Subject prefix for NATS credential event publishing (default: "coop").
    #[arg(long, default_value = "coop", env = "COOP_MUX_NATS_PREFIX")]
    nats_prefix: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Open the mux dashboard in a browser.
    Open(OpenArgs),
}

#[derive(clap::Args)]
struct OpenArgs {
    /// Mux server URL. Defaults to COOP_MUX_URL or http://{host}:{port}.
    #[arg(env = "COOP_MUX_URL")]
    url: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Open(args)) => {
            std::process::exit(open_dashboard(&cli.config, &args));
        }
        None => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();

            let nats = cli.nats_url.map(|url| coopmux::NatsConfig {
                url,
                token: cli.nats_token,
                prefix: cli.nats_prefix,
            });
            if let Err(e) = coopmux::run(cli.config, nats).await {
                error!("fatal: {e:#}");
                std::process::exit(1);
            }
        }
    }
}

fn open_dashboard(config: &MuxConfig, args: &OpenArgs) -> i32 {
    let base = if let Some(ref url) = args.url {
        url.trim_end_matches('/').to_owned()
    } else {
        format!("http://{}:{}", config.host, config.port)
    };

    let mut url = format!("{base}/mux");
    if let Some(ref token) = config.auth_token {
        url.push_str(&format!("?token={token}"));
    }

    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "start"
    } else {
        "xdg-open"
    };

    match std::process::Command::new(cmd).arg(&url).spawn() {
        Ok(_) => {
            eprintln!("Opening {url}");
            0
        }
        Err(e) => {
            eprintln!("Failed to open browser: {e}");
            eprintln!("Open manually: {url}");
            1
        }
    }
}
