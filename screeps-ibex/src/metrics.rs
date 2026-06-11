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
//! [`FaultCounters`] events are recorded from anywhere via the
//! `record_*` free functions (atomics — wasm is single-threaded, and
//! atomics keep the host test lane safe). Counters are cumulative per
//! VM lifetime; `vm_fresh` marks the first emitted block after a reset
//! so the harness can segment them by restart.
//!
//! ## Bucket window
//!
//! [`MetricsState`] keeps a rolling bucket window in the heap (lost on
//! reset — it refills within [`BUCKET_WINDOW`] ticks). Its trend is the
//! CpuGovernor's input (P1.B3) and the death-spiral alarm signal
//! (ADR 0004); the game loop refreshes the governor snapshot from this
//! window at tick start via [`bucket_window_trend`].

use crate::cpugovernor;
use crate::memorysystem::*;
use crate::missions::data::MissionData;
use crate::operations::data::OperationData;
use crate::room::data::*;
use crate::segments::{COMPONENT_SEGMENTS, METRICS_SEGMENT};
use screeps::*;
use screeps_ibex_metrics::*;
use specs::prelude::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};

/// Rolling bucket-sample window (one sample per tick). 100 ticks reacts
/// to a drain within a respawn-survivable horizon while smoothing the
/// per-tick sawtooth.
pub const BUCKET_WINDOW: usize = 100;

static DESER_FAILURES: AtomicU32 = AtomicU32::new(0);
static PANICS_CAUGHT: AtomicU32 = AtomicU32::new(0);
static SERIALIZE_SKIPPED_SHED: AtomicU32 = AtomicU32::new(0);
static SERIALIZE_SKIPPED_ABORTED: AtomicU32 = AtomicU32::new(0);
static SEGMENT_CHUNKS_USED: AtomicU32 = AtomicU32::new(0);

/// A component-pipeline deserialization failure, INCLUDING the
/// base64/decompress decode path (previously silent: decode→empty).
pub fn record_deser_failure() {
    DESER_FAILURES.fetch_add(1, Ordering::Relaxed);
}

/// A panic caught by the tick containment boundary (wired at P1.C2).
#[allow(dead_code)]
pub fn record_panic_caught() {
    PANICS_CAUGHT.fetch_add(1, Ordering::Relaxed);
}

/// `serialize_world` intentionally shed by the governor (P1.C2/C5).
#[allow(dead_code)]
pub fn record_serialize_skipped_shed() {
    SERIALIZE_SKIPPED_SHED.fetch_add(1, Ordering::Relaxed);
}

/// `serialize_world` lost to an aborted tick (containment, P1.C2).
#[allow(dead_code)]
pub fn record_serialize_skipped_aborted() {
    SERIALIZE_SKIPPED_ABORTED.fetch_add(1, Ordering::Relaxed);
}

/// The 0002 chunk watermark, routed into the metrics block (Inc-2
/// rescope): how many component segments the last serialize consumed.
pub fn record_segment_chunks(used: u32) {
    SEGMENT_CHUNKS_USED.store(used, Ordering::Relaxed);
}

fn fault_counters() -> FaultCounters {
    FaultCounters {
        deser_failures: DESER_FAILURES.load(Ordering::Relaxed),
        panics_caught: PANICS_CAUGHT.load(Ordering::Relaxed),
        serialize_skipped_shed: SERIALIZE_SKIPPED_SHED.load(Ordering::Relaxed),
        serialize_skipped_aborted: SERIALIZE_SKIPPED_ABORTED.load(Ordering::Relaxed),
        segment_chunks_used: SEGMENT_CHUNKS_USED.load(Ordering::Relaxed),
        segment_chunk_budget: COMPONENT_SEGMENTS.len() as u32,
    }
}

/// Heap-resident emitter state: the bucket window and the fresh-VM
/// flag. Default (= post-reset) state starts fresh with an empty window.
pub struct MetricsState {
    bucket_window: VecDeque<i32>,
    fresh: bool,
}

impl Default for MetricsState {
    fn default() -> Self {
        MetricsState {
            bucket_window: VecDeque::with_capacity(BUCKET_WINDOW),
            fresh: true,
        }
    }
}

impl MetricsState {
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
}

/// Tick-start hook (game_loop): sample the bucket, refresh the
/// CpuGovernor snapshot from the window trend. Runs BEFORE dispatch so
/// every system reads a consistent governor view for the whole tick.
pub fn tick_start(world: &mut World) {
    let mut state = world.entry::<MetricsState>().or_insert_with(MetricsState::default);
    let bucket = game::cpu::bucket();
    state.push_bucket_sample(bucket);
    cpugovernor::refresh(bucket, state.trend(), game::cpu::tick_limit());
}

#[derive(SystemData)]
pub struct MetricsSystemData<'a> {
    entities: Entities<'a>,
    room_data: ReadStorage<'a, RoomData>,
    mission_data: ReadStorage<'a, MissionData>,
    operation_data: ReadStorage<'a, OperationData>,
    state: Write<'a, MetricsState>,
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
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
            faults: fault_counters(),
            governor: Some(GovernorMetrics {
                tier: cpugovernor::tier().as_str().to_string(),
            }),
            pathing: None,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MetricsSystem {
    type SystemData = MetricsSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
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

    /// Fault counters round-trip through the atomics into the block shape.
    #[test]
    fn fault_counters_capture_recorded_events() {
        // Atomics are process-global: read the baseline first so this
        // test composes with any other test that records events.
        let before = fault_counters();
        record_deser_failure();
        record_segment_chunks(3);
        let after = fault_counters();
        assert_eq!(after.deser_failures, before.deser_failures + 1);
        assert_eq!(after.segment_chunks_used, 3);
        assert_eq!(after.segment_chunk_budget, COMPONENT_SEGMENTS.len() as u32);
    }
}
