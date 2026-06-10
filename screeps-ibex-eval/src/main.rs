//! screeps-ibex-eval CLI — the thin evaluation-policy entry point (`smoke`,
//! `run`; baselines are `run` with a scenario label).
//!
//! Operator/stack commands (`server`, `bootstrap`, `deploy`, `cli`,
//! `tick`, `open`, `config`) live in the generic `screeps-server-kit`
//! CLI; this binary only adds what is ibex-specific: the gates and the
//! capture spec.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use screeps_server_kit::config::KitConfig;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "screeps-ibex-eval",
    about = "Evaluation harness policy for screeps-ibex: smoke gates + capture runs \
             (mechanism: ../screeps-server-kit)",
    version
)]
struct Cli {
    /// Path to the credentials file (fixed default: ../.screeps.yaml at
    /// the repo root — the only override; stack settings live in
    /// ../screeps-server-kit/config/local.yml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Server entry name in .screeps.yaml the harness acts as (default:
    /// the first bots: entry from the kit's config/local.yml, falling
    /// back to "private-server")
    #[arg(long, global = true)]
    server_name: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// One-shot: server up -> bootstrap --reset -> deploy -> run -> summary.
    /// Exits nonzero on the hard-zero gates (deploy failure, zero ticks,
    /// panic lines, deserialization-failure lines); metrics never gate.
    Smoke {
        #[arg(long, default_value_t = screeps_ibex_eval::smoke::SMOKE_TICKS_DEFAULT)]
        ticks: u64,
    },
    /// Run for N ticks, capturing console + metrics to the repo-root
    /// runs/ tree (baselines: --scenario baseline-N)
    Run {
        #[arg(long, default_value_t = 200)]
        ticks: u64,
        /// Scenario label for the runs/<scenario>-<git-sha>-<stamp>/ dir
        #[arg(long, default_value = "adhoc")]
        scenario: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "screeps_ibex_eval=info,screeps_server_kit=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run { ticks, scenario } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let spec = screeps_ibex_eval::gates::capture_spec();
            let artifacts = screeps_server_kit::capture::run(&cfg, ticks, &scenario, &spec).await?;
            println!("artifacts: {}", artifacts.dir.display());
            println!("{}", artifacts.summary);
            Ok(())
        }
        Command::Smoke { ticks } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let report = screeps_ibex_eval::smoke::smoke(&cfg, ticks).await?;
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
