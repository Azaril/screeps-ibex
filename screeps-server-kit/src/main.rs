//! screeps-server-kit CLI — the operator-facing entry point for the
//! GENERIC private-server toolkit: stack lifecycle, world bootstrap,
//! deploy, server-CLI passthrough, tick control.
//!
//! Two users, equal priority: automation built on the library (e.g.
//! this repo's `screeps-ibex-eval` smoke/run loop) and the operator's manual
//! iteration loop. Every subcommand is a thin wrapper over a library
//! function — bot-specific evaluation commands (`smoke`, `run`) live in
//! the consumer crate, which drives the same library.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use screeps_server_kit::config::{KitConfig, TICK_MS_FLOOR, TICK_MS_SMOKE};
use screeps_server_kit::server::{mask_cli_command, CliClient};
use std::io::IsTerminal;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "screeps-server-kit",
    about = "Local private-server toolkit for Screeps bot development: \
             stack lifecycle, world bootstrap, deploy, tick control",
    version
)]
struct Cli {
    /// Path to the credentials file (fixed default: ../.screeps.yaml
    /// next to this crate — the only override; stack settings live in
    /// config/local.yml, see config/local.example.yml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Server entry name in .screeps.yaml the kit acts as (default:
    /// the first bots: entry from config/local.yml, falling back to
    /// "private-server")
    #[arg(long, global = true)]
    server_name: Option<String>,

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
    /// Reset/initialize the world: users, passwords, tick rate, spawn
    /// placement — one spawn per configured bots: entry, distinct rooms
    Bootstrap {
        /// Wipe all server data first (system.resetAllData)
        #[arg(long)]
        reset: bool,
    },
    /// Build and upload the bot via screeps-pack (cargo build ->
    /// wasm-bindgen -> glue -> wasm-opt -> upload; first builds are slow)
    Deploy {
        /// Deploy a debug build (cargo build without --release; the dev
        /// wasm-opt profile)
        #[arg(long)]
        debug: bool,
        /// Bot identity to deploy as: a .screeps.yaml servers: entry
        /// (default: --server-name) — selects both the upload
        /// credentials and that entry's per-server build flags
        #[arg(long)]
        user: Option<String>,
    },
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
    /// Live-tail a bot's console output over the websocket (operator view;
    /// no artifacts — see `screeps-ibex-eval run` for recorded capture)
    Console {
        /// Bot identity whose console to tail: a .screeps.yaml servers:
        /// entry (default: --server-name, else the first bots: entry)
        #[arg(long)]
        user: Option<String>,
        /// Stop after this many seconds (default: stream until Ctrl-C)
        #[arg(long)]
        seconds: Option<u64>,
        /// Only print lines containing this substring (case-insensitive),
        /// e.g. --grep ClaimOp
        #[arg(long)]
        grep: Option<String>,
    },
    /// Run a single JavaScript expression once in a bot's runtime console
    /// (`POST /api/user/console`). E.g. restore the feature defaults after a
    /// `_features` shape change: `exec --user ibex-2 "delete Memory._features"`.
    Exec {
        /// Bot identity whose runtime to run in: a .screeps.yaml servers:
        /// entry (default: --server-name, else the first bots: entry)
        #[arg(long)]
        user: Option<String>,
        /// The JavaScript expression to evaluate once in the bot's runtime
        expression: String,
    },
}

#[derive(Subcommand)]
enum ServerAction {
    /// Start the stack (pull images / create / start as needed) and wait
    /// until the game API answers. First boot takes ~10 min (in-container
    /// npm install); warm restarts are fast.
    Up,
    /// Stop the containers; keep containers, network, and volumes
    /// (the next `up` is a warm restart)
    Down,
    /// Remove containers, network, AND volumes — all world data is lost
    Destroy {
        /// Confirm the data loss
        #[arg(long)]
        yes: bool,
    },
    /// Container table (state, health, discovered published ports) +
    /// live game-API and CLI-port probes
    Status,
    /// Build the launcher image from config/local.yml's image.build
    /// context (a full screepers/screeps-launcher clone)
    BuildImage,
    /// Print the launcher container's logs
    Logs {
        /// Keep streaming new output (Ctrl-C to stop)
        #[arg(long, short)]
        follow: bool,
        /// Number of trailing lines to show
        #[arg(long, default_value_t = 100)]
        tail: u32,
    },
}

#[derive(Subcommand)]
enum TickAction {
    /// Set the tick duration in milliseconds (floor: 50 ms)
    Set {
        ms: u64,
    },
    Pause,
    Resume,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // screeps_prospector: progress lines when bootstrap
                // runs with `spawnPlacement: prospector` (P0.P4).
                .unwrap_or_else(|_| "screeps_server_kit=info,screeps_prospector=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Config => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            // Safe by construction: SecretString fields Debug-redact (P0.A7 pin).
            println!("{cfg:#?}");
            Ok(())
        }
        Command::Open => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let url = cfg.server.http_base();
            println!("web client: {url}/");
            // Best-effort launch; printing alone satisfies the command.
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &format!("{url}/")])
                .spawn();
            Ok(())
        }
        Command::Tick { action } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let client = CliClient::new(cfg.stack.cli_port)?;
            let api = screeps_server_kit::api::client(&cfg.server)?;
            match action {
                TickAction::Set { ms } => {
                    if ms < TICK_MS_FLOOR {
                        bail!(
                            "tick {ms} ms is below the {TICK_MS_FLOOR} ms floor \
                             (server/UI cannot keep up)"
                        );
                    }
                    if ms < TICK_MS_SMOKE {
                        eprintln!(
                            "warning: {ms} ms is below the {TICK_MS_SMOKE} ms default — \
                             the server may not keep up"
                        );
                    }
                    let applied = screeps_server_kit::server::set_tick_ms(&client, ms).await?;
                    println!("tick duration: {applied} ms (read back from the server)");
                }
                TickAction::Pause => {
                    screeps_server_kit::server::pause(&client).await?;
                    let time = api.game_time().await.ok().map(|r| r.time);
                    match time {
                        Some(t) => println!("simulation paused at tick {t}"),
                        None => println!("simulation paused"),
                    }
                }
                TickAction::Resume => {
                    screeps_server_kit::server::resume(&client).await?;
                    let time = api.game_time().await.ok().map(|r| r.time);
                    match time {
                        Some(t) => println!("simulation resumed (tick {t})"),
                        None => println!("simulation resumed"),
                    }
                }
            }
            Ok(())
        }
        Command::Server { action } => {
            match action {
                ServerAction::Up => {
                    let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
                    screeps_server_kit::docker::up(&cfg.stack).await?;
                    let report = screeps_server_kit::docker::status(&cfg.stack).await?;
                    println!("{report}");
                }
                ServerAction::Down => screeps_server_kit::docker::down().await?,
                ServerAction::Destroy { yes } => {
                    if !yes {
                        bail!(
                            "`server destroy` removes the containers, the {net} network, and \
                             the volumes — ALL world data is lost. Re-run with --yes to confirm \
                             (use `server down` to stop while keeping data).",
                            net = screeps_server_kit::docker::NETWORK
                        );
                    }
                    screeps_server_kit::docker::destroy().await?;
                }
                ServerAction::Status => {
                    let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
                    let report = screeps_server_kit::docker::status(&cfg.stack).await?;
                    println!("{report}");
                }
                ServerAction::BuildImage => {
                    let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
                    let docker = screeps_server_kit::docker::connect()?;
                    screeps_server_kit::docker::build_launcher_image(&docker, &cfg.stack.image)
                        .await?;
                    println!("built image {}", cfg.stack.image.name);
                }
                ServerAction::Logs { follow, tail } => {
                    screeps_server_kit::docker::logs(follow, tail).await?;
                }
            }
            Ok(())
        }
        Command::Bootstrap { reset } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let outcome = screeps_server_kit::server::bootstrap(&cfg, reset).await?;
            println!("{outcome}");
            println!("web client: {}/", cfg.server.http_base());
            Ok(())
        }
        Command::Cli { command } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let client = CliClient::new(cfg.stack.cli_port)?;
            match command {
                // One-shot: print the (masked) response and exit.
                Some(cmd) => {
                    let body = client.send_raw(&cmd).await?;
                    println!("{}", mask_cli_command(&body));
                    Ok(())
                }
                None => repl(&client).await,
            }
        }
        Command::Deploy { debug, user } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            // --user selects the bot identity; default is the kit's own
            // server entry.
            let entry = user.as_deref().unwrap_or(&cfg.server_name);
            let report = screeps_server_kit::deploy::deploy(&cfg, entry, debug).await?;
            println!("{report}");
            Ok(())
        }
        Command::Console {
            user,
            seconds,
            grep,
        } => {
            // --user picks the identity to tail; otherwise the global
            // --server-name (falling back to the first bots: entry).
            let name = user.as_deref().or(cli.server_name.as_deref());
            let cfg = KitConfig::load(cli.config.as_deref(), name)?;
            screeps_server_kit::console::tail(&cfg, seconds, grep.as_deref()).await
        }
        Command::Exec { user, expression } => {
            let name = user.as_deref().or(cli.server_name.as_deref());
            let cfg = KitConfig::load(cli.config.as_deref(), name)?;
            let api = screeps_server_kit::api::connect(&cfg.server).await?;
            api.console(&expression).await?;
            println!("sent to '{}': {expression}", cfg.server.username);
            Ok(())
        }
    }
}

/// Interactive REPL against the server CLI. Every line that we echo and
/// every response body passes through [`mask_cli_command`] — the vm
/// echoes command source in error stacks, so even responses can carry a
/// typed password (P0.A7(c)).
async fn repl(client: &CliClient) -> Result<()> {
    use std::io::Write;
    use tokio::io::{AsyncBufReadExt, BufReader};

    println!("{}", client.greeting().await?);
    println!("(screeps-server-kit REPL — quit/exit or Ctrl-C to leave; setPassword arguments are masked in echoed output)");

    // When stdin is piped (operator scripting), echo each command so the
    // transcript is readable — masked.
    let echo_input = !std::io::stdin().is_terminal();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print!("screeps> ");
        std::io::stdout().flush()?;
        let line: Option<String> = tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            line = lines.next_line() => line?,
        };
        let Some(line) = line else { break }; // EOF
                                              // PowerShell pipes prepend a UTF-8 BOM to the first line.
        let cmd = line.trim_start_matches('\u{feff}').trim();
        if cmd.is_empty() {
            continue;
        }
        if matches!(cmd, "quit" | "exit" | ".exit") {
            break;
        }
        if echo_input {
            println!("{}", mask_cli_command(cmd));
        }
        match client.send_raw(cmd).await {
            Ok(body) => println!("{}", mask_cli_command(&body)),
            Err(e) => eprintln!("{e:#}"),
        }
    }
    println!();
    Ok(())
}
