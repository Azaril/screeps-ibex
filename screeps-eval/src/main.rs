//! screeps-eval CLI — operator-facing entry point.
//!
//! Two users, equal priority (Phase 0 plan, Workstream A intro): the
//! automation harness and the operator's manual iteration loop. Every
//! subcommand is a thin wrapper over a library function.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use screeps_eval::api::GameApi;
use screeps_eval::config::{EvalConfig, DEFAULT_SERVER_NAME, TICK_MS_FLOOR, TICK_MS_SMOKE};
use screeps_eval::server::{mask_cli_command, CliClient};
use std::io::IsTerminal;
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
    /// Build and upload the bot to the private server (wraps
    /// js_tools/deploy.js; the full wasm build runs — first builds are slow)
    Deploy {
        /// Deploy a debug build (deploy.js --mode debug -> wasm-pack --dev)
        #[arg(long)]
        debug: bool,
    },
    /// Run for N ticks, capturing console + metrics to runs/
    Run {
        #[arg(long, default_value_t = 200)]
        ticks: u64,
        /// Scenario label for the runs/<scenario>-<git-sha>-<stamp>/ dir
        #[arg(long, default_value = "adhoc")]
        scenario: String,
    },
    /// One-shot: server up -> bootstrap --reset -> deploy -> run -> summary.
    /// Exits nonzero on the hard-zero gates (deploy failure, zero ticks,
    /// panic lines, deserialization-failure lines); metrics never gate.
    Smoke {
        #[arg(long, default_value_t = screeps_eval::smoke::SMOKE_TICKS_DEFAULT)]
        ticks: u64,
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
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            let client = CliClient::new(cfg.eval.cli_port)?;
            let api = GameApi::new(&cfg.server)?;
            match action {
                TickAction::Set { ms } => {
                    if ms < TICK_MS_FLOOR {
                        bail!(
                            "tick {ms} ms is below the {TICK_MS_FLOOR} ms floor \
                             (server/UI cannot keep up — plan D-2)"
                        );
                    }
                    if ms < TICK_MS_SMOKE {
                        eprintln!(
                            "warning: {ms} ms is below the {TICK_MS_SMOKE} ms smoke default — \
                             the server may not keep up (plan D-2)"
                        );
                    }
                    let applied = screeps_eval::server::set_tick_ms(&client, ms).await?;
                    println!("tick duration: {applied} ms (read back from the server)");
                }
                TickAction::Pause => {
                    screeps_eval::server::pause(&client).await?;
                    let time = api.game_time().await.ok();
                    match time {
                        Some(t) => println!("simulation paused at tick {t}"),
                        None => println!("simulation paused"),
                    }
                }
                TickAction::Resume => {
                    screeps_eval::server::resume(&client).await?;
                    let time = api.game_time().await.ok();
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
                    let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
                    screeps_eval::docker::up(&cfg.eval).await?;
                    let report = screeps_eval::docker::status(&cfg.eval).await?;
                    println!("{report}");
                }
                ServerAction::Down => screeps_eval::docker::down().await?,
                ServerAction::Destroy { yes } => {
                    if !yes {
                        bail!(
                            "`server destroy` removes the containers, the {net} network, and \
                             the volumes — ALL world data is lost. Re-run with --yes to confirm \
                             (use `server down` to stop while keeping data).",
                            net = screeps_eval::docker::NETWORK
                        );
                    }
                    screeps_eval::docker::destroy().await?;
                }
                ServerAction::Status => {
                    let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
                    let report = screeps_eval::docker::status(&cfg.eval).await?;
                    println!("{report}");
                }
                ServerAction::Logs { follow, tail } => {
                    screeps_eval::docker::logs(follow, tail).await?;
                }
            }
            Ok(())
        }
        Command::Bootstrap { reset } => {
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            let outcome = screeps_eval::server::bootstrap(&cfg, reset).await?;
            println!("{outcome}");
            println!("web client: {}/", cfg.server.http_base());
            Ok(())
        }
        Command::Cli { command } => {
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            let client = CliClient::new(cfg.eval.cli_port)?;
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
        Command::Deploy { debug } => {
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            let report = screeps_eval::deploy::deploy(&cfg, &cli.server_name, debug).await?;
            println!("{report}");
            Ok(())
        }
        Command::Run { ticks, scenario } => {
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            let artifacts = screeps_eval::capture::run(&cfg, ticks, &scenario).await?;
            println!("artifacts: {}", artifacts.dir.display());
            println!("{}", artifacts.summary);
            Ok(())
        }
        Command::Smoke { ticks } => {
            let cfg = EvalConfig::load(cli.config.as_deref(), &cli.server_name)?;
            let report = screeps_eval::smoke::smoke(&cfg, &cli.server_name, ticks).await?;
            println!("deploy:   {}", report.deploy);
            println!("artifacts: {}", report.artifacts.dir.display());
            println!("{}", report.artifacts.summary);
            if report.gate_failures.is_empty() {
                println!("smoke: PASS (all hard-zero gates green)");
                Ok(())
            } else {
                for failure in &report.gate_failures {
                    eprintln!("smoke gate FAILED: {failure}");
                }
                bail!("smoke failed {} gate(s)", report.gate_failures.len());
            }
        }
    }
}

/// Interactive REPL against the server CLI (P0.A8). Every line that we
/// echo and every response body passes through [`mask_cli_command`] —
/// the vm echoes command source in error stacks, so even responses can
/// carry a typed password (P0.A7(c)).
async fn repl(client: &CliClient) -> Result<()> {
    use std::io::Write;
    use tokio::io::{AsyncBufReadExt, BufReader};

    println!("{}", client.greeting().await?);
    println!("(screeps-eval REPL — quit/exit or Ctrl-C to leave; setPassword arguments are masked in echoed output)");

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
