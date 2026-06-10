//! Console + metrics capture into `runs/` artifacts (P0.A5) and the
//! `run --ticks N` loop.
//!
//! The console-websocket PROTOCOL (frames, auth/subscribe handshake,
//! payload flattening) lives in the shared `screeps-rest-api` crate
//! (P0.A12) — `screeps_rest_api::socket` pins it with citations. This
//! module owns what is harness-specific: the run loop, the JSONL
//! artifact records, the gate counters, and the summary.
//!
//! ## Bot error markers (the P0.A6 smoke gates pin these)
//!
//! Console line format is `(<LEVEL>) <target>: <message>`
//! (screeps-ibex/src/logging.rs:32). Gate markers:
//! - **panic**: the panic hook formats `PanicHookInfo` Display — the
//!   message contains `panicked at` — and logs it via `log::error!`
//!   (screeps-ibex/src/panic.rs:23-28,55).
//! - **deserialization failure**: `Failed deserialization: <e>`
//!   (screeps-ibex/src/game_loop.rs:533) and `Failed to decode stats
//!   history` (screeps-ibex/src/stats_history.rs:208).
//!   Serialize-side errors (`Failed serialization:` game_loop.rs:424,
//!   `Encode failed:` game_loop.rs:429) are counted under
//!   `error_log_lines` but are not deser-gate markers.
//!
//! ## Metrics sources
//!
//! - `GET /api/game/time` per sample (tick progress).
//! - `GET /api/user/memory-segment?segment=99` — the bot's live stats
//!   JSON (screeps-ibex/src/statssystem.rs:339-351 writes
//!   `{"shard":{"<shard>":{time,gcl,gpl,cpu:{bucket,limit,used},room,market}}}`
//!   to segment 99 every tick). Segment 57 (ADR 0006 metrics segment)
//!   joins when it lands.
//! - Creep count (best-effort) via the server CLI:
//!   `storage.db['rooms.objects'].count({type:'creep',user:<id>})`.

use crate::config::EvalConfig;
use crate::server::CliClient;
use anyhow::{anyhow, bail, Context, Result};
use screeps_rest_api::{console_lines, ConsoleSocket};
use secrecy::SecretString;
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where a console line came from — the shared client's protocol enum,
/// re-exported under the established name (it serializes lowercase into
/// `console.jsonl`, pinned by `console_jsonl_record_shape`).
pub use screeps_rest_api::ConsoleLineKind as ConsoleKind;

/// Metrics sample cadence. At the 100 ms smoke tick rate this is one
/// sample every ~20 ticks; console capture is continuous regardless.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// The bot's live-stats segment (statssystem.rs:340).
pub const STATS_SEGMENT: u8 = 99;

// ===================================================================
// the console.jsonl record
// ===================================================================

/// One `console.jsonl` record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConsoleLine {
    /// Receive time (unix ms).
    pub ts_ms: u64,
    /// Latest game tick sampled via `/api/game/time` at receive time —
    /// approximate (console events carry no tick number; this lags by at
    /// most one [`SAMPLE_INTERVAL`]).
    pub tick: Option<u64>,
    pub kind: ConsoleKind,
    pub line: String,
}

// ===================================================================
// error markers (pure; pinned in the module docs)
// ===================================================================

/// screeps-ibex/src/panic.rs:28 — `PanicHookInfo` Display.
pub const PANIC_MARKER: &str = "panicked at";
/// screeps-ibex/src/game_loop.rs:533 and stats_history.rs:208.
pub const DESER_FAILURE_MARKERS: &[&str] =
    &["Failed deserialization:", "Failed to decode stats history"];

pub fn is_panic_line(line: &str) -> bool {
    line.contains(PANIC_MARKER)
}

pub fn is_deser_failure_line(line: &str) -> bool {
    DESER_FAILURE_MARKERS.iter().any(|m| line.contains(m))
}

/// `(ERROR) <target>: ...` — the fern console format (logging.rs:32).
fn is_error_log_line(line: &str) -> bool {
    line.starts_with("(ERROR)")
}

/// Counters aggregated over a run's console stream.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ConsoleCounters {
    /// Total JSONL records written (non-empty events only).
    pub lines: u64,
    pub log_lines: u64,
    pub result_lines: u64,
    /// `error`-kind events from the engine (thrown errors/aborts).
    pub error_events: u64,
    /// `(ERROR)`-level bot log lines (informational superset).
    pub error_log_lines: u64,
    /// HARD-ZERO gate: lines matching [`PANIC_MARKER`] (any kind).
    pub panic_lines: u64,
    /// HARD-ZERO gate: lines matching [`DESER_FAILURE_MARKERS`].
    pub deser_failure_lines: u64,
}

impl ConsoleCounters {
    pub fn record(&mut self, line: &ConsoleLine) {
        self.lines += 1;
        match line.kind {
            ConsoleKind::Log => {
                self.log_lines += 1;
                if is_error_log_line(&line.line) {
                    self.error_log_lines += 1;
                }
            }
            ConsoleKind::Result => self.result_lines += 1,
            ConsoleKind::Error => self.error_events += 1,
        }
        if is_panic_line(&line.line) {
            self.panic_lines += 1;
        }
        if is_deser_failure_line(&line.line) {
            self.deser_failure_lines += 1;
        }
    }
}

// ===================================================================
// metrics shaping (pure)
// ===================================================================

/// CPU stats as the bot publishes them (statssystem.rs `CpuStats`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CpuSample {
    pub bucket: f64,
    pub limit: f64,
    pub used: f64,
}

/// Extract the CPU block from the seg-99 stats JSON
/// (`{"shard":{"<name>":{"cpu":{bucket,limit,used},...}}}`).
pub fn cpu_from_stats(stats: &Value) -> Option<CpuSample> {
    let shards = stats.get("shard")?.as_object()?;
    let cpu = shards.values().next()?.get("cpu")?;
    Some(CpuSample {
        bucket: cpu.get("bucket")?.as_f64()?,
        limit: cpu.get("limit")?.as_f64()?,
        used: cpu.get("used")?.as_f64()?,
    })
}

/// One `metrics.jsonl` record.
#[derive(Debug, Serialize)]
pub struct MetricsSample {
    pub ts_ms: u64,
    /// `/api/game/time` (None if the poll failed this sample).
    pub tick: Option<u64>,
    /// Extracted from `stats` for convenience.
    pub cpu: Option<CpuSample>,
    /// Own-creep count via the server CLI (best-effort).
    pub creeps: Option<u64>,
    /// Full parsed seg-99 stats (informational; None until the bot
    /// writes the segment).
    pub stats: Option<Value>,
}

// ===================================================================
// summary (pure aggregation + the smoke gates)
// ===================================================================

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CpuSummary {
    pub used_avg: f64,
    pub used_max: f64,
    pub bucket_min: f64,
    pub bucket_last: f64,
    pub limit: f64,
}

pub fn summarize_cpu(samples: &[CpuSample]) -> Option<CpuSummary> {
    let first = samples.first()?;
    let n = samples.len() as f64;
    Some(CpuSummary {
        used_avg: samples.iter().map(|s| s.used).sum::<f64>() / n,
        used_max: samples.iter().map(|s| s.used).fold(f64::MIN, f64::max),
        bucket_min: samples.iter().map(|s| s.bucket).fold(f64::MAX, f64::min),
        bucket_last: samples.last().unwrap_or(first).bucket,
        limit: first.limit,
    })
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CreepSummary {
    pub first: u64,
    pub last: u64,
    pub max: u64,
}

pub fn summarize_creeps(counts: &[u64]) -> Option<CreepSummary> {
    Some(CreepSummary {
        first: *counts.first()?,
        last: *counts.last()?,
        max: counts.iter().copied().max()?,
    })
}

/// `summary.json` — the run's headline record.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub scenario: String,
    pub git_sha: String,
    pub started_at_ms: u64,
    pub wall_seconds: f64,
    pub tick_ms_configured: u64,
    pub tick_first: u64,
    pub tick_last: u64,
    pub ticks_observed: u64,
    pub samples: u64,
    pub console: ConsoleCounters,
    pub cpu: Option<CpuSummary>,
    pub creeps: Option<CreepSummary>,
}

impl Summary {
    /// The P0.A6 smoke gates — **HARD ZEROS only** (phase-0.md §5 exit
    /// criterion 6): zero ticks observed, any panic line, any deser
    /// failure. (Deploy failure gates earlier, in the deploy step.)
    /// Every metric (CPU, creeps, error counts) is informational.
    pub fn gate_failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if self.ticks_observed == 0 {
            failures.push("zero ticks observed (simulation not advancing)".to_string());
        }
        if self.console.panic_lines > 0 {
            failures.push(format!(
                "{} console line(s) matching the panic marker ({PANIC_MARKER:?})",
                self.console.panic_lines
            ));
        }
        if self.console.deser_failure_lines > 0 {
            failures.push(format!(
                "{} console line(s) matching deserialization-failure markers {DESER_FAILURE_MARKERS:?}",
                self.console.deser_failure_lines
            ));
        }
        failures
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "scenario: {} (git {})", self.scenario, self.git_sha)?;
        writeln!(
            f,
            "ticks:    {} -> {} ({} observed) in {:.1} s wall ({} ms/tick configured)",
            self.tick_first,
            self.tick_last,
            self.ticks_observed,
            self.wall_seconds,
            self.tick_ms_configured
        )?;
        writeln!(
            f,
            "console:  {} lines ({} log, {} results), {} error events, {} (ERROR) lines, {} panics, {} deser failures",
            self.console.lines,
            self.console.log_lines,
            self.console.result_lines,
            self.console.error_events,
            self.console.error_log_lines,
            self.console.panic_lines,
            self.console.deser_failure_lines
        )?;
        match &self.cpu {
            Some(cpu) => writeln!(
                f,
                "cpu:      used avg {:.2} / max {:.2} (limit {}), bucket min {} last {}",
                cpu.used_avg, cpu.used_max, cpu.limit, cpu.bucket_min, cpu.bucket_last
            )?,
            None => writeln!(f, "cpu:      (no seg-99 stats seen)")?,
        }
        match &self.creeps {
            Some(c) => write!(f, "creeps:   {} -> {} (max {})", c.first, c.last, c.max)?,
            None => write!(f, "creeps:   (not sampled)")?,
        }
        Ok(())
    }
}

/// Keep run-directory names filesystem- and convention-safe
/// (`<scenario>-<git-sha>-<stamp>`, the F14 fixture scheme).
pub fn sanitize_scenario(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "adhoc".to_string()
    } else {
        cleaned
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Wall-clock safety stop for a run: 10× the nominal tick budget plus
/// slack (the server may run well below the configured rate once creeps
/// exist — plan D-2 honesty note).
fn run_budget(ticks: u64, tick_ms: u64) -> Duration {
    Duration::from_millis(ticks.saturating_mul(tick_ms).saturating_mul(10))
        + Duration::from_secs(120)
}

// ===================================================================
// the run loop (P0.A5: `run --ticks N [--scenario NAME]`)
// ===================================================================

pub struct RunArtifacts {
    pub dir: PathBuf,
    pub summary: Summary,
}

/// Repo root = the directory `.screeps.yaml` was loaded from (where the
/// gitignored `runs/` tree lives).
fn repo_root(cfg: &EvalConfig) -> Result<PathBuf> {
    cfg.source_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("config was not loaded from a file — cannot locate runs/")
}

async fn git_short_sha(repo_root: &Path) -> Result<String> {
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_root)
        .output()
        .await
        .context("running git rev-parse")?;
    if !out.status.success() {
        bail!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Capture console + metrics until `ticks` game ticks elapse, then
/// write `summary.json`. Artifacts land in
/// `runs/<scenario>-<git-sha>-<stamp>/{console.jsonl,metrics.jsonl,summary.json}`.
pub async fn run(cfg: &EvalConfig, ticks: u64, scenario: &str) -> Result<RunArtifacts> {
    if ticks == 0 {
        bail!("--ticks must be at least 1");
    }
    let root = repo_root(cfg)?;
    let sha = git_short_sha(&root).await?;
    let scenario = sanitize_scenario(scenario);
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let dir = root.join("runs").join(format!("{scenario}-{sha}-{stamp}"));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    tracing::info!("run artifacts: {}", dir.display());

    // HTTP sampler client + identity.
    let api = crate::api::connect(&cfg.server).await?;
    let user = api.me().await?;
    // Separate token for the websocket (tokens are rolling — sharing
    // one between the socket auth and the HTTP sampler breaks one side).
    let ws_token = api.fresh_token().await?;

    let tick_first = api.game_time().await?.time;
    let target = tick_first + ticks;
    let started_at_ms = unix_ms();
    let started = Instant::now();
    let budget = run_budget(ticks, cfg.eval.tick_ms);

    // Shared state between the console task and the sampler.
    let last_tick = Arc::new(AtomicU64::new(tick_first));
    let counters: Arc<Mutex<ConsoleCounters>> = Arc::default();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut console_task = tokio::spawn(console_capture_task(
        api.ws_url(),
        ws_token,
        user.id.clone(),
        dir.join("console.jsonl"),
        last_tick.clone(),
        counters.clone(),
        stop_rx,
    ));

    // Metrics sampler (this task).
    let mut metrics_file = std::io::BufWriter::new(
        std::fs::File::create(dir.join("metrics.jsonl")).context("creating metrics.jsonl")?,
    );
    let cli = CliClient::new(cfg.eval.cli_port).ok(); // creep counts: best-effort
    let mut cpu_samples: Vec<CpuSample> = Vec::new();
    let mut creep_counts: Vec<u64> = Vec::new();
    let mut samples = 0u64;
    let mut tick_last = tick_first;
    let mut consecutive_failures = 0u32;
    let mut last_progress = Instant::now();

    loop {
        tokio::time::sleep(SAMPLE_INTERVAL).await;

        // A dead console capture means lost artifacts — surface it now.
        if console_task.is_finished() {
            let err = match (&mut console_task).await {
                Ok(Ok(())) => anyhow!("console socket closed unexpectedly"),
                Ok(Err(e)) => e,
                Err(join) => anyhow!(join),
            };
            return Err(err.context("console capture ended mid-run"));
        }

        let tick = match api.game_time().await {
            Ok(r) => {
                let t = r.time;
                consecutive_failures = 0;
                last_tick.store(t, Ordering::Relaxed);
                tick_last = tick_last.max(t);
                Some(t)
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!("game-time poll failed ({consecutive_failures}x): {e:#}");
                if consecutive_failures >= 10 {
                    bail!("server stopped answering /api/game/time — aborting the run");
                }
                None
            }
        };

        let stats: Option<Value> = match api.memory_segment(STATS_SEGMENT).await {
            Ok(resp) => match resp.data {
                Some(raw) if !raw.is_empty() => serde_json::from_str(&raw).ok(),
                _ => None,
            },
            Err(e) => {
                tracing::warn!("seg-{STATS_SEGMENT} read failed: {e:#}");
                None
            }
        };
        let cpu = stats.as_ref().and_then(cpu_from_stats);
        if let Some(c) = cpu {
            cpu_samples.push(c);
        }

        let creeps = match &cli {
            Some(cli) => cli
                .send(&crate::server::cmd_count_creeps(&user.id))
                .await
                .ok()
                .and_then(|body| body.trim().parse::<u64>().ok()),
            None => None,
        };
        if let Some(c) = creeps {
            creep_counts.push(c);
        }

        let sample = MetricsSample {
            ts_ms: unix_ms(),
            tick,
            cpu,
            creeps,
            stats,
        };
        serde_json::to_writer(&mut metrics_file, &sample).context("writing metrics.jsonl")?;
        metrics_file.write_all(b"\n")?;
        samples += 1;

        if last_progress.elapsed() > Duration::from_secs(15) {
            let done = tick_last.saturating_sub(tick_first);
            tracing::info!(
                "tick {tick_last} / {target} ({done}/{ticks} observed, {:.0}%) — {} console lines, cpu {}",
                done as f64 * 100.0 / ticks as f64,
                counters.lock().map(|c| c.lines).unwrap_or(0),
                cpu.map(|c| format!("{:.1}", c.used)).unwrap_or_else(|| "n/a".into()),
            );
            last_progress = Instant::now();
        }

        if tick_last >= target {
            break;
        }
        if started.elapsed() > budget {
            bail!(
                "run did not reach tick {target} within the {budget:?} safety budget \
                 (at {tick_last}); is the simulation paused or crawling?"
            );
        }
    }
    metrics_file.flush()?;

    // Stop the console task and let it flush.
    let _ = stop_tx.send(true);
    match tokio::time::timeout(Duration::from_secs(10), console_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => tracing::warn!("console capture ended with: {e:#}"),
        Ok(Err(join)) => tracing::warn!("console task panicked: {join}"),
        Err(_) => tracing::warn!("console task did not stop within 10 s"),
    }

    let console = counters.lock().map(|c| c.clone()).unwrap_or_default();
    let summary = Summary {
        scenario,
        git_sha: sha,
        started_at_ms,
        wall_seconds: started.elapsed().as_secs_f64(),
        tick_ms_configured: cfg.eval.tick_ms,
        tick_first,
        tick_last,
        ticks_observed: tick_last.saturating_sub(tick_first),
        samples,
        console,
        cpu: summarize_cpu(&cpu_samples),
        creeps: summarize_creeps(&creep_counts),
    };
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(&summary).context("serializing summary")?,
    )
    .context("writing summary.json")?;
    tracing::info!("summary written: {}", dir.join("summary.json").display());

    Ok(RunArtifacts { dir, summary })
}

// ===================================================================
// console websocket task
// ===================================================================

async fn console_capture_task(
    ws_url: String,
    token: SecretString,
    user_id: String,
    path: PathBuf,
    last_tick: Arc<AtomicU64>,
    counters: Arc<Mutex<ConsoleCounters>>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    // The shared client owns the protocol: greeting frames, the `auth`
    // handshake (the token goes straight into the socket and the
    // `auth ok <fresh-token>` reply is dropped at parse time — P0.A7),
    // the channel subscribe, and ping/pong.
    let mut socket = ConsoleSocket::connect(&ws_url, token, &user_id)
        .await
        .with_context(|| format!("connecting console websocket {ws_url}"))?;
    tracing::info!("console capture live (channel user:{user_id}/console)");

    let mut file = std::io::BufWriter::new(
        std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?,
    );
    let mut since_flush = 0u32;
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
            event = socket.next_event() => {
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        file.flush()?;
                        return Err(e).context("console websocket");
                    }
                };
                if !event.channel.ends_with("/console") {
                    continue;
                }
                let ts_ms = unix_ms();
                let tick = Some(last_tick.load(Ordering::Relaxed));
                for payload_line in console_lines(&event.payload) {
                    let line = ConsoleLine {
                        ts_ms,
                        tick,
                        kind: payload_line.kind,
                        line: payload_line.line,
                    };
                    if let Ok(mut c) = counters.lock() {
                        c.record(&line);
                    }
                    serde_json::to_writer(&mut file, &line)
                        .context("writing console.jsonl")?;
                    file.write_all(b"\n")?;
                    since_flush += 1;
                }
                if since_flush >= 50 {
                    file.flush()?;
                    since_flush = 0;
                }
            }
        }
    }
    file.flush()?;
    Ok(())
}

// ===================================================================
// tests — pure parts against literal fixtures (live shapes 2026-06-09)
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------- the console.jsonl record ----------------
    // (Socket-frame parsing and payload flattening are pinned in the
    // shared screeps-rest-api crate's tests.)

    /// Pins the artifact shape end-to-end, including the lowercase
    /// `kind` from the shared crate's `ConsoleLineKind`.
    #[test]
    fn console_jsonl_record_shape() {
        let line = ConsoleLine {
            ts_ms: 1781061591846,
            tick: Some(7435),
            kind: ConsoleKind::Log,
            line: "(INFO) screeps_ibex: hello".into(),
        };
        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"ts_ms":1781061591846,"tick":7435,"kind":"log","line":"(INFO) screeps_ibex: hello"}"#
        );
    }

    // ---------------- markers + counters ----------------

    /// The panic hook output (panic.rs:28 — std PanicHookInfo Display)
    /// through the fern console format (logging.rs:32).
    #[test]
    fn panic_marker_matches_hook_output() {
        let line = "(ERROR) screeps_ibex::panic: panicked at 'index out of bounds', src/lib.rs:1:1";
        assert!(is_panic_line(line));
        // Modern rustc Display variant (message on the next line).
        assert!(is_panic_line(
            "(ERROR) screeps_ibex::panic: panicked at src/missions/data.rs:66:13:"
        ));
        assert!(!is_panic_line("(INFO) screeps_ibex: spawning hauler"));
    }

    /// game_loop.rs:533 + stats_history.rs:208 are deser-gate markers;
    /// serialize-side failures are NOT (game_loop.rs:424/:429).
    #[test]
    fn deser_markers_match_pinned_sources() {
        assert!(is_deser_failure_line(
            "(ERROR) screeps_ibex::game_loop: Failed deserialization: invalid value"
        ));
        assert!(is_deser_failure_line(
            "(WARN) screeps_ibex::stats_history: Failed to decode stats history, using default: x"
        ));
        assert!(!is_deser_failure_line(
            "(ERROR) screeps_ibex::game_loop: Failed serialization: oops"
        ));
        assert!(!is_deser_failure_line(
            "(ERROR) screeps_ibex::game_loop: Encode failed: oops"
        ));
    }

    #[test]
    fn counters_classify_lines() {
        let mut c = ConsoleCounters::default();
        let mk = |kind, line: &str| ConsoleLine {
            ts_ms: 0,
            tick: None,
            kind,
            line: line.into(),
        };
        c.record(&mk(ConsoleKind::Log, "(INFO) a: fine"));
        c.record(&mk(ConsoleKind::Log, "(ERROR) a: bad but not gating"));
        c.record(&mk(
            ConsoleKind::Log,
            "(ERROR) screeps_ibex::panic: panicked at 'x', s.rs:1:1",
        ));
        c.record(&mk(
            ConsoleKind::Log,
            "(ERROR) screeps_ibex::game_loop: Failed deserialization: e",
        ));
        c.record(&mk(ConsoleKind::Result, "undefined"));
        c.record(&mk(ConsoleKind::Error, "Error: thrown"));
        assert_eq!(c.lines, 6);
        assert_eq!(c.log_lines, 4);
        assert_eq!(c.result_lines, 1);
        assert_eq!(c.error_events, 1);
        assert_eq!(c.error_log_lines, 3); // all (ERROR)-prefixed log lines
        assert_eq!(c.panic_lines, 1);
        assert_eq!(c.deser_failure_lines, 1);
    }

    // ---------------- metrics ----------------

    /// The seg-99 shape statssystem.rs:339-351 writes.
    #[test]
    fn cpu_extracts_from_seg99_stats() {
        let stats: Value = serde_json::from_str(
            r#"{"shard":{"shard0":{"time":7435,"gcl":{"progress":1,"progress_total":2,"level":1},
                "cpu":{"bucket":10000,"limit":100,"used":12.5},"room":{},"market":{"credits":0}}}}"#,
        )
        .unwrap();
        assert_eq!(
            cpu_from_stats(&stats),
            Some(CpuSample {
                bucket: 10000.0,
                limit: 100.0,
                used: 12.5
            })
        );
        assert_eq!(cpu_from_stats(&serde_json::json!({"shard": {}})), None);
        assert_eq!(cpu_from_stats(&serde_json::json!({})), None);
    }

    #[test]
    fn cpu_and_creep_summaries_aggregate() {
        let cpu = summarize_cpu(&[
            CpuSample {
                bucket: 9000.0,
                limit: 100.0,
                used: 5.0,
            },
            CpuSample {
                bucket: 9500.0,
                limit: 100.0,
                used: 15.0,
            },
        ])
        .unwrap();
        assert_eq!(cpu.used_avg, 10.0);
        assert_eq!(cpu.used_max, 15.0);
        assert_eq!(cpu.bucket_min, 9000.0);
        assert_eq!(cpu.bucket_last, 9500.0);
        assert_eq!(cpu.limit, 100.0);
        assert!(summarize_cpu(&[]).is_none());

        let creeps = summarize_creeps(&[2, 6, 4]).unwrap();
        assert_eq!((creeps.first, creeps.last, creeps.max), (2, 4, 6));
        assert!(summarize_creeps(&[]).is_none());
    }

    // ---------------- summary + gates ----------------

    fn summary_with(console: ConsoleCounters, ticks_observed: u64) -> Summary {
        Summary {
            scenario: "smoke".into(),
            git_sha: "abc1234".into(),
            started_at_ms: 0,
            wall_seconds: 1.0,
            tick_ms_configured: 100,
            tick_first: 100,
            tick_last: 100 + ticks_observed,
            ticks_observed,
            samples: 1,
            console,
            cpu: None,
            creeps: None,
        }
    }

    /// HARD ZEROS only (phase-0.md §5 criterion 6) — metrics never gate.
    #[test]
    fn gates_are_hard_zeros_only() {
        assert!(summary_with(ConsoleCounters::default(), 600)
            .gate_failures()
            .is_empty());

        // Error lines / error events do NOT gate (informational).
        let noisy = ConsoleCounters {
            lines: 10,
            log_lines: 8,
            error_log_lines: 5,
            error_events: 2,
            ..Default::default()
        };
        assert!(summary_with(noisy, 600).gate_failures().is_empty());

        assert_eq!(
            summary_with(ConsoleCounters::default(), 0)
                .gate_failures()
                .len(),
            1
        );
        let panicking = ConsoleCounters {
            panic_lines: 1,
            ..Default::default()
        };
        assert_eq!(summary_with(panicking, 600).gate_failures().len(), 1);
        let deser = ConsoleCounters {
            deser_failure_lines: 2,
            ..Default::default()
        };
        let failures = summary_with(deser, 0).gate_failures();
        assert_eq!(failures.len(), 2);
    }

    #[test]
    fn summary_json_has_the_headline_fields() {
        let s = summary_with(ConsoleCounters::default(), 600);
        let v: Value = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        for key in [
            "scenario",
            "git_sha",
            "wall_seconds",
            "tick_first",
            "tick_last",
            "ticks_observed",
            "samples",
            "console",
            "cpu",
            "creeps",
        ] {
            assert!(v.get(key).is_some(), "summary.json missing {key}");
        }
        assert_eq!(v["console"]["panic_lines"], 0);
    }

    // ---------------- misc ----------------

    #[test]
    fn scenario_names_are_sanitized() {
        assert_eq!(sanitize_scenario("baseline-0"), "baseline-0");
        assert_eq!(sanitize_scenario("smoke check #1"), "smoke-check--1");
        assert_eq!(sanitize_scenario("  "), "adhoc");
        assert_eq!(sanitize_scenario(""), "adhoc");
    }

    #[test]
    fn run_budget_scales_with_ticks() {
        assert_eq!(
            run_budget(600, 100),
            Duration::from_millis(600_000) + Duration::from_secs(120)
        );
        assert!(run_budget(2000, 100) > Duration::from_secs(2000));
    }
}
