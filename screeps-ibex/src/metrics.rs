//! Seg-57 metrics emission (P1.A1, ADR 0006) — the always-on, versioned
//! telemetry block, decoupled from the debug/visualization flag.
//!
//! The schema is the `screeps-ibex-metrics` crate (shared with the eval
//! harness reader); this module is the WRITER glue: gather from the game
//! API + world, serialize one [`MetricsBlock`] JSON into
//! [`METRICS_SEGMENT`] each tick.
//!
//! ## Fault counters
//!
//! [`FaultCounters`] events are recorded on [`MetricsState`] (a specs
//! Resource — statics-review M3; the writer systems carry
//! `Write<MetricsState>`). Counters are cumulative per ENVIRONMENT
//! lifetime: they reset when the world is rebuilt (VM restart or a
//! deliberate `reset.environment`) — the same lifetime as `vm_starts`
//! bumping, so the harness segments them by restart exactly as before.
//! `vm_fresh` marks the first emitted block after a reset.
//!
//! ## Bucket window
//!
//! [`MetricsState`] keeps a rolling bucket window in the heap (lost on
//! reset — it refills within [`BUCKET_WINDOW`] ticks). Its trend is the
//! CpuGovernor's input (P1.B3) and the death-spiral alarm signal
//! (ADR 0004); the game loop refreshes the governor snapshot from this
//! window at tick start via [`bucket_window_trend`].

use crate::cpugovernor::GovernorSnapshot;
use crate::memorysystem::*;
use crate::missions::data::MissionData;
use crate::operations::data::OperationData;
use crate::pathing::pathfinderservice::PathfinderService;
use crate::room::data::*;
use crate::segments::{COMPONENT_SEGMENTS, METRICS_SEGMENT};
use screeps::*;
use screeps_ibex_metrics::*;
use specs::prelude::*;
use std::collections::VecDeque;

/// Rolling bucket-sample window (one sample per tick). 100 ticks reacts
/// to a drain within a respawn-survivable horizon while smoothing the
/// per-tick sawtooth.
pub const BUCKET_WINDOW: usize = 100;

/// EMA smoothing factor for the per-tick CPU-used model (≈100-tick
/// effective window: `2/(N+1)` with N≈100). Slow on purpose — the
/// expansion cap should track sustained cost, not per-tick spikes.
const CPU_EMA_ALPHA: f64 = 0.02;

/// Samples required before the CPU model is trusted for the room cap.
/// Below this the claim pipeline falls back to its configured constant.
const MIN_CPU_SAMPLES: u32 = 20;

/// Heap-resident emitter state: the bucket window, the fresh-VM flag,
/// the Memory-persisted VM-start counter, and the fault/movement
/// telemetry counters (statics-review M3 — formerly module atomics).
/// Default (= post-reset) state starts fresh with an empty window and
/// zeroed counters; everything here has environment lifetime.
pub struct MetricsState {
    bucket_window: VecDeque<i32>,
    fresh: bool,
    /// Bumped in `Memory._metrics.vm_starts` once per VM lifetime
    /// (Memory survives resets; the heap doesn't) — the restart
    /// counter samplers can't miss, unlike the one-tick `vm_fresh`.
    vm_starts: u32,
    /// Cumulative (per environment) deserialization failures.
    deser_failures: u32,
    /// Seeded from `Memory._metrics.aborted_ticks` — the JS loader is
    /// the containment boundary (P1.C1); there is no Rust-side
    /// recorder by design. An aborted tick is both a caught panic and
    /// a lost serialize.
    panics_caught: u32,
    /// A shed-skip of serialize is unreachable by construction
    /// (never-shed set, ADR 0004); the counter exists so the schema
    /// can prove it stays zero.
    serialize_skipped_shed: u32,
    serialize_skipped_aborted: u32,
    /// The 0002 chunk watermark (last serialize's segment use).
    segment_chunks_used: u32,
    // Per-tick movement telemetry (P1.B2), last-write-wins.
    movement_ops_cap: u32,
    movement_ops_consumed: u32,
    movement_repaths: u32,
    movement_failures: u32,
    /// Self-tuning CPU cost model: EMA of per-tick `game::cpu::get_used()`
    /// (None until the first sample / load-seed) and the fold count. Drives
    /// the dynamic expansion room cap; persisted in the seg-57 block and
    /// re-seeded on VM start via [`load_cpu_model`].
    cpu_used_ema: Option<f64>,
    cpu_samples: u32,
}

impl Default for MetricsState {
    fn default() -> Self {
        MetricsState {
            bucket_window: VecDeque::with_capacity(BUCKET_WINDOW),
            fresh: true,
            vm_starts: 0,
            deser_failures: 0,
            panics_caught: 0,
            serialize_skipped_shed: 0,
            serialize_skipped_aborted: 0,
            segment_chunks_used: 0,
            movement_ops_cap: 0,
            movement_ops_consumed: 0,
            movement_repaths: 0,
            movement_failures: 0,
            cpu_used_ema: None,
            cpu_samples: 0,
        }
    }
}

/// Increment and persist the VM-start counter (called once per VM
/// lifetime from [`tick_start`]).
fn bump_vm_starts() -> u32 {
    let next = crate::memory_helper::path_f64("_metrics.vm_starts").unwrap_or(0.0) as u32 + 1;
    crate::memory_helper::path_set("_metrics.vm_starts", next as f64);
    next
}

impl MetricsState {
    /// Per-tick movement telemetry (P1.B2): recorded by the movement
    /// system after `process()`, emitted in the block's `pathing` section.
    pub fn record_movement_stats(&mut self, stats: screeps_rover::MovementTickStats) {
        self.movement_ops_cap = stats.ops_budget_cap;
        self.movement_ops_consumed = stats.ops_consumed;
        self.movement_repaths = stats.repaths;
    }

    /// Movement results the rover gave up on this tick (P1.D6 / IBEX-015:
    /// the previously-dead detection signal, surfaced; recovery = Inc 6).
    pub fn record_movement_failures(&mut self, count: u32) {
        self.movement_failures = count;
    }

    /// A component-pipeline deserialization failure, INCLUDING the
    /// base64/decompress decode path (previously silent: decode→empty).
    pub fn record_deser_failure(&mut self) {
        self.deser_failures += 1;
    }

    /// The 0002 chunk watermark, routed into the metrics block (Inc-2
    /// rescope): how many component segments the last serialize consumed.
    pub fn record_segment_chunks(&mut self, used: u32) {
        self.segment_chunks_used = used;
    }

    fn fault_counters(&self) -> FaultCounters {
        FaultCounters {
            deser_failures: self.deser_failures,
            panics_caught: self.panics_caught,
            serialize_skipped_shed: self.serialize_skipped_shed,
            serialize_skipped_aborted: self.serialize_skipped_aborted,
            segment_chunks_used: self.segment_chunks_used,
            segment_chunk_budget: COMPONENT_SEGMENTS.len() as u32,
        }
    }

    pub fn push_bucket_sample(&mut self, bucket: i32) {
        if self.bucket_window.len() == BUCKET_WINDOW {
            self.bucket_window.pop_front();
        }
        self.bucket_window.push_back(bucket);
    }

    /// Least-squares slope over the window (bucket units per tick).
    pub fn trend(&self) -> f64 {
        let samples: Vec<i32> = self.bucket_window.iter().copied().collect();
        bucket_trend(&samples)
    }

    /// Fold one tick's CPU-used sample into the EMA.
    pub fn push_cpu_used(&mut self, used: f64) {
        if !used.is_finite() || used < 0.0 {
            return;
        }
        self.cpu_used_ema = Some(match self.cpu_used_ema {
            Some(ema) => CPU_EMA_ALPHA * used + (1.0 - CPU_EMA_ALPHA) * ema,
            None => used,
        });
        self.cpu_samples = self.cpu_samples.saturating_add(1);
    }

    /// The CPU-used EMA once enough samples have accumulated to trust it;
    /// `None` while still warming up (the claim cap falls back to config).
    pub fn cpu_used_estimate(&self) -> Option<f64> {
        match self.cpu_used_ema {
            Some(ema) if self.cpu_samples >= MIN_CPU_SAMPLES => Some(ema),
            _ => None,
        }
    }

    /// Re-seed the CPU model from a persisted block on VM start, so the
    /// estimate survives resets instead of warming from cold.
    pub fn seed_cpu_model(&mut self, ema_used: f64, samples: u32) {
        if ema_used.is_finite() && ema_used >= 0.0 && samples > 0 {
            self.cpu_used_ema = Some(ema_used);
            self.cpu_samples = samples;
        }
    }

    /// Snapshot of the CPU model for persistence in the metrics block.
    pub fn cpu_model_metrics(&self) -> Option<screeps_ibex_metrics::CpuModelMetrics> {
        self.cpu_used_ema.map(|ema_used| screeps_ibex_metrics::CpuModelMetrics {
            ema_used,
            samples: self.cpu_samples,
        })
    }
}

/// The tick's expansion CPU budget, derived at `tick_start` from the CPU
/// model and the per-tick limit. Room-count-independent (the claim op
/// divides by its own owned-room count), so it is safe to compute before
/// dispatch. A specs `Resource`, read like [`GovernorSnapshot`].
#[derive(Debug, Clone, Copy)]
pub struct CpuBudget {
    /// EMA of per-tick CPU used, once warm (`None` while warming up).
    pub cpu_used_estimate: Option<f64>,
    /// `game::cpu::limit()` — the sustainable per-tick budget (NOT
    /// `tick_limit`, which folds in the burst bucket).
    pub cpu_limit: f64,
}

impl Default for CpuBudget {
    fn default() -> Self {
        CpuBudget {
            cpu_used_estimate: None,
            cpu_limit: 0.0,
        }
    }
}

/// Load callback (registered on `METRICS_SEGMENT`): re-seed the CPU cost
/// model from the last persisted block so the dynamic room cap survives VM
/// resets. `MemoryArbiter` is out of the world during callbacks, so read the
/// segment directly via `raw_memory` (mirrors `stats_history::load_stats_history`).
pub fn load_cpu_model(world: &mut World) {
    let model = screeps::raw_memory::segments().get(METRICS_SEGMENT as u8).and_then(|raw| {
        if raw.is_empty() {
            None
        } else {
            MetricsBlock::from_json(&raw).ok().and_then(|block| block.cpu_model)
        }
    });

    if let Some(model) = model {
        world
            .entry::<MetricsState>()
            .or_insert_with(MetricsState::default)
            .seed_cpu_model(model.ema_used, model.samples);
    }
}

/// Tick-start hook (game_loop): sample the bucket, then insert the
/// tick's [`GovernorSnapshot`] Resource and re-arm the
/// [`PathfinderService`] pool at its tier (statics-review M1/M4). Runs
/// BEFORE dispatch so every system reads a consistent governor view
/// for the whole tick.
pub fn tick_start(world: &mut World) {
    world
        .entry::<crate::intents::IntentRecorder>()
        .or_insert_with(Default::default)
        .reset();
    let (bucket, trend) = {
        let mut state = world.entry::<MetricsState>().or_insert_with(MetricsState::default);
        if state.fresh && state.vm_starts == 0 {
            state.vm_starts = bump_vm_starts();
            // Containment accounting (P1.C2): the loader counts caught
            // aborts in Memory (the heap dies with the halt). An aborted
            // tick is both a caught panic and a lost serialize.
            let aborted = crate::memory_helper::path_f64("_metrics.aborted_ticks").unwrap_or(0.0) as u32;
            state.panics_caught = aborted;
            state.serialize_skipped_aborted = aborted;
        }
        let bucket = game::cpu::bucket();
        state.push_bucket_sample(bucket);
        (bucket, state.trend())
    };
    let snapshot = GovernorSnapshot::compute(bucket, trend, game::cpu::tick_limit());
    world.insert(snapshot);
    world
        .entry::<PathfinderService>()
        .or_insert_with(PathfinderService::default)
        .begin_tick(snapshot.tier);

    // Publish the expansion CPU budget for the claim pipeline (P-expansion):
    // the warm CPU-used EMA + the sustainable per-tick limit. Read like the
    // governor snapshot.
    let cpu_used_estimate = world
        .entry::<MetricsState>()
        .or_insert_with(MetricsState::default)
        .cpu_used_estimate();
    world.insert(CpuBudget {
        cpu_used_estimate,
        cpu_limit: game::cpu::limit() as f64,
    });
}

#[derive(SystemData)]
pub struct MetricsSystemData<'a> {
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    mission_data: ReadStorage<'a, MissionData>,
    operation_data: ReadStorage<'a, OperationData>,
    state: Write<'a, MetricsState>,
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
    governor: Read<'a, GovernorSnapshot>,
    pathfinder: Read<'a, PathfinderService>,
    intents: Read<'a, crate::intents::IntentRecorder>,
}

pub struct MetricsSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MetricsSystem {
    fn room_metrics(data: &MetricsSystemData) -> Vec<RoomMetrics> {
        (&data.entities, &data.room_data)
            .join()
            .filter(|(_, room_data)| {
                room_data
                    .get_dynamic_visibility_data()
                    .map(|v| v.visible() && v.owner().mine())
                    .unwrap_or(false)
            })
            .filter_map(|(_, room_data)| {
                let room = game::rooms().get(room_data.name)?;
                let controller = room.controller()?;
                let stored_energy = room_data
                    .get_structures()
                    .map(|structures| {
                        structures
                            .storages()
                            .iter()
                            .map(|s| s.store().get_used_capacity(Some(ResourceType::Energy)))
                            .sum::<u32>()
                            + structures
                                .terminals()
                                .iter()
                                .map(|t| t.store().get_used_capacity(Some(ResourceType::Energy)))
                                .sum::<u32>()
                    })
                    .unwrap_or(0);
                Some(RoomMetrics {
                    name: room_data.name.to_string(),
                    rcl: controller.level() as u32,
                    rcl_progress: controller.progress().unwrap_or(0) as f64,
                    rcl_progress_total: controller.progress_total().unwrap_or(0) as f64,
                    energy_available: room.energy_available(),
                    energy_capacity_available: room.energy_capacity_available(),
                    stored_energy,
                })
            })
            .collect()
    }

    fn build_block(data: &MetricsSystemData, vm_fresh: bool) -> MetricsBlock {
        MetricsBlock {
            v: METRICS_SCHEMA_VERSION,
            tick: game::time(),
            vm_fresh,
            vm_starts: data.state.vm_starts,
            cpu: CpuMetrics {
                used: game::cpu::get_used(),
                limit: game::cpu::limit() as f64,
                tick_limit: game::cpu::tick_limit(),
                bucket: game::cpu::bucket(),
                bucket_trend: data.state.trend(),
            },
            gcl: LevelProgress {
                level: game::gcl::level(),
                progress: game::gcl::progress(),
                progress_total: game::gcl::progress_total(),
            },
            gpl: LevelProgress {
                level: game::gpl::level(),
                progress: game::gpl::progress(),
                progress_total: game::gpl::progress_total(),
            },
            credits: game::market::credits(),
            creeps: game::creeps().keys().count() as u32,
            missions: data.mission_data.join().count() as u32,
            operations: data.operation_data.join().count() as u32,
            rooms: Self::room_metrics(data),
            faults: data.state.fault_counters(),
            governor: Some(GovernorMetrics {
                tier: data.governor.tier.as_str().to_string(),
            }),
            pathing: Some({
                let (mission_pool, mission_used) = data.pathfinder.snapshot();
                PathingMetrics {
                    ops_used: data.state.movement_ops_consumed,
                    ops_pool: data.state.movement_ops_cap,
                    repath_count: data.state.movement_repaths,
                    move_failures: data.state.movement_failures,
                    mission_ops_pool: mission_pool,
                    mission_ops_used: mission_used,
                }
            }),
            intents: Some({
                let (counts, digest) = data.intents.snapshot();
                IntentMetrics {
                    attack: counts[0],
                    ranged_attack: counts[1],
                    ranged_mass_attack: counts[2],
                    heal: counts[3],
                    ranged_heal: counts[4],
                    digest: format!("{digest:016x}"),
                }
            }),
            cpu_model: data.state.cpu_model_metrics(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MetricsSystem {
    type SystemData = MetricsSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // Sample per-tick CPU UNCONDITIONALLY (before the segment gate), so the
        // cost model accrues every tick regardless of whether the block is
        // written. The metrics system runs near the end of the list, so this
        // captures near-total tick CPU (excludes only render/persist/serialize
        // after it) — a consistent, slightly-optimistic sample the headroom
        // factor absorbs.
        data.state.push_cpu_used(game::cpu::get_used());

        data.memory_arbiter.request(METRICS_SEGMENT);

        if data.memory_arbiter.is_active(METRICS_SEGMENT) {
            let vm_fresh = data.state.fresh;
            let block = Self::build_block(&data, vm_fresh);
            match block.to_json() {
                Ok(json) => {
                    data.memory_arbiter.set(METRICS_SEGMENT, &json);
                    data.state.fresh = false;
                }
                Err(err) => {
                    // Loud, once-per-cause: a schema that cannot
                    // serialize is a bug, not a runtime condition.
                    log::error!("Metrics block serialization failed: {}", err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window evicts oldest-first and the trend tracks a drain.
    #[test]
    fn bucket_window_rolls_and_trends() {
        let mut state = MetricsState::default();
        assert!(state.fresh);
        assert_eq!(state.trend(), 0.0);
        for i in 0..(BUCKET_WINDOW as i32 + 50) {
            state.push_bucket_sample(10_000 - 4 * i);
        }
        assert_eq!(state.bucket_window.len(), BUCKET_WINDOW);
        let trend = state.trend();
        assert!((trend + 4.0).abs() < 1e-6, "expected -4/tick, got {trend}");
    }

    /// CPU model: warms up after enough samples, converges to a steady load,
    /// and stays cold (no estimate) until the sample floor is reached.
    #[test]
    fn cpu_model_warms_up_and_estimates() {
        let mut state = MetricsState::default();
        assert_eq!(state.cpu_used_estimate(), None);

        // One sample seeds the EMA but the estimate stays cold.
        state.push_cpu_used(10.0);
        assert_eq!(state.cpu_used_estimate(), None);

        // Constant load: EMA stays at the load value; estimate appears once the
        // sample floor is crossed.
        for _ in 0..MIN_CPU_SAMPLES {
            state.push_cpu_used(10.0);
        }
        let est = state.cpu_used_estimate().expect("warm");
        assert!((est - 10.0).abs() < 1e-6, "{est}");

        // Garbage samples are ignored (no NaN poisoning).
        state.push_cpu_used(f64::NAN);
        state.push_cpu_used(-5.0);
        assert!((state.cpu_used_estimate().unwrap() - 10.0).abs() < 1e-6);
    }

    /// The model seeds from a persisted block so the cap survives VM resets.
    #[test]
    fn cpu_model_seeds_from_persisted_values() {
        let mut state = MetricsState::default();
        state.seed_cpu_model(42.0, 100);
        assert_eq!(state.cpu_used_estimate(), Some(42.0));
        let persisted = state.cpu_model_metrics().expect("present");
        assert_eq!(persisted.ema_used, 42.0);
        assert_eq!(persisted.samples, 100);

        // A zero-sample seed is rejected (nothing to trust).
        let mut empty = MetricsState::default();
        empty.seed_cpu_model(42.0, 0);
        assert_eq!(empty.cpu_used_estimate(), None);
    }

    /// Fault counters round-trip through the state into the block
    /// shape. Per-instance (M3) — no process-global baseline dance.
    #[test]
    fn fault_counters_capture_recorded_events() {
        let mut state = MetricsState::default();
        state.record_deser_failure();
        state.record_segment_chunks(3);
        let counters = state.fault_counters();
        assert_eq!(counters.deser_failures, 1);
        assert_eq!(counters.segment_chunks_used, 3);
        assert_eq!(counters.panics_caught, 0);
        assert_eq!(counters.segment_chunk_budget, COMPONENT_SEGMENTS.len() as u32);
        // A second instance is unaffected.
        assert_eq!(MetricsState::default().fault_counters().deser_failures, 0);
    }
}
