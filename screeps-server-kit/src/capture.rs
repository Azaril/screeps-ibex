//! Console + metrics capture into `runs/` artifacts, and the
//! `run --ticks N` loop.
//!
//! **Mechanism only** (P0.A14): this module owns the run loop, the JSONL
//! artifact records, the counters, and the summary — but it knows
//! nothing about any particular bot. What counts as a panic line, a
//! deserialization failure, or an error-level log line is supplied by
//! the caller as a [`MarkerSpec`] (policy; this repo's lives in
//! `screeps-ibex-eval`'s `gates` module), and which memory segment carries the
//! bot's live stats is supplied via [`CaptureSpec`].
//!
//! The console-websocket PROTOCOL (frames, auth/subscribe handshake,
//! payload flattening) lives in the shared `screeps-rest-api` crate
//! (P0.A12) — `screeps_rest_api::socket` pins it with citations.
//!
//! ## Metrics sources
//!
//! - `GET /api/game/time` per sample (tick progress).
//! - `GET /api/user/memory-segment?segment=N` (`CaptureSpec::stats_segment`)
//!   — the bot's live stats JSON. CPU extraction ([`cpu_from_stats`])
//!   understands the common screeps-stats shard shape
//!   `{"shard":{"<name>":{"cpu":{bucket,limit,used},...}}}`.
//! - Creep count (best-effort) via the server CLI:
//!   `storage.db['rooms.objects'].count({type:'creep',user:<id>})`.

use crate::config::KitConfig;
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

/// Metrics sample cadence. At a 100 ms tick rate this is one sample
/// every ~20 ticks; console capture is continuous regardless.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

// ===================================================================
// the caller-supplied capture policy
// ===================================================================

/// The bot-specific log markers the counters classify against. The kit
/// COUNTS; the consumer crate decides what the markers ARE (and pins
/// them against its bot's sources) — e.g. `screeps-ibex-eval`'s gates module.
#[derive(Debug, Clone, Default)]
pub struct MarkerSpec {
    /// Substrings identifying a panic in the bot's console output
    /// (counted into [`ConsoleCounters::panic_lines`], any line kind).
    pub panic_markers: Vec<String>,
    /// Substrings identifying a state-deserialization failure
    /// (counted into [`ConsoleCounters::deser_failure_lines`]).
    pub deser_markers: Vec<String>,
    /// Prefix of an error-level line in the bot's log format (counted
    /// into [`ConsoleCounters::error_log_lines`], `log`-kind only).
    pub error_log_prefix: Option<String>,
}

impl MarkerSpec {
    pub fn is_panic_line(&self, line: &str) -> bool {
        self.panic_markers.iter().any(|m| line.contains(m.as_str()))
    }

    pub fn is_deser_failure_line(&self, line: &str) -> bool {
        self.deser_markers.iter().any(|m| line.contains(m.as_str()))
    }

    fn is_error_log_line(&self, line: &str) -> bool {
        self.error_log_prefix
            .as_deref()
            .is_some_and(|p| line.starts_with(p))
    }
}

/// Everything a [`run`] needs to know about the bot under capture.
#[derive(Debug, Clone, Default)]
pub struct CaptureSpec {
    pub markers: MarkerSpec,
    /// Memory segment polled for the bot's live-stats JSON each sample
    /// (`None` disables stats/CPU sampling).
    pub stats_segment: Option<u8>,
    /// Memory segment polled for the bot's versioned metrics block each
    /// sample (`None` disables; for ibex this is seg 57 — the ADR 0006
    /// block whose schema lives in `screeps-ibex-metrics`). Captured
    /// raw-parsed: the kit stays bot-agnostic, callers interpret.
    pub metrics_segment: Option<u8>,
    /// Fault-injection schedule (P1.A5): console expressions fired once
    /// when the OBSERVED tick count crosses `at_observed_tick` (checked
    /// at the sampling cadence, so firing lags by up to one sample
    /// interval). The expression runs in the user's runtime next tick —
    /// callers use it to set `Memory` flags their bot reads (cpu
    /// burner, one-shot reset triggers). The kit stays bot-agnostic.
    pub console_injections: Vec<ConsoleInjection>,
}

/// One scheduled console-expression injection (see
/// [`CaptureSpec::console_injections`]).
#[derive(Debug, Clone)]
pub struct ConsoleInjection {
    /// Fires once the run has OBSERVED this many ticks.
    pub at_observed_tick: u64,
    /// JS evaluated in the user's runtime at the next tick.
    pub expression: String,
    /// Human-readable label for logs.
    pub label: String,
}

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

/// Counters aggregated over a run's console stream, classified against
/// the caller's [`MarkerSpec`].
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ConsoleCounters {
    /// Total JSONL records written (non-empty events only).
    pub lines: u64,
    pub log_lines: u64,
    pub result_lines: u64,
    /// `error`-kind events from the engine (thrown errors/aborts).
    pub error_events: u64,
    /// Error-level bot log lines per `error_log_prefix` (informational
    /// superset).
    pub error_log_lines: u64,
    /// HARD-ZERO gate material: lines matching `panic_markers` (any kind).
    pub panic_lines: u64,
    /// HARD-ZERO gate material: lines matching `deser_markers`.
    pub deser_failure_lines: u64,
}

impl ConsoleCounters {
    pub fn record(&mut self, line: &ConsoleLine, markers: &MarkerSpec) {
        self.lines += 1;
        match line.kind {
            ConsoleKind::Log => {
                self.log_lines += 1;
                if markers.is_error_log_line(&line.line) {
                    self.error_log_lines += 1;
                }
            }
            ConsoleKind::Result => self.result_lines += 1,
            ConsoleKind::Error => self.error_events += 1,
        }
        if markers.is_panic_line(&line.line) {
            self.panic_lines += 1;
        }
        if markers.is_deser_failure_line(&line.line) {
            self.deser_failure_lines += 1;
        }
    }
}

// ===================================================================
// metrics shaping (pure)
// ===================================================================

/// CPU stats as published in the common screeps-stats shard shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CpuSample {
    pub bucket: f64,
    pub limit: f64,
    pub used: f64,
}

/// Extract the CPU block from a stats JSON in the common shard shape
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
    /// Full parsed stats-segment JSON (informational; None until the
    /// bot writes the segment, or when no segment is configured).
    pub stats: Option<Value>,
    /// Full parsed metrics-segment JSON (`CaptureSpec::metrics_segment`;
    /// None until the bot writes it, or when unconfigured).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Value>,
}

// ===================================================================
// summary (pure aggregation + the hard-zero gates)
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
    /// The smoke gates — **HARD ZEROS only**: zero ticks observed, any
    /// panic-marker line, any deser-marker line. (Deploy failure gates
    /// earlier, in the deploy step.) Every metric (CPU, creeps, error
    /// counts) is informational — single-run metric gates are flake
    /// generators. `markers` is the same spec the counters were
    /// classified against (it only shapes the failure messages).
    pub fn gate_failures(&self, markers: &MarkerSpec) -> Vec<String> {
        let mut failures = Vec::new();
        if self.ticks_observed == 0 {
            failures.push("zero ticks observed (simulation not advancing)".to_string());
        }
        if self.console.panic_lines > 0 {
            failures.push(format!(
                "{} console line(s) matching the panic marker(s) {:?}",
                self.console.panic_lines, markers.panic_markers
            ));
        }
        if self.console.deser_failure_lines > 0 {
            failures.push(format!(
                "{} console line(s) matching deserialization-failure markers {:?}",
                self.console.deser_failure_lines, markers.deser_markers
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
            "console:  {} lines ({} log, {} results), {} error events, {} error-level lines, {} panics, {} deser failures",
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
            None => writeln!(f, "cpu:      (no stats-segment data seen)")?,
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
/// exist).
fn run_budget(ticks: u64, tick_ms: u64) -> Duration {
    Duration::from_millis(ticks.saturating_mul(tick_ms).saturating_mul(10))
        + Duration::from_secs(120)
}

// ===================================================================
// the run loop (`run --ticks N [--scenario NAME]`)
// ===================================================================

pub struct RunArtifacts {
    pub dir: PathBuf,
    pub summary: Summary,
}

/// Repo root = the directory `.screeps.yaml` was loaded from (where the
/// gitignored `runs/` tree lives).
fn repo_root(cfg: &KitConfig) -> Result<PathBuf> {
    cfg.source_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("config was not loaded from a file — cannot locate runs/")
}

/// Read a memory segment and parse it as JSON; `None` for empty,
/// unwritten, unparseable, or failed reads (failures warn — segment
/// sampling is best-effort and must not abort a run).
async fn read_segment_json(api: &screeps_rest_api::Client, segment: u8) -> Option<Value> {
    match api.memory_segment(segment).await {
        Ok(resp) => match resp.data {
            Some(raw) if !raw.is_empty() => serde_json::from_str::<Value>(&raw).ok(),
            _ => None,
        },
        Err(e) => {
            tracing::warn!("seg-{segment} read failed: {e:#}");
            None
        }
    }
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
/// `runs/<scenario>-<git-sha>-<stamp>/{console.jsonl,metrics.jsonl,summary.json}`
/// under the repo root (the directory `.screeps.yaml` was loaded from).
/// `spec` supplies the bot-specific markers and stats segment.
pub async fn run(
    cfg: &KitConfig,
    ticks: u64,
    scenario: &str,
    spec: &CaptureSpec,
) -> Result<RunArtifacts> {
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
    let budget = run_budget(ticks, cfg.stack.tick_ms);

    // Shared state between the console task and the sampler.
    let last_tick = Arc::new(AtomicU64::new(tick_first));
    let counters: Arc<Mutex<ConsoleCounters>> = Arc::default();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut console_task = tokio::spawn(console_capture_task(
        api.ws_url(),
        ws_token,
        user.id.clone(),
        dir.join("console.jsonl"),
        spec.markers.clone(),
        last_tick.clone(),
        counters.clone(),
        stop_rx,
    ));

    // Metrics sampler (this task).
    let mut metrics_file = std::io::BufWriter::new(
        std::fs::File::create(dir.join("metrics.jsonl")).context("creating metrics.jsonl")?,
    );
    let cli = CliClient::new(cfg.stack.cli_port).ok(); // creep counts: best-effort
    let mut cpu_samples: Vec<CpuSample> = Vec::new();
    let mut creep_counts: Vec<u64> = Vec::new();
    let mut samples = 0u64;
    let mut tick_last = tick_first;
    let mut consecutive_failures = 0u32;
    let mut last_progress = Instant::now();
    let mut injections_fired = vec![false; spec.console_injections.len()];

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

        // Fault injections: fire each once when its observed-tick
        // threshold is crossed (lag bounded by the sample interval).
        let observed_now = tick_last.saturating_sub(tick_first);
        for (i, injection) in spec.console_injections.iter().enumerate() {
            if !injections_fired[i] && observed_now >= injection.at_observed_tick {
                injections_fired[i] = true;
                match api.console(&injection.expression).await {
                    Ok(_) => tracing::info!(
                        "fault injection '{}' fired at observed tick {observed_now} (scheduled {})",
                        injection.label,
                        injection.at_observed_tick
                    ),
                    Err(e) => tracing::warn!(
                        "fault injection '{}' FAILED to send: {e:#}",
                        injection.label
                    ),
                }
            }
        }

        let stats: Option<Value> = match spec.stats_segment {
            Some(segment) => read_segment_json(&api, segment).await,
            None => None,
        };
        let metrics: Option<Value> = match spec.metrics_segment {
            Some(segment) => read_segment_json(&api, segment).await,
            None => None,
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
            metrics,
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
        tick_ms_configured: cfg.stack.tick_ms,
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

#[allow(clippy::too_many_arguments)]
async fn console_capture_task(
    ws_url: String,
    token: SecretString,
    user_id: String,
    path: PathBuf,
    markers: MarkerSpec,
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
                        c.record(&line, &markers);
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
// tests — the mechanism, against SYNTHETIC markers (the real bot
// markers are policy and are pinned in the consumer crate, e.g.
// screeps-ibex-eval's gates module)
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A deliberately bot-agnostic spec: proves classification is
    /// driven by the caller's spec, not by built-in strings.
    fn synthetic_markers() -> MarkerSpec {
        MarkerSpec {
            panic_markers: vec!["KABOOM:".to_string()],
            deser_markers: vec!["LOAD-FAIL-A:".to_string(), "LOAD-FAIL-B:".to_string()],
            error_log_prefix: Some("[err]".to_string()),
        }
    }

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
            line: "(INFO) some_bot: hello".into(),
        };
        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"ts_ms":1781061591846,"tick":7435,"kind":"log","line":"(INFO) some_bot: hello"}"#
        );
    }

    // ---------------- markers + counters ----------------

    /// Marker matching is substring-based, line-kind-independent for
    /// panic/deser, and prefix-based for error-level lines.
    #[test]
    fn marker_spec_classifies_by_supplied_strings_only() {
        let spec = synthetic_markers();
        assert!(spec.is_panic_line("xx KABOOM: it broke"));
        assert!(!spec.is_panic_line("panicked at 'x'")); // a FOREIGN bot's marker — not ours
        assert!(spec.is_deser_failure_line("LOAD-FAIL-B: bad bytes"));
        assert!(!spec.is_deser_failure_line("Failed deserialization: e"));
        assert!(spec.is_error_log_line("[err] something"));
        assert!(!spec.is_error_log_line("info [err]"));

        // An empty spec counts nothing as a marker.
        let empty = MarkerSpec::default();
        assert!(!empty.is_panic_line("KABOOM: x"));
        assert!(!empty.is_deser_failure_line("LOAD-FAIL-A: x"));
        assert!(!empty.is_error_log_line("[err] x"));
    }

    #[test]
    fn counters_classify_lines_against_the_spec() {
        let spec = synthetic_markers();
        let mut c = ConsoleCounters::default();
        let mk = |kind, line: &str| ConsoleLine {
            ts_ms: 0,
            tick: None,
            kind,
            line: line.into(),
        };
        c.record(&mk(ConsoleKind::Log, "(INFO) a: fine"), &spec);
        c.record(&mk(ConsoleKind::Log, "[err] bad but not gating"), &spec);
        c.record(
            &mk(ConsoleKind::Log, "[err] KABOOM: panic-equivalent"),
            &spec,
        );
        c.record(
            &mk(ConsoleKind::Log, "[err] LOAD-FAIL-A: state gone"),
            &spec,
        );
        c.record(&mk(ConsoleKind::Result, "undefined"), &spec);
        c.record(&mk(ConsoleKind::Error, "Error: thrown"), &spec);
        assert_eq!(c.lines, 6);
        assert_eq!(c.log_lines, 4);
        assert_eq!(c.result_lines, 1);
        assert_eq!(c.error_events, 1);
        assert_eq!(c.error_log_lines, 3); // all [err]-prefixed log lines
        assert_eq!(c.panic_lines, 1);
        assert_eq!(c.deser_failure_lines, 1);
    }

    // ---------------- metrics ----------------

    /// The common screeps-stats shard shape.
    #[test]
    fn cpu_extracts_from_shard_stats_shape() {
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

    /// HARD ZEROS only — metrics never gate; the failure messages name
    /// the caller's markers.
    #[test]
    fn gates_are_hard_zeros_only() {
        let spec = synthetic_markers();
        assert!(summary_with(ConsoleCounters::default(), 600)
            .gate_failures(&spec)
            .is_empty());

        // Error lines / error events do NOT gate (informational).
        let noisy = ConsoleCounters {
            lines: 10,
            log_lines: 8,
            error_log_lines: 5,
            error_events: 2,
            ..Default::default()
        };
        assert!(summary_with(noisy, 600).gate_failures(&spec).is_empty());

        assert_eq!(
            summary_with(ConsoleCounters::default(), 0)
                .gate_failures(&spec)
                .len(),
            1
        );
        let panicking = ConsoleCounters {
            panic_lines: 1,
            ..Default::default()
        };
        let failures = summary_with(panicking, 600).gate_failures(&spec);
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0].contains("KABOOM:"),
            "gate message must name the caller's marker: {failures:?}"
        );
        let deser = ConsoleCounters {
            deser_failure_lines: 2,
            ..Default::default()
        };
        let failures = summary_with(deser, 0).gate_failures(&spec);
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
