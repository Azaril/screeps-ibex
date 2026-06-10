//! screeps-eval CLI — operator-facing entry point.
//!
//! Two users, equal priority (Phase 0 plan, Workstream A intro): the
//! automation harness and the operator's manual iteration loop. Every
//! subcommand is a thin wrapper over a library function.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use screeps_eval::config::{EvalConfig, DEFAULT_SERVER_NAME, TICK_MS_FLOOR};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "screeps-eval",
    about = "Private-server execution, deployment & evaluation harness for screeps-ibex",
    version
)]
struct Cli {
    /// Path to .screeps.yaml (default: walk up from the current directory)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Server entry name in .screeps.yaml
    #[arg(long, global = true, default_value = DEFAULT_SERVER_NAME)]
    server_name: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage the private-server container stack (launcher + mongo + redis)
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },
    /// Reset/initialize the world: password, tick rate, spawn placement
    Bootstrap {
        /// Wipe all server data first (system.resetAllData)
        #[arg(long)]
        reset: bool,
    },
    /// Build and upload the bot to the private server
    Deploy {
        /// Deploy a debug build (richer logs)
        #[arg(long)]
        debug: bool,
    },
    /// Run for N ticks, capturing console + metrics to runs/
    Run {
        #[arg(long, default_value_t = 200)]
        ticks: u64,
    },
    /// One-shot: server up -> bootstrap --reset -> deploy -> run -> summary
    Smoke,
    /// Interactive passthrough to the server CLI (operator mode)
    Cli {
        /// Single command to send (omit for an interactive REPL)
        command: Option<String>,
    },
    /// Tick-rate control (operator mode)
    Tick {
        #[command(subcommand)]
        action: TickAction,
    },
    /// Print (and try to launch) the web-client URL
    Open,
    /// Show the resolved configuration (secrets redacted by construction)
    Config,
}

#[derive(Subcommand)]
enum ServerAction {
    Up,
    Down,
    /// Down + remove containers, network, and volumes (fresh next Up)
    Destroy,
    Status,
    Logs,
}

#[derive(Subcommand)]
enum TickAction {
    /// Set the tick duration in milliseconds (floor: 50 ms)
    Set { ms: u64 },
    Pause,
    Resume,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "screeps_eval=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Config => {
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            // Safe by construction: SecretString fields Debug-redact (P0.A7 pin).
            println!("{cfg:#?}");
            Ok(())
        }
        Command::Open => {
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            let url = cfg.server.http_base();
            println!("web client: {url}/");
            // Best-effort launch; printing alone satisfies the command.
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &format!("{url}/")])
                .spawn();
            Ok(())
        }
        Command::Tick { action } => {
            if let TickAction::Set { ms } = &action {
                if *ms < TICK_MS_FLOOR {
                    bail!(
                        "tick {ms} ms is below the {TICK_MS_FLOOR} ms floor \
                         (server/UI cannot keep up — plan D-2)"
                    );
                }
            }
            bail!("tick control lands with P0.A3/P0.A8 (server-CLI client)")
        }
        Command::Server { .. } => bail!("server lifecycle lands with P0.A2 (bollard)"),
        Command::Bootstrap { .. } => bail!("bootstrap lands with P0.A3"),
        Command::Deploy { .. } => bail!("deploy lands with P0.A4"),
        Command::Run { .. } => bail!("run/capture lands with P0.A5"),
        Command::Smoke => bail!("smoke lands with P0.A6 (after A2-A5)"),
        Command::Cli { .. } => bail!("CLI passthrough lands with P0.A3/P0.A8"),
    }
}
