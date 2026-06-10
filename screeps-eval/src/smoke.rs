//! The one-command smoke loop (P0.A6):
//! `server up` → `bootstrap --reset` → `deploy` → `run --ticks K` →
//! summary + gate verdict.
//!
//! Gates are **HARD ZEROS only** (phase-0.md §5 exit criterion 6):
//! 1. deploy failure (the deploy step errors out),
//! 2. zero ticks observed,
//! 3. any console panic line,
//! 4. any console deserialization-failure line.
//!
//! Everything else (CPU, creep counts, error-line counts) is printed but
//! never gates — single-run metric gates are the flake generator ADR
//! 0015 rejects.

use crate::capture::RunArtifacts;
use crate::config::EvalConfig;
use crate::deploy::DeployReport;
use anyhow::Result;

/// Default tick count for a smoke run (~1 minute of game time at the
/// 100 ms smoke tick rate — enough to see spawning + early panics).
pub const SMOKE_TICKS_DEFAULT: u64 = 600;

pub struct SmokeReport {
    pub deploy: DeployReport,
    pub artifacts: RunArtifacts,
    /// Empty = all gates passed.
    pub gate_failures: Vec<String>,
}

/// Run the full smoke loop. Returns `Ok` with the report even when the
/// gates fail — the caller decides the exit code (the CLI exits nonzero
/// on any gate failure). Infrastructure failures (server, bootstrap,
/// deploy, capture) are `Err` as usual.
pub async fn smoke(cfg: &EvalConfig, server_name: &str, ticks: u64) -> Result<SmokeReport> {
    tracing::info!("smoke 1/4: server up");
    crate::docker::up(&cfg.eval).await?;

    tracing::info!("smoke 2/4: bootstrap --reset (fresh world)");
    let outcome = crate::server::bootstrap(cfg, true).await?;
    tracing::info!("bootstrap complete:\n{outcome}");

    tracing::info!("smoke 3/4: deploy (release)");
    let deploy = crate::deploy::deploy(cfg, server_name, false).await?;

    tracing::info!("smoke 4/4: run --ticks {ticks} --scenario smoke");
    let artifacts = crate::capture::run(cfg, ticks, "smoke").await?;

    let gate_failures = artifacts.summary.gate_failures();
    Ok(SmokeReport {
        deploy,
        artifacts,
        gate_failures,
    })
}
