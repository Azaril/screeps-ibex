//! The seg-57 metrics block — ADR 0006's always-on, versioned telemetry
//! segment (P1.A1). This crate is the SCHEMA CONTRACT shared by the two
//! sides of the seam:
//!
//! - **writer**: the bot (`screeps-ibex/src/metrics.rs`) serializes one
//!   [`MetricsBlock`] to RawMemory segment 57 every tick;
//! - **reader**: the eval harness (`screeps-ibex-eval`, workspace-excluded
//!   — hence this dedicated crate rather than a bot module) parses the
//!   same type out of captured segments.
//!
//! No game-API dependencies: everything here compiles on any host target
//! and is kernel-testable (ADR 0015 L0).
//!
//! ## Versioning
//!
//! `MetricsBlock::v` carries [`METRICS_SCHEMA_VERSION`]. Readers accept
//! any version and match on it; fields are additive within a version
//! bump (`#[serde(default)]` everywhere on the read path), so an old
//! reader sees a new block as "newer version, parse what I know".
//! Removing or re-typing a field REQUIRES a version bump and a reader
//! update — the round-trip pin tests below are the tripwire.

use serde::{Deserialize, Serialize};

/// Bump on any non-additive schema change; readers match on it.
pub const METRICS_SCHEMA_VERSION: u32 = 1;

/// One tick's metrics block — the entire seg-57 payload, JSON-encoded
/// (human-debuggable; a block is ~1–2 KB against the 100 KB segment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsBlock {
    /// Schema version ([`METRICS_SCHEMA_VERSION`]).
    pub v: u32,
    /// Game tick this block describes.
    pub tick: u32,
    /// True on the FIRST block emitted after a global (VM/heap) reset.
    /// One-tick flag — samplers can miss it; prefer [`Self::vm_starts`].
    #[serde(default)]
    pub vm_fresh: bool,
    /// Cumulative VM starts, persisted in `Memory._metrics.vm_starts`
    /// (Memory survives VM resets; the heap doesn't) — the robust
    /// restart counter samplers can't miss. 0 = writer predates the
    /// field.
    #[serde(default)]
    pub vm_starts: u32,
    pub cpu: CpuMetrics,
    pub gcl: LevelProgress,
    pub gpl: LevelProgress,
    #[serde(default)]
    pub credits: f64,
    #[serde(default)]
    pub creeps: u32,
    #[serde(default)]
    pub missions: u32,
    #[serde(default)]
    pub operations: u32,
    #[serde(default)]
    pub rooms: Vec<RoomMetrics>,
    #[serde(default)]
    pub faults: FaultCounters,
    /// Governor view (P1.B3) — absent until the governor lands/emits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governor: Option<GovernorMetrics>,
    /// Pathfinding budget view (P1.B2/B4) — absent until the facade lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pathing: Option<PathingMetrics>,
    /// Guarded-intent recorder view (P1.C3) — combat categories first;
    /// grows as more pipelines route through the sink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intents: Option<IntentMetrics>,
    /// Self-tuning CPU cost model (expansion capacity input). Persisted so
    /// the per-room estimate survives VM resets — the writer seeds its heap
    /// EMA from this block on load. Absent until the model has samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<CpuModelMetrics>,
    /// Squad-cohesion view (P2.H3) — the combat-effectiveness signal: how tightly squads hold
    /// formation. Absent until a squad exists. Feeds the colony-health military term + the
    /// movement-overhaul soak (cohesion should hold ~1.0 forming/idle and recover after engagements).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cohesion: Option<CohesionMetrics>,
}

/// Persisted CPU cost model: an exponential moving average of per-tick CPU
/// used, with the sample count so a reader can tell a warm model from a cold
/// one. The claim pipeline divides `ema_used` by the owned-room count to
/// estimate marginal per-room CPU and size the dynamic room cap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct CpuModelMetrics {
    /// EMA of `game::cpu::get_used()` per tick.
    #[serde(default)]
    pub ema_used: f64,
    /// Samples folded into the EMA since it was first seeded.
    #[serde(default)]
    pub samples: u32,
}

/// Per-tick guarded-intent counts + the order-sensitive stream digest
/// (the P1.C5 shadow-dispatch parity instrument).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IntentMetrics {
    #[serde(default)]
    pub attack: u32,
    #[serde(default)]
    pub ranged_attack: u32,
    #[serde(default)]
    pub ranged_mass_attack: u32,
    #[serde(default)]
    pub heal: u32,
    #[serde(default)]
    pub ranged_heal: u32,
    /// Chained FNV-1a over the tick's intent tuples, hex-encoded.
    #[serde(default)]
    pub digest: String,
}

/// Squad-cohesion aggregate for the tick (P2.H3) — the combat-effectiveness signal. Computed from
/// `combat-decision::cohesion::measure` over each squad's live member positions. `f32` fields ⇒
/// `PartialEq`, not `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CohesionMetrics {
    /// Squads with ≥1 living member this tick.
    #[serde(default)]
    pub squad_count: u32,
    /// Squads currently in the Engaged state (the "is there combat?" gate for the military score).
    #[serde(default)]
    pub engaged_squads: u32,
    /// Mean fraction of members in formation (anchor + slot), over all squads. 1.0 = perfectly held.
    #[serde(default)]
    pub avg_in_formation_rate: f32,
    /// Mean member-to-centroid Chebyshev distance (tightness; lower = tighter), over all squads.
    #[serde(default)]
    pub avg_centroid_spread: f32,
    /// Worst single-squad diameter (max pairwise Chebyshev) — the scatter alarm.
    #[serde(default)]
    pub max_pairwise: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CpuMetrics {
    /// `game::cpu::get_used()` at emission time — the metrics system
    /// runs at the END of the system list, so this is near-total tick
    /// CPU (excludes only serialize/memory flush after it).
    pub used: f64,
    /// `game::cpu::limit()`.
    pub limit: f64,
    /// `game::cpu::tick_limit()`.
    pub tick_limit: f64,
    pub bucket: i32,
    /// Slope of the bucket over the emitter's rolling window, in
    /// bucket-units per tick ([`bucket_trend`]). Negative = draining —
    /// the governor input and the death-spiral alarm signal (ADR 0004).
    #[serde(default)]
    pub bucket_trend: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelProgress {
    pub level: u32,
    pub progress: f64,
    pub progress_total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomMetrics {
    /// Room name, e.g. "W7N4".
    pub name: String,
    pub rcl: u32,
    pub rcl_progress: f64,
    pub rcl_progress_total: f64,
    pub energy_available: u32,
    pub energy_capacity_available: u32,
    /// Energy in storage-class structures (storage + terminal).
    #[serde(default)]
    pub stored_energy: u32,
}

/// Loud-failure counters (rewrite-plan non-negotiable #2): cumulative
/// since the last VM reset (pair with [`MetricsBlock::vm_fresh`] when
/// aggregating across restarts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FaultCounters {
    /// Component-pipeline deserialization failures, INCLUDING the
    /// previously-silent base64-decode→empty path.
    #[serde(default)]
    pub deser_failures: u32,
    /// Panics caught by the tick containment boundary (0 until P1.C2).
    #[serde(default)]
    pub panics_caught: u32,
    /// `serialize_world` skips: governor shed (intentional).
    #[serde(default)]
    pub serialize_skipped_shed: u32,
    /// `serialize_world` skips: aborted tick (containment caught).
    #[serde(default)]
    pub serialize_skipped_aborted: u32,
    /// Component-segment chunks used by the last serialize (0002
    /// watermark, routed here per the Inc-2 rescope).
    #[serde(default)]
    pub segment_chunks_used: u32,
    /// The chunk budget (`COMPONENT_SEGMENTS` length).
    #[serde(default)]
    pub segment_chunk_budget: u32,
}

/// Governor state as emitted (P1.B3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernorMetrics {
    /// "normal" | "conserve" | "critical".
    pub tier: String,
}

/// Pathfinding-budget telemetry (P1.B2/B4) — schema reserved, fields
/// additive when the facade lands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PathingMetrics {
    #[serde(default)]
    pub ops_used: u32,
    #[serde(default)]
    pub ops_pool: u32,
    #[serde(default)]
    pub repath_count: u32,
    /// Movement results the rover gave up on this tick (path not
    /// found / stuck past threshold / blocked) — the IBEX-015 signal,
    /// surfaced for telemetry; job-level RECOVERY lands with the Inc-6
    /// transient-tolerance work (ADR 0003 A6).
    #[serde(default)]
    pub move_failures: u32,
    /// Mission-side ops pool this tick (P1.B4; tier-scaled).
    #[serde(default)]
    pub mission_ops_pool: u32,
    /// Mission-side ops consumed from the pool this tick.
    #[serde(default)]
    pub mission_ops_used: u32,
}

impl MetricsBlock {
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    pub fn from_json(s: &str) -> serde_json::Result<MetricsBlock> {
        serde_json::from_str(s)
    }
}

/// Bucket trend in bucket-units per tick over a sample window
/// (oldest-first, one sample per tick): least-squares slope, which
/// tolerates the sawtooth a bursty tick pattern puts on the bucket far
/// better than `(last-first)/n`.
///
/// Windows shorter than 2 samples have no defined slope → 0.0.
pub fn bucket_trend(samples: &[i32]) -> f64 {
    let n = samples.len();
    if n < 2 {
        return 0.0;
    }
    let n_f = n as f64;
    // x = 0..n-1; slope = (n·Σxy − Σx·Σy) / (n·Σx² − (Σx)²)
    let sum_x = (n * (n - 1)) as f64 / 2.0;
    let sum_x2 = ((n - 1) * n * (2 * n - 1)) as f64 / 6.0;
    let sum_y: f64 = samples.iter().map(|&y| y as f64).sum();
    let sum_xy: f64 = samples.iter().enumerate().map(|(x, &y)| x as f64 * y as f64).sum();
    let denom = n_f * sum_x2 - sum_x * sum_x;
    if denom == 0.0 {
        return 0.0;
    }
    (n_f * sum_xy - sum_x * sum_y) / denom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_block() -> MetricsBlock {
        MetricsBlock {
            v: METRICS_SCHEMA_VERSION,
            tick: 12345,
            vm_fresh: true,
            vm_starts: 3,
            cpu: CpuMetrics {
                used: 12.5,
                limit: 100.0,
                tick_limit: 500.0,
                bucket: 9876,
                bucket_trend: -1.25,
            },
            gcl: LevelProgress {
                level: 3,
                progress: 1000.0,
                progress_total: 2000.0,
            },
            gpl: LevelProgress {
                level: 0,
                progress: 0.0,
                progress_total: 1000.0,
            },
            credits: 0.0,
            creeps: 38,
            missions: 12,
            operations: 4,
            rooms: vec![RoomMetrics {
                name: "W7N4".into(),
                rcl: 4,
                rcl_progress: 5000.0,
                rcl_progress_total: 405_000.0,
                energy_available: 800,
                energy_capacity_available: 1300,
                stored_energy: 250_000,
            }],
            faults: FaultCounters {
                deser_failures: 0,
                panics_caught: 0,
                serialize_skipped_shed: 0,
                serialize_skipped_aborted: 0,
                segment_chunks_used: 1,
                segment_chunk_budget: 5,
            },
            governor: Some(GovernorMetrics { tier: "normal".into() }),
            pathing: None,
            intents: None,
            cpu_model: Some(CpuModelMetrics {
                ema_used: 12.5,
                samples: 100,
            }),
            cohesion: Some(CohesionMetrics {
                squad_count: 2,
                engaged_squads: 1,
                avg_in_formation_rate: 0.875,
                avg_centroid_spread: 0.6,
                max_pairwise: 2,
            }),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let block = sample_block();
        let json = block.to_json().unwrap();
        let back = MetricsBlock::from_json(&json).unwrap();
        assert_eq!(block, back);
    }

    /// Additive evolution: a reader must tolerate unknown fields (a
    /// newer writer) and absent optional fields (an older writer).
    #[test]
    fn reader_tolerates_other_writer_versions() {
        // Newer writer: extra field.
        let mut v: serde_json::Value = serde_json::to_value(sample_block()).unwrap();
        v["future_field"] = serde_json::json!({"x": 1});
        let parsed = MetricsBlock::from_json(&v.to_string()).unwrap();
        assert_eq!(parsed.tick, 12345);
        // Older writer: optionals absent.
        let minimal = r#"{
            "v": 1, "tick": 7,
            "cpu": {"used": 1.0, "limit": 100.0, "tick_limit": 500.0, "bucket": 10000},
            "gcl": {"level": 1, "progress": 0.0, "progress_total": 1000.0},
            "gpl": {"level": 0, "progress": 0.0, "progress_total": 1000.0}
        }"#;
        let parsed = MetricsBlock::from_json(minimal).unwrap();
        assert_eq!(parsed.tick, 7);
        assert!(!parsed.vm_fresh);
        assert_eq!(parsed.faults, FaultCounters::default());
        assert!(parsed.governor.is_none());
        assert_eq!(parsed.cpu.bucket_trend, 0.0);
    }

    /// The version constant is load-bearing for readers — pin it.
    #[test]
    fn schema_version_is_pinned() {
        assert_eq!(METRICS_SCHEMA_VERSION, 1);
    }

    #[test]
    fn bucket_trend_slopes() {
        // Steady drain of 5/tick.
        let drain: Vec<i32> = (0..50).map(|i| 10_000 - 5 * i).collect();
        assert!((bucket_trend(&drain) + 5.0).abs() < 1e-9, "{}", bucket_trend(&drain));
        // Steady fill.
        let fill: Vec<i32> = (0..50).map(|i| 5_000 + 3 * i).collect();
        assert!((bucket_trend(&fill) - 3.0).abs() < 1e-9);
        // Flat.
        assert_eq!(bucket_trend(&[8000; 30]), 0.0);
        // Sawtooth around flat: slope ~0 despite per-tick jumps.
        let saw: Vec<i32> = (0..40).map(|i| 8000 + if i % 2 == 0 { 100 } else { -100 }).collect();
        assert!(bucket_trend(&saw).abs() < 6.0, "{}", bucket_trend(&saw));
        // Degenerate windows.
        assert_eq!(bucket_trend(&[]), 0.0);
        assert_eq!(bucket_trend(&[5000]), 0.0);
    }
}
