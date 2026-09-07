//! `report` — the seg-57 series/summary extractor for one run directory
//! (ADR 0004 governor calibration evidence; WS-CLOSE lane b4).
//!
//! Reads `runs/<dir>/metrics.jsonl` (one capture sample per line, each
//! carrying the parsed seg-57 [`MetricsBlock`] under `"metrics"`) and
//! prints the tick series the pressure ladder is graded on — bucket,
//! trend, governor tier, pathing pools, move failures, creeps, fault
//! counters, `vm_starts` — plus a summary: tier transitions with the
//! bucket/trend they fired at, per-tier sample counts, bucket floor,
//! creeps-through-Critical (the "progress continues" check),
//! `vm_starts` delta (restart-loop check) and the stale-sample count
//! (a repeated block tick is the tick-kill signature).
//!
//! The scheduler's `scheduler: shed N system(s) under Critical` line is
//! `debug!` and the deployed logger is set up at `Info`
//! (`screeps-ibex/src/lib.rs` → `logging::setup_logging(logging::Info)`),
//! so the console carries NO shed line: the tier + pool series here IS
//! the Critical-shed evidence.
//!
//! Also home to the NOMINAL DRAIN MODEL the shipped
//! `scenarios/pressure-*.json` schedules were derived from (their
//! `description` fields carry the arithmetic in prose; the model + the
//! pin tests keep a JSON edit honest: never pinned at 0, Critical by
//! level reached, tail long enough to recover).

use anyhow::{Context, Result};
use screeps_ibex_metrics::MetricsBlock;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(clap::Args, Debug)]
pub struct ReportArgs {
    /// Run directory (runs/<scenario>-<sha>-<stamp>/) holding metrics.jsonl
    #[arg(long)]
    pub run: PathBuf,
    /// Emit the per-sample series as CSV instead of the aligned table
    #[arg(long)]
    pub csv: bool,
    /// Print the summary only (skip the per-sample series)
    #[arg(long)]
    pub summary_only: bool,
}

/// One seg-57 block, flattened to the fields the calibration reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub tick: u32,
    pub used: f64,
    pub bucket: i32,
    pub trend: f64,
    /// `governor.tier`, or "-" when the block predates the governor.
    pub tier: String,
    pub mission_ops_pool: u32,
    pub mission_ops_used: u32,
    pub ops_pool: u32,
    pub ops_used: u32,
    pub move_failures: u32,
    pub wasted_moves: u32,
    pub creeps: u32,
    pub serialize_skipped_shed: u32,
    pub serialize_skipped_aborted: u32,
    pub panics_caught: u32,
    pub vm_starts: u32,
    pub vm_fresh: bool,
}

impl Row {
    pub fn from_block(b: &MetricsBlock) -> Row {
        let pathing = b.pathing.clone().unwrap_or_default();
        Row {
            tick: b.tick,
            used: b.cpu.used,
            bucket: b.cpu.bucket,
            trend: b.cpu.bucket_trend,
            tier: b
                .governor
                .as_ref()
                .map(|g| g.tier.clone())
                .unwrap_or_else(|| "-".into()),
            mission_ops_pool: pathing.mission_ops_pool,
            mission_ops_used: pathing.mission_ops_used,
            ops_pool: pathing.ops_pool,
            ops_used: pathing.ops_used,
            move_failures: pathing.move_failures,
            wasted_moves: pathing.wasted_moves,
            creeps: b.creeps,
            serialize_skipped_shed: b.faults.serialize_skipped_shed,
            serialize_skipped_aborted: b.faults.serialize_skipped_aborted,
            panics_caught: b.faults.panics_caught,
            vm_starts: b.vm_starts,
            vm_fresh: b.vm_fresh,
        }
    }
}

/// Parsed series + the sample bookkeeping the summary reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// One row per DISTINCT block tick, in file order.
    pub rows: Vec<Row>,
    /// Capture samples in the file (lines that parsed as JSON).
    pub samples: usize,
    /// Samples carrying no parsable seg-57 block (segment not written yet).
    pub blockless: usize,
    /// Samples whose block tick repeated the previous sample's — the
    /// segment did not advance between samples: the tick-kill signature
    /// (a killed tick persists nothing), or a paused simulation.
    pub stale: usize,
}

/// Parse a `metrics.jsonl` text (pure). Unparsable lines are skipped
/// like the score loader does; a repeated block tick is counted, not
/// duplicated.
pub fn parse_metrics_jsonl(raw: &str) -> Series {
    let mut rows: Vec<Row> = Vec::new();
    let mut samples = 0;
    let mut blockless = 0;
    let mut stale = 0;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(sample) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        samples += 1;
        let Some(block) = sample
            .get("metrics")
            .and_then(crate::gates::parse_metrics_block)
        else {
            blockless += 1;
            continue;
        };
        let row = Row::from_block(&block);
        if rows.last().map(|r| r.tick) == Some(row.tick) {
            stale += 1;
            continue;
        }
        rows.push(row);
    }
    Series {
        rows,
        samples,
        blockless,
        stale,
    }
}

pub fn load_series(run_dir: &Path) -> Result<Series> {
    let path = run_dir.join("metrics.jsonl");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(parse_metrics_jsonl(&raw))
}

/// A tier change between consecutive samples, with the inputs the
/// kernel saw at the sample it first showed the new tier.
#[derive(Debug, Clone, PartialEq)]
pub struct TierTransition {
    pub tick: u32,
    pub from: String,
    pub to: String,
    pub bucket: i32,
    pub trend: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub samples: usize,
    pub blockless: usize,
    pub stale: usize,
    pub rows: usize,
    pub tick_first: Option<u32>,
    pub tick_last: Option<u32>,
    pub used_mean: f64,
    pub used_max: f64,
    pub bucket_first: Option<i32>,
    pub bucket_min: Option<(u32, i32)>,
    pub bucket_last: Option<i32>,
    /// Samples at bucket 0 — the artefact regime (tick capped at the
    /// per-tick limit; a non-sheddable burner kills every tick).
    pub bucket_zero_samples: usize,
    pub trend_min: Option<(u32, f64)>,
    pub trend_max: Option<(u32, f64)>,
    /// (tier, sample count), in first-seen order.
    pub tier_counts: Vec<(String, usize)>,
    pub transitions: Vec<TierTransition>,
    pub first_critical_tick: Option<u32>,
    pub last_critical_tick: Option<u32>,
    /// Distinct mission-pool sizes seen (tier-scaled: 20000/10000/5000).
    pub mission_pools: Vec<u32>,
    pub ops_pool_min: Option<u32>,
    pub ops_used_max: u32,
    pub mission_ops_used_max: u32,
    pub move_failures_max: u32,
    pub wasted_moves_max: u32,
    pub wasted_moves_sum: u64,
    pub creeps_first: Option<u32>,
    pub creeps_min: Option<u32>,
    pub creeps_max: Option<u32>,
    pub creeps_last: Option<u32>,
    /// Lowest creep count at or after the first Critical sample (the
    /// "progress continues under shed" check; None if never Critical).
    pub creeps_min_from_critical: Option<u32>,
    pub shed_last: u32,
    pub aborted_last: u32,
    pub panics_last: u32,
    pub vm_starts_first: Option<u32>,
    pub vm_starts_last: Option<u32>,
    pub vm_fresh_samples: usize,
}

impl Summary {
    pub fn vm_starts_delta(&self) -> Option<i64> {
        Some(self.vm_starts_last? as i64 - self.vm_starts_first? as i64)
    }
}

pub fn summarize(series: &Series) -> Summary {
    let rows = &series.rows;
    let n = rows.len();
    let used_mean = if n == 0 {
        0.0
    } else {
        rows.iter().map(|r| r.used).sum::<f64>() / n as f64
    };
    let used_max = rows.iter().map(|r| r.used).fold(0.0_f64, f64::max);

    let bucket_min = rows
        .iter()
        .map(|r| (r.tick, r.bucket))
        .min_by_key(|&(_, b)| b);
    let trend_min = rows
        .iter()
        .map(|r| (r.tick, r.trend))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let trend_max = rows
        .iter()
        .map(|r| (r.tick, r.trend))
        .max_by(|a, b| a.1.total_cmp(&b.1));

    let mut tier_counts: Vec<(String, usize)> = Vec::new();
    for r in rows {
        match tier_counts.iter_mut().find(|(t, _)| *t == r.tier) {
            Some((_, c)) => *c += 1,
            None => tier_counts.push((r.tier.clone(), 1)),
        }
    }
    let transitions: Vec<TierTransition> = rows
        .windows(2)
        .filter(|w| w[0].tier != w[1].tier)
        .map(|w| TierTransition {
            tick: w[1].tick,
            from: w[0].tier.clone(),
            to: w[1].tier.clone(),
            bucket: w[1].bucket,
            trend: w[1].trend,
        })
        .collect();
    let first_critical_tick = rows.iter().find(|r| r.tier == "critical").map(|r| r.tick);
    let last_critical_tick = rows.iter().rev().find(|r| r.tier == "critical").map(|r| r.tick);
    let creeps_min_from_critical = first_critical_tick.and_then(|t| {
        rows.iter()
            .filter(|r| r.tick >= t)
            .map(|r| r.creeps)
            .min()
    });

    let mut mission_pools: Vec<u32> = rows.iter().map(|r| r.mission_ops_pool).collect();
    mission_pools.sort_unstable();
    mission_pools.dedup();
    mission_pools.reverse();

    let last = rows.last();
    Summary {
        samples: series.samples,
        blockless: series.blockless,
        stale: series.stale,
        rows: n,
        tick_first: rows.first().map(|r| r.tick),
        tick_last: last.map(|r| r.tick),
        used_mean,
        used_max,
        bucket_first: rows.first().map(|r| r.bucket),
        bucket_min,
        bucket_last: last.map(|r| r.bucket),
        bucket_zero_samples: rows.iter().filter(|r| r.bucket <= 0).count(),
        trend_min,
        trend_max,
        tier_counts,
        transitions,
        first_critical_tick,
        last_critical_tick,
        mission_pools,
        ops_pool_min: rows.iter().map(|r| r.ops_pool).min(),
        ops_used_max: rows.iter().map(|r| r.ops_used).max().unwrap_or(0),
        mission_ops_used_max: rows.iter().map(|r| r.mission_ops_used).max().unwrap_or(0),
        move_failures_max: rows.iter().map(|r| r.move_failures).max().unwrap_or(0),
        wasted_moves_max: rows.iter().map(|r| r.wasted_moves).max().unwrap_or(0),
        wasted_moves_sum: rows.iter().map(|r| r.wasted_moves as u64).sum(),
        creeps_first: rows.first().map(|r| r.creeps),
        creeps_min: rows.iter().map(|r| r.creeps).min(),
        creeps_max: rows.iter().map(|r| r.creeps).max(),
        creeps_last: last.map(|r| r.creeps),
        creeps_min_from_critical,
        shed_last: last.map(|r| r.serialize_skipped_shed).unwrap_or(0),
        aborted_last: last.map(|r| r.serialize_skipped_aborted).unwrap_or(0),
        panics_last: last.map(|r| r.panics_caught).unwrap_or(0),
        vm_starts_first: rows.first().map(|r| r.vm_starts),
        vm_starts_last: last.map(|r| r.vm_starts),
        vm_fresh_samples: rows.iter().filter(|r| r.vm_fresh).count(),
    }
}

pub const SERIES_COLUMNS: [&str; 17] = [
    "tick",
    "used",
    "bucket",
    "trend",
    "tier",
    "mpool",
    "mused",
    "opool",
    "oused",
    "mvfail",
    "wasted",
    "creeps",
    "shed",
    "abort",
    "panic",
    "vm",
    "fresh",
];

fn row_cells(r: &Row) -> [String; 17] {
    [
        r.tick.to_string(),
        format!("{:.1}", r.used),
        r.bucket.to_string(),
        format!("{:.2}", r.trend),
        r.tier.clone(),
        r.mission_ops_pool.to_string(),
        r.mission_ops_used.to_string(),
        r.ops_pool.to_string(),
        r.ops_used.to_string(),
        r.move_failures.to_string(),
        r.wasted_moves.to_string(),
        r.creeps.to_string(),
        r.serialize_skipped_shed.to_string(),
        r.serialize_skipped_aborted.to_string(),
        r.panics_caught.to_string(),
        r.vm_starts.to_string(),
        if r.vm_fresh { "1".into() } else { "0".into() },
    ]
}

/// The per-sample series: CSV, or a right-aligned table.
pub fn render_series(rows: &[Row], csv: bool) -> String {
    let mut out = String::new();
    if csv {
        out.push_str(&SERIES_COLUMNS.join(","));
        out.push('\n');
        for r in rows {
            out.push_str(&row_cells(r).join(","));
            out.push('\n');
        }
        return out;
    }
    let cells: Vec<[String; 17]> = rows.iter().map(row_cells).collect();
    let widths: Vec<usize> = SERIES_COLUMNS
        .iter()
        .enumerate()
        .map(|(i, h)| {
            cells
                .iter()
                .map(|c| c[i].len())
                .max()
                .unwrap_or(0)
                .max(h.len())
        })
        .collect();
    let line = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{:>w$}", c, w = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
    };
    let header: Vec<String> = SERIES_COLUMNS.iter().map(|s| s.to_string()).collect();
    out.push_str(&line(&header));
    out.push('\n');
    for c in &cells {
        out.push_str(&line(c));
        out.push('\n');
    }
    out
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let opt = |v: Option<u32>| v.map(|x| x.to_string()).unwrap_or_else(|| "n/a".into());
        writeln!(
            f,
            "samples:   {} ({} without a block, {} stale/repeated tick) -> {} rows, ticks {}..{}",
            self.samples,
            self.blockless,
            self.stale,
            self.rows,
            opt(self.tick_first),
            opt(self.tick_last)
        )?;
        writeln!(f, "cpu.used:  mean {:.1}  max {:.1}", self.used_mean, self.used_max)?;
        match self.bucket_min {
            Some((tick, min)) => writeln!(
                f,
                "bucket:    first {}  min {} @tick {}  last {}  zero-samples {}",
                self.bucket_first.unwrap_or(0),
                min,
                tick,
                self.bucket_last.unwrap_or(0),
                self.bucket_zero_samples
            )?,
            None => writeln!(f, "bucket:    n/a")?,
        }
        if let (Some((tmin, vmin)), Some((tmax, vmax))) = (self.trend_min, self.trend_max) {
            writeln!(
                f,
                "trend:     min {:.2} @tick {}  max {:.2} @tick {}",
                vmin, tmin, vmax, tmax
            )?;
        }
        let tiers: Vec<String> = self
            .tier_counts
            .iter()
            .map(|(t, c)| format!("{t}={c}"))
            .collect();
        writeln!(f, "tier:      {}", tiers.join("  "))?;
        writeln!(
            f,
            "critical:  first {}  last {}  creeps-min-from-critical {}",
            opt(self.first_critical_tick),
            opt(self.last_critical_tick),
            opt(self.creeps_min_from_critical)
        )?;
        if self.transitions.is_empty() {
            writeln!(f, "transitions: none")?;
        } else {
            writeln!(f, "transitions:")?;
            for t in &self.transitions {
                writeln!(
                    f,
                    "  tick {}: {} -> {} (bucket {}, trend {:.2})",
                    t.tick, t.from, t.to, t.bucket, t.trend
                )?;
            }
        }
        let pools: Vec<String> = self.mission_pools.iter().map(|p| p.to_string()).collect();
        writeln!(
            f,
            "pathing:   mission pools {{{}}} mission-used max {}  ops pool min {}  ops used max {}",
            pools.join(","),
            self.mission_ops_used_max,
            opt(self.ops_pool_min),
            self.ops_used_max
        )?;
        writeln!(
            f,
            "moves:     move_failures max {}  wasted_moves max {} sum {}",
            self.move_failures_max, self.wasted_moves_max, self.wasted_moves_sum
        )?;
        writeln!(
            f,
            "creeps:    first {}  min {}  max {}  last {}",
            opt(self.creeps_first),
            opt(self.creeps_min),
            opt(self.creeps_max),
            opt(self.creeps_last)
        )?;
        writeln!(
            f,
            "faults:    serialize_skipped_shed {}  serialize_skipped_aborted {}  panics_caught {}",
            self.shed_last, self.aborted_last, self.panics_last
        )?;
        let delta = self
            .vm_starts_delta()
            .map(|d| format!("{d:+}"))
            .unwrap_or_else(|| "n/a".into());
        writeln!(
            f,
            "vm_starts: first {}  last {}  delta {}  vm_fresh samples {}",
            opt(self.vm_starts_first),
            opt(self.vm_starts_last),
            delta,
            self.vm_fresh_samples
        )
    }
}

/// Full report text for a run directory (series unless `summary_only`,
/// then the summary; `summary.json`'s bucket floor when present so the
/// seg-99 and seg-57 floors can be eyeballed together).
pub fn report(run_dir: &Path, csv: bool, summary_only: bool) -> Result<String> {
    let series = load_series(run_dir)?;
    let mut out = String::new();
    if !summary_only {
        out.push_str(&render_series(&series.rows, csv));
        out.push('\n');
    }
    write!(out, "{}", summarize(&series))?;
    if let Ok(raw) = std::fs::read_to_string(run_dir.join("summary.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let get = |p: &[&str]| {
                let mut cur = &v;
                for k in p {
                    cur = cur.get(k)?;
                }
                Some(cur.clone())
            };
            writeln!(
                out,
                "summary.json: ticks_observed {}  cpu.bucket_min {}  cpu.used_max {}  panic_lines {}  deser_failure_lines {}",
                get(&["ticks_observed"]).unwrap_or_default(),
                get(&["cpu", "bucket_min"]).unwrap_or_default(),
                get(&["cpu", "used_max"]).unwrap_or_default(),
                get(&["console", "panic_lines"]).unwrap_or_default(),
                get(&["console", "deser_failure_lines"]).unwrap_or_default(),
            )?;
        }
    }
    Ok(out)
}

pub fn run(args: &ReportArgs) -> Result<()> {
    print!("{}", report(&args.run, args.csv, args.summary_only)?);
    Ok(())
}

// ===================================================================
// nominal drain model — the arithmetic behind scenarios/pressure-*.json
// ===================================================================

/// Bucket dynamics of the private server as observed in
/// `runs/pressure-bbe86e0-20260611-060159` (see the scenario
/// descriptions for the derivation). Net drain while burning is
/// `ms - break_even_ms` per tick; refill while not burning is
/// `refill_per_tick`. Clamped to `[0, bucket_cap]`.
#[derive(Debug, Clone, Copy)]
pub struct DrainModel {
    /// Burn level at which the sustained net drain was ~0 (empirical, ms).
    pub break_even_ms: f64,
    /// Bucket regained per tick with the burner off (limit − own cost).
    pub refill_per_tick: f64,
    pub bucket_cap: f64,
    pub bucket_start: f64,
}

impl DrainModel {
    /// The fresh-colony private-server model the shipped scenarios use:
    /// limit 100, own cost ~18 ms, break-even burn ~90 ms.
    pub const PRIVATE_FRESH: DrainModel = DrainModel {
        break_even_ms: 90.0,
        refill_per_tick: 82.0,
        bucket_cap: 10_000.0,
        bucket_start: 10_000.0,
    };
}

/// Burn level scheduled for each observed tick `0..ticks` (last fault
/// in vector order wins on a tick, mirroring the injections' same-tick
/// firing order). Non-burn faults contribute nothing.
pub fn burn_schedule(faults: &[crate::scenario::Fault], ticks: u64) -> Vec<u32> {
    let mut schedule = vec![0u32; ticks as usize];
    for fault in faults {
        if let crate::scenario::Fault::CpuBurn {
            at_observed_tick,
            ms,
            duration_ticks,
        } = fault
        {
            let end = (at_observed_tick + duration_ticks).min(ticks);
            for slot in schedule
                .iter_mut()
                .take(end as usize)
                .skip(*at_observed_tick as usize)
            {
                *slot = *ms;
            }
        }
    }
    schedule
}

/// Nominal bucket AFTER each observed tick under `model` (no sampler
/// lag, no break-even error — the design-intent trajectory).
pub fn nominal_trajectory(schedule: &[u32], model: DrainModel) -> Vec<i32> {
    let mut bucket = model.bucket_start;
    schedule
        .iter()
        .map(|&ms| {
            let delta = if ms == 0 {
                model.refill_per_tick
            } else {
                -(ms as f64 - model.break_even_ms)
            };
            bucket = (bucket + delta).clamp(0.0, model.bucket_cap);
            bucket.round() as i32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{Fault, Scenario};

    /// Mirrors `screeps-ibex/src/cpugovernor.rs` (the eval crate cannot
    /// depend on the bot crate); the calibration bed is designed against
    /// these level rules, so a constants change here is a design change.
    const CRITICAL_BUCKET: i32 = 1_500;
    /// The band the hover must stay in: above the tick-kill artefact
    /// regime, below CRITICAL_BUCKET.
    const HOVER_FLOOR: i32 = 300;

    fn sample(
        tick: u32,
        bucket: i32,
        trend: f64,
        tier: &str,
        mpool: u32,
        creeps: u32,
        vm_starts: u32,
    ) -> String {
        format!(
            r#"{{"ts_ms":1,"tick":{tick},"cpu":null,"creeps":{creeps},"stats":null,"metrics":{{"v":1,"tick":{tick},"vm_fresh":{fresh},"vm_starts":{vm_starts},"cpu":{{"used":{used},"limit":100.0,"tick_limit":500.0,"bucket":{bucket},"bucket_trend":{trend}}},"gcl":{{"level":1,"progress":0.0,"progress_total":1.0}},"gpl":{{"level":0,"progress":0.0,"progress_total":1.0}},"creeps":{creeps},"faults":{{"serialize_skipped_shed":0,"serialize_skipped_aborted":0,"panics_caught":0}},"governor":{{"tier":"{tier}"}},"pathing":{{"ops_pool":{opool},"ops_used":7,"mission_ops_pool":{mpool},"mission_ops_used":3,"move_failures":{mvf},"wasted_moves":2}}}}}}"#,
            fresh = vm_starts > 1 && tick == 460,
            used = if tier == "normal" { 20.0 } else { 108.5 },
            opool = mpool,
            mvf = if tier == "critical" { 4 } else { 0 },
        )
    }

    /// Five samples + one blockless + one stale repeat: a fresh start,
    /// a Conserve-on-trend sample, two Critical samples spanning a
    /// reset, and the recovery.
    fn fixture() -> String {
        [
            r#"{"ts_ms":0,"tick":100,"cpu":null,"creeps":null,"stats":null}"#.to_string(),
            sample(112, 10000, 0.0, "normal", 20000, 2, 1),
            sample(400, 9400, -6.5, "conserve", 10000, 3, 1),
            sample(430, 1300, -12.0, "critical", 5000, 3, 1),
            sample(430, 1300, -12.0, "critical", 5000, 3, 1),
            sample(460, 900, 0.0, "critical", 5000, 4, 2),
            sample(520, 4200, 40.0, "normal", 20000, 5, 2),
        ]
        .join("\n")
    }

    #[test]
    fn parses_rows_and_counts_blockless_and_stale_samples() {
        let s = parse_metrics_jsonl(&fixture());
        assert_eq!(s.samples, 7);
        assert_eq!(s.blockless, 1);
        assert_eq!(s.stale, 1);
        let ticks: Vec<u32> = s.rows.iter().map(|r| r.tick).collect();
        assert_eq!(ticks, vec![112, 400, 430, 460, 520]);
        let r = &s.rows[2];
        assert_eq!(r.tier, "critical");
        assert_eq!(r.mission_ops_pool, 5000);
        assert_eq!(r.move_failures, 4);
        assert_eq!(r.wasted_moves, 2);
        assert!(!r.vm_fresh);
        assert!(s.rows[3].vm_fresh);
    }

    #[test]
    fn summary_extracts_transitions_floor_and_restart_delta() {
        let s = parse_metrics_jsonl(&fixture());
        let sum = summarize(&s);
        assert_eq!(sum.bucket_min, Some((460, 900)));
        assert_eq!(sum.bucket_zero_samples, 0);
        assert_eq!(sum.trend_min, Some((430, -12.0)));
        assert_eq!(
            sum.tier_counts,
            vec![
                ("normal".to_string(), 2),
                ("conserve".to_string(), 1),
                ("critical".to_string(), 2)
            ]
        );
        let path: Vec<(u32, &str, &str, i32)> = sum
            .transitions
            .iter()
            .map(|t| (t.tick, t.from.as_str(), t.to.as_str(), t.bucket))
            .collect();
        assert_eq!(
            path,
            vec![
                (400, "normal", "conserve", 9400),
                (430, "conserve", "critical", 1300),
                (520, "critical", "normal", 4200)
            ]
        );
        assert_eq!(sum.first_critical_tick, Some(430));
        assert_eq!(sum.last_critical_tick, Some(460));
        // Progress continued through the Critical hold (3 -> 4 -> 5).
        assert_eq!(sum.creeps_min_from_critical, Some(3));
        assert_eq!(sum.mission_pools, vec![20000, 10000, 5000]);
        assert_eq!(sum.ops_pool_min, Some(5000));
        assert_eq!(sum.move_failures_max, 4);
        assert_eq!(sum.wasted_moves_sum, 10);
        // Exactly one restart across the fixture.
        assert_eq!(sum.vm_starts_delta(), Some(1));
        assert_eq!(sum.vm_fresh_samples, 1);
        let text = sum.to_string();
        assert!(text.contains("tick 430: conserve -> critical (bucket 1300, trend -12.00)"));
        assert!(text.contains("vm_starts: first 1  last 2  delta +1"));
        assert!(text.contains("mission pools {20000,10000,5000}"));
    }

    #[test]
    fn empty_series_summarizes_without_panicking() {
        let s = parse_metrics_jsonl("not json\n\n");
        let sum = summarize(&s);
        assert_eq!(sum.rows, 0);
        assert_eq!(sum.bucket_min, None);
        assert_eq!(sum.vm_starts_delta(), None);
        assert!(sum.to_string().contains("transitions: none"));
    }

    #[test]
    fn series_renders_csv_and_aligned_table() {
        let s = parse_metrics_jsonl(&fixture());
        let csv = render_series(&s.rows, true);
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), SERIES_COLUMNS.join(","));
        assert_eq!(
            lines.next().unwrap(),
            "112,20.0,10000,0.00,normal,20000,3,20000,7,0,2,2,0,0,0,1,0"
        );
        assert_eq!(csv.lines().count(), 6);
        let table = render_series(&s.rows, false);
        let header = table.lines().next().unwrap();
        assert!(header.trim_start().starts_with("tick"));
        // Every row has the same width as the header (aligned columns).
        assert!(table.lines().all(|l| l.len() == header.len()));
    }

    /// The real 2026-06-11 pressure run parses end to end and yields the
    /// Conserve-on-trend transition the phase-1 ledger recorded.
    #[test]
    fn prior_pressure_run_reproduces_the_ledger_transitions() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../runs/pressure-bbe86e0-20260611-060159");
        if !dir.join("metrics.jsonl").exists() {
            eprintln!("skipping: {} not present (runs/ is gitignored)", dir.display());
            return;
        }
        let text = report(&dir, false, true).unwrap();
        assert!(text.contains("tick 460: normal -> conserve (bucket 9385, trend -6.63)"), "{text}");
        assert!(text.contains("tick 549: conserve -> normal"), "{text}");
        assert!(text.contains("bucket:    first 10000  min 8988 @tick 672"), "{text}");
        assert!(text.contains("critical:  first n/a"), "{text}");
    }

    // ---------------------------------------------------------------
    // the shipped calibration schedules (scenarios/pressure-*.json)
    // ---------------------------------------------------------------

    fn shipped(name: &str) -> (Scenario, serde_json::Value) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scenarios")
            .join(format!("{name}.json"));
        let scenario = Scenario::load(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        (scenario, raw)
    }

    fn burn_stages(s: &Scenario) -> Vec<(u64, u32, u64)> {
        s.faults
            .iter()
            .filter_map(|f| match f {
                Fault::CpuBurn {
                    at_observed_tick,
                    ms,
                    duration_ticks,
                } => Some((*at_observed_tick, *ms, *duration_ticks)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn burn_schedule_applies_faults_in_vector_order() {
        let faults = vec![
            Fault::CpuBurn {
                at_observed_tick: 2,
                ms: 220,
                duration_ticks: 3,
            },
            Fault::GlobalReset { at_observed_tick: 3 },
            Fault::CpuBurn {
                at_observed_tick: 5,
                ms: 96,
                duration_ticks: 2,
            },
        ];
        assert_eq!(burn_schedule(&faults, 8), vec![0, 0, 220, 220, 220, 96, 96, 0]);
        let traj = nominal_trajectory(&[0, 220, 220, 0], DrainModel::PRIVATE_FRESH);
        assert_eq!(traj, vec![10000, 9870, 9740, 9822]);
        // Clamped at the floor: the artefact regime is representable.
        let pinned = nominal_trajectory(&[500; 30], DrainModel::PRIVATE_FRESH);
        assert_eq!(*pinned.last().unwrap(), 0);
    }

    /// Every shipped schedule: loads under the v1 gate, carries its
    /// arithmetic in `description`, stages fire in vector order
    /// (stage-2 ON strictly after stage-1 OFF), and under the nominal
    /// model reaches Critical by level, never pins at 0, and recovers to
    /// a full bucket before the run ends.
    #[test]
    fn shipped_pressure_schedules_hover_in_the_critical_band_and_recover() {
        for name in [
            "pressure-critical-hover",
            "pressure-release",
            "pressure-reset-under-critical",
        ] {
            let (s, raw) = shipped(name);
            assert_eq!(s.name, name);
            let description = raw["description"].as_str().unwrap_or("");
            assert!(
                description.contains("BE~90") && description.contains("L=100"),
                "{name}: description must carry the burn arithmetic"
            );
            let stages = burn_stages(&s);
            assert_eq!(stages.len(), 2, "{name}: ramp + hover");
            let (ramp_at, ramp_ms, ramp_len) = stages[0];
            let (hover_at, hover_ms, hover_len) = stages[1];
            assert_eq!(
                hover_at,
                ramp_at + ramp_len + 1,
                "{name}: hover ON must be scheduled one tick after ramp OFF"
            );
            assert!(ramp_ms > hover_ms && hover_ms > 90, "{name}: {ramp_ms} > {hover_ms} > BE");

            let schedule = burn_schedule(&s.faults, s.ticks);
            let traj = nominal_trajectory(&schedule, DrainModel::PRIVATE_FRESH);
            let hover_end = (hover_at + hover_len) as usize;
            let ramp_end = traj[(ramp_at + ramp_len) as usize - 1];
            assert!(
                (1_500..=3_500).contains(&ramp_end),
                "{name}: ramp should end just above CRITICAL_BUCKET, got {ramp_end}"
            );
            let hover: &[i32] = &traj[hover_at as usize..hover_end];
            let hover_min = *hover.iter().min().unwrap();
            assert!(hover_min >= HOVER_FLOOR, "{name}: hover floor {hover_min} < {HOVER_FLOOR}");
            let critical_ticks = hover.iter().filter(|&&b| b < CRITICAL_BUCKET).count();
            assert!(
                critical_ticks >= 40,
                "{name}: only {critical_ticks} hover ticks under CRITICAL_BUCKET"
            );
            assert_eq!(traj.iter().filter(|&&b| b == 0).count(), 0, "{name}: pinned at 0");
            let tail = s.ticks as usize - hover_end;
            assert!(tail >= 300, "{name}: recovery tail {tail} < 3 windows");
            assert_eq!(*traj.last().unwrap(), 10_000, "{name}: bucket must be full again");
        }
    }

    /// The reset rung fires its global reset while the nominal bucket is
    /// under CRITICAL_BUCKET (tier Critical by level on both sides).
    #[test]
    fn reset_rung_resets_inside_the_critical_hold() {
        let (s, _) = shipped("pressure-reset-under-critical");
        let reset_at = s
            .faults
            .iter()
            .find_map(|f| match f {
                Fault::GlobalReset { at_observed_tick } => Some(*at_observed_tick),
                _ => None,
            })
            .expect("a global_reset fault");
        let traj = nominal_trajectory(&burn_schedule(&s.faults, s.ticks), DrainModel::PRIVATE_FRESH);
        let at_reset = traj[reset_at as usize];
        assert!(
            (HOVER_FLOOR..CRITICAL_BUCKET).contains(&at_reset),
            "reset at bucket {at_reset}, outside the Critical band"
        );
        // Exactly one reset: the vm_starts +1 expectation.
        let resets = s
            .faults
            .iter()
            .filter(|f| matches!(f, Fault::GlobalReset { .. }))
            .count();
        assert_eq!(resets, 1);
        // Compiles to on/off/on/off + the reset flag, in vector order.
        let inj = s.injections();
        assert_eq!(inj.len(), 5);
        assert!(inj[4].expression.contains("reset.environment=true"));
    }
}
