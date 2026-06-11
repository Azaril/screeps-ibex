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
    /// Run a FAULT-INJECTION scenario end to end (P1.A3/A5): the smoke
    /// loop with the scenario's scheduled console injections, plus the
    /// colony-health score. Built-ins: smoke, pressure,
    /// reset-under-pressure; or --file a scenario JSON.
    Scenario {
        /// Built-in scenario name (smoke | pressure |
        /// reset-under-pressure)
        #[arg(long, conflicts_with = "file")]
        name: Option<String>,
        /// Path to a scenario JSON (schema v1)
        #[arg(long)]
        file: Option<PathBuf>,
        /// Observed ticks (built-ins only; files carry their own)
        #[arg(long, default_value_t = 900)]
        ticks: u64,
    },
    /// Compare two runs' score.json (P1.A4 differ): prints deltas,
    /// exits nonzero when the candidate regresses beyond the threshold.
    Compare {
        /// Baseline run directory (containing score.json)
        baseline: PathBuf,
        /// Candidate run directory
        candidate: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // screeps_prospector: progress lines when bootstrap
                // runs with `spawnPlacement: prospector` (P0.P4).
                "screeps_ibex_eval=info,screeps_server_kit=info,screeps_prospector=info".into()
            }),
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
            print_scenario_report("smoke", &report)
        }
        Command::Scenario { name, file, ticks } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let scenario = match (&name, &file) {
                (_, Some(path)) => screeps_ibex_eval::scenario::Scenario::load(path)?,
                (Some(name), None) => screeps_ibex_eval::scenario::Scenario::builtin(name, ticks)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "unknown built-in '{name}' (smoke | pressure | reset-under-pressure)"
                        )
                    })?,
                (None, None) => bail!("pass --name <builtin> or --file <scenario.json>"),
            };
            let report = screeps_ibex_eval::smoke::run_scenario(&cfg, &scenario).await?;
            print_scenario_report(&scenario.name, &report)
        }
        Command::Compare {
            baseline,
            candidate,
        } => {
            let load = |dir: &PathBuf| -> Result<screeps_ibex_eval::score::ScoreArtifact> {
                let raw = std::fs::read_to_string(dir.join("score.json"))?;
                Ok(serde_json::from_str(&raw)?)
            };
            let base = load(&baseline)?;
            let cand = load(&candidate)?;
            let cmp = screeps_ibex_eval::score::compare(&base.health, &cand.health);
            println!(
                "baseline:  {:.4} ({} @ {})",
                cmp.baseline_total, base.scenario, base.git_sha
            );
            println!(
                "candidate: {:.4} ({} @ {})",
                cmp.candidate_total, cand.scenario, cand.git_sha
            );
            println!("delta:     {:+.4}", cmp.delta);
            if cmp.regression {
                bail!(
                    "REGRESSION: total dropped beyond the {} threshold",
                    screeps_ibex_eval::score::REGRESSION_DROP
                );
            }
            println!("verdict:   no regression");
            Ok(())
        }
    }
}

fn print_scenario_report(
    name: &str,
    report: &screeps_ibex_eval::smoke::SmokeReport,
) -> Result<()> {
    println!("deploy:   {}", report.deploy);
    println!("artifacts: {}", report.artifacts.dir.display());
    println!("{}", report.artifacts.summary);
    let h = &report.health;
    println!(
        "health:   total {:.4} (survival {:.0}, cpu {:.3}, econ {:.3}{})",
        h.total,
        h.survival,
        h.cpu_headroom,
        h.econ_growth,
        h.military
            .map(|m| format!(", military {m:.3}"))
            .unwrap_or_else(|| ", military n/a".into())
    );
    if report.gate_failures.is_empty() {
        println!("{name}: PASS (all hard-zero gates green)");
        Ok(())
    } else {
        for failure in &report.gate_failures {
            eprintln!("{name} gate FAILED: {failure}");
        }
        bail!("{name} failed {} gate(s)", report.gate_failures.len());
    }
}
