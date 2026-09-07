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
        /// Skip `bootstrap --reset` (WS-CLOSE D8: keep the warm world)
        #[arg(long)]
        keep_world: bool,
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
        /// Skip `bootstrap --reset` (WS-CLOSE D8: keep the warm world)
        #[arg(long)]
        keep_world: bool,
    },
    /// H5 sim-vs-server parity oracle (ADR 0006 §B.4): golden-vector
    /// capture (layer 1), the budgeted divergence report (layer 2), the
    /// one-command nightly, and the Docker-free synth/check helpers.
    Parity {
        #[command(subcommand)]
        cmd: ParityCommand,
    },
    /// Compare two runs' score.json (P1.A4 differ): prints deltas,
    /// exits nonzero when the candidate regresses beyond the threshold.
    Compare {
        /// Baseline run directory (containing score.json)
        baseline: PathBuf,
        /// Candidate run directory
        candidate: PathBuf,
    },
    /// Print a run's seg-57 tick series + summary (bucket, trend, governor tier, pathing pools, creeps, faults, vm_starts) from runs/<dir>/metrics.jsonl — the ADR 0004 pressure-ladder evidence extractor.
    Report(screeps_ibex_eval::report::ReportArgs),
}

/// Shared bed flags for `parity capture` / `parity report` / `parity nightly`.
#[derive(clap::Args, Clone)]
struct ParityBedArgs {
    /// Override the catalog entry's room (a NEUTRAL room in the warm world)
    #[arg(long)]
    room: Option<String>,
    /// Override the catalog entry's tick count
    #[arg(long)]
    ticks: Option<u32>,
    /// Wipe the world first (`bootstrap --reset`); the default keeps it (D8)
    #[arg(long)]
    reset_world: bool,
    /// Flag frozen rooms active + restart the stack before seeding
    /// (the neutral-room freeze; the flag is only read at boot)
    #[arg(long)]
    activate_rooms: bool,
    /// Ticks between arming both owners and scenario tick 0
    #[arg(long, default_value_t = screeps_ibex_eval::parity::DEFAULT_LEAD_TICKS)]
    lead_ticks: u32,
    /// Output directory (default: the engine's tests/conformance for
    /// capture, runs/parity-<sha>-<stamp> for report)
    #[arg(long)]
    out: Option<PathBuf>,
}

impl ParityBedArgs {
    fn options(&self) -> screeps_ibex_eval::parity::BedOptions {
        screeps_ibex_eval::parity::BedOptions {
            room: self.room.clone(),
            ticks: self.ticks,
            keep_world: !self.reset_world,
            activate_rooms: self.activate_rooms,
            lead_ticks: self.lead_ticks,
            out: self.out.clone(),
        }
    }
}

#[derive(Subcommand)]
enum ParityCommand {
    /// List the scenario catalog (parity/<name>.json)
    List,
    /// Docker-free: replay a catalog entry through the sim and write it as a
    /// PLACEHOLDER golden vector (provenance says so) into the engine's
    /// tests/conformance/ — replaced by the first `capture`
    Synth {
        /// Catalog entry name(s); default: every entry
        #[arg(long)]
        scenario: Vec<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Docker-free: replay a vector file and print its divergence
    Check { file: PathBuf },
    /// Layer 1: seed the bed into the warm world, arm both owners with the
    /// script, capture per-tick PV1 frames, write the golden vector
    Capture {
        #[arg(long)]
        scenario: String,
        #[command(flatten)]
        bed: ParityBedArgs,
    },
    /// Layer 2: the unscripted bed (driver in trace mode) vs the sim from
    /// first contact, graded against parity/parity-budget.json
    Report {
        #[arg(long)]
        scenario: String,
        /// Grade an existing run directory (console.jsonl) instead of
        /// running the bed
        #[arg(long)]
        run: Option<PathBuf>,
        #[command(flatten)]
        bed: ParityBedArgs,
    },
    /// Every catalog entry's layer-2 report (the operator-scheduled
    /// "nightly"; fails only under a gating budget)
    Nightly {
        #[command(flatten)]
        bed: ParityBedArgs,
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
        Command::Smoke { ticks, keep_world } => {
            let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
            let report = screeps_ibex_eval::smoke::smoke(&cfg, ticks, keep_world).await?;
            print_scenario_report("smoke", &report)
        }
        Command::Parity { cmd } => {
            use screeps_ibex_eval::parity;
            match cmd {
                ParityCommand::List => {
                    for name in parity::catalog_names()? {
                        let v = parity::load_catalog(&name)?;
                        println!(
                            "{name}: room {} · {} creeps · {} structures · {} towers · {} ticks",
                            v.room,
                            v.creeps.len(),
                            v.structures.len(),
                            v.towers.len(),
                            v.tick_count()
                        );
                    }
                    Ok(())
                }
                ParityCommand::Synth { scenario, out } => {
                    let names = if scenario.is_empty() { parity::catalog_names()? } else { scenario };
                    for name in names {
                        let path = parity::synth(&name, out.as_deref())?;
                        println!("synthesized placeholder: {}", path.display());
                    }
                    Ok(())
                }
                ParityCommand::Check { file } => {
                    let d = parity::check_file(&file)?;
                    println!("{}", d.summary());
                    if d.is_zero() {
                        Ok(())
                    } else {
                        bail!("{}: {} delta(s)", file.display(), d.deltas.len())
                    }
                }
                ParityCommand::Capture { scenario, bed } => {
                    let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
                    let path = parity::capture(&cfg, &scenario, &bed.options()).await?;
                    println!("golden vector: {}", path.display());
                    Ok(())
                }
                ParityCommand::Report { scenario, run, bed } => {
                    let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
                    parity::report(&cfg, &scenario, run.as_deref(), &bed.options()).await?;
                    Ok(())
                }
                ParityCommand::Nightly { bed } => {
                    let cfg = KitConfig::load(cli.config.as_deref(), cli.server_name.as_deref())?;
                    let reports = parity::nightly(&cfg, &bed.options()).await?;
                    println!("nightly: {} report(s)", reports.len());
                    Ok(())
                }
            }
        }
        Command::Scenario { name, file, ticks, keep_world } => {
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
            let report = screeps_ibex_eval::smoke::run_scenario(&cfg, &scenario, keep_world).await?;
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
        Command::Report(args) => screeps_ibex_eval::report::run(&args),
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
