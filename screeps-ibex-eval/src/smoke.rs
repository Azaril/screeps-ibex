//! The scenario runner (P1.A3) and the one-command smoke loop (P0.A6):
//! `server up` → `bootstrap --reset` → `deploy` → captured run with the
//! scenario's fault injections → gates + colony-health score.
//!
//! Mechanism comes from `screeps_server_kit`; the GATES are ibex policy
//! ([`crate::gates`]) and are **HARD ZEROS only** (phase-0.md §5 exit
//! criterion 6):
//! 1. deploy failure (the deploy step errors out),
//! 2. zero ticks observed,
//! 3. any console panic line,
//! 4. any console deserialization-failure line.
//!
//! Everything else (CPU, creep counts, error-line counts, the
//! colony-health score) is printed but never gates — single-run metric
//! gates are the flake generator ADR 0015 rejects; scores gate only in
//! the N=9 gate mode.

use anyhow::Result;
use screeps_server_kit::capture::RunArtifacts;
use screeps_server_kit::config::KitConfig;
use screeps_server_kit::deploy::DeployReport;

/// Default tick count for a smoke run (~1 minute of game time at the
/// 100 ms smoke tick rate — enough to see spawning + early panics).
pub const SMOKE_TICKS_DEFAULT: u64 = 600;

pub struct SmokeReport {
    pub deploy: DeployReport,
    pub artifacts: RunArtifacts,
    /// Empty = all gates passed.
    pub gate_failures: Vec<String>,
    /// Colony-health score (P1.A4) — informational on a single run.
    pub health: crate::score::ColonyHealth,
}

/// The classic smoke loop — a thin wrapper over the scenario runner
/// with the built-in smoke scenario.
pub async fn smoke(cfg: &KitConfig, ticks: u64) -> Result<SmokeReport> {
    run_scenario(cfg, &crate::scenario::Scenario::builtin_smoke(ticks)).await
}

/// Run a scenario end to end. Returns `Ok` with the report even when
/// the gates fail — the caller decides the exit code. Infrastructure
/// failures (server, bootstrap, deploy, capture) are `Err` as usual.
pub async fn run_scenario(
    cfg: &KitConfig,
    scenario: &crate::scenario::Scenario,
) -> Result<SmokeReport> {
    let mut spec = crate::gates::capture_spec();
    spec.console_injections = scenario.injections();
    if !spec.console_injections.is_empty() {
        tracing::info!(
            "scenario '{}' carries {} fault injection(s)",
            scenario.name,
            spec.console_injections.len()
        );
    }

    tracing::info!("scenario 1/4: server up");
    screeps_server_kit::docker::up(&cfg.stack).await?;

    tracing::info!("scenario 2/4: bootstrap --reset (fresh world)");
    let outcome = screeps_server_kit::server::bootstrap(cfg, true).await?;
    tracing::info!("bootstrap complete:\n{outcome}");

    tracing::info!("scenario 3/4: deploy (release)");
    let deploy = screeps_server_kit::deploy::deploy(cfg, &cfg.server_name, false).await?;

    tracing::info!(
        "scenario 4/4: run --ticks {} --scenario {}",
        scenario.ticks,
        scenario.name
    );
    let artifacts =
        screeps_server_kit::capture::run(cfg, scenario.ticks, &scenario.name, &spec).await?;

    let gate_failures = artifacts.summary.gate_failures(&spec.markers);

    // Colony-health score (P1.A4): seg-57 blocks + console gate
    // counters; persisted next to the other artifacts as score.json.
    let blocks = read_metrics_blocks(&artifacts.dir);
    let health = crate::score::colony_health(
        &blocks,
        crate::score::GateInputs {
            panic_lines: artifacts.summary.console.panic_lines,
            deser_failure_lines: artifacts.summary.console.deser_failure_lines,
        },
    );
    let score_artifact = crate::score::ScoreArtifact {
        scenario: artifacts.summary.scenario.clone(),
        git_sha: artifacts.summary.git_sha.clone(),
        health: health.clone(),
    };
    match serde_json::to_string_pretty(&score_artifact) {
        Ok(json) => {
            if let Err(e) = std::fs::write(artifacts.dir.join("score.json"), json) {
                tracing::warn!("score.json write failed: {e:#}");
            }
        }
        Err(e) => tracing::warn!("score serialization failed: {e:#}"),
    }

    Ok(SmokeReport {
        deploy,
        artifacts,
        gate_failures,
        health,
    })
}

/// Parse the seg-57 blocks out of a run's `metrics.jsonl`
/// (chronological; samples without a block are skipped).
pub fn read_metrics_blocks(run_dir: &std::path::Path) -> Vec<screeps_ibex_metrics::MetricsBlock> {
    let Ok(raw) = std::fs::read_to_string(run_dir.join("metrics.jsonl")) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|sample| {
            sample
                .get("metrics")
                .and_then(crate::gates::parse_metrics_block)
        })
        .collect()
}
