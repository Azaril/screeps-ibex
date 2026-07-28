use super::data::JobData;
use super::utility::dismantlebehavior::BreachPlanCache;
use crate::cpugovernor::GovernorSnapshot;
use crate::creep::CreepOwner;
use crate::energy_stress::EnergyLeakStats;
use crate::entitymappingsystem::*;
use crate::features::Features;
use crate::intents::IntentRecorder;
use crate::military::economy::EconomySnapshot;
use crate::military::squad::SquadContext;
use crate::pathing::hauldistance::HaulDistanceService;
use crate::pathing::pathfinderservice::PathfinderService;
use crate::repairqueue::RepairQueue;
use crate::room::data::*;
use crate::room::visibilitysystem::VisibilityQueue;
use crate::transfer::transfersystem::*;
use crate::visualization::SummaryContent;
use screeps::*;
use screeps_rover::*;
use specs::prelude::*;

#[derive(specs::SystemData)]
pub struct JobSystemData<'a> {
    creep_owners: ReadStorage<'a, CreepOwner>,
    jobs: WriteStorage<'a, JobData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    transfer_queue: Write<'a, TransferQueue>,
    room_data: ReadStorage<'a, RoomData>,
    movement: WriteExpect<'a, MovementData<Entity>>,
    movement_results: ReadExpect<'a, MovementResults<Entity>>,
    mapping: Read<'a, EntityMappingData>,
    squad_contexts: WriteStorage<'a, SquadContext>,
    repair_queue: Read<'a, RepairQueue>,
    economy: Read<'a, EconomySnapshot>,
    features: Read<'a, Features>,
    energy_leak: Write<'a, EnergyLeakStats>,
    visibility_queue: Write<'a, VisibilityQueue>,
    pathfinder: Write<'a, PathfinderService>,
    /// ADR 0044 step 2 — the shared per-room cost-matrix cache (also driven by `MovementUpdateSystem`;
    /// segment-loaded, structures built once/tick) + the `(pickup,sink)` routed-distance memo. The
    /// hauler market pass prices the haul leg on the true routed distance through these (via a
    /// `RoverDistanceOracle`), matching the sim mover.
    cost_matrix_cache: WriteExpect<'a, CostMatrixCache>,
    haul_distance_service: Write<'a, HaulDistanceService>,
    intent_recorder: Write<'a, IntentRecorder>,
    breach_cache: Write<'a, BreachPlanCache>,
    /// The tick's CPU-pressure view (ADR 0004) — read by the hauler re-match cadence
    /// (ADR 0007 item 2 via ADR 0040 M3; the cadence POLICY lives in
    /// `screeps_econ_decision::cadence`, this is the adapter-side governor read).
    governor: Read<'a, GovernorSnapshot>,
    /// ADR 0040 M5a — the live e/t market readout (§D8 #5): the opportunity floor + top unmet
    /// bids the hauling pass publishes each tick, exported to seg-57 by `metrics.rs`.
    market_bids: Write<'a, crate::transfer::transfersystem::MarketBidSummary>,
}

pub struct JobExecutionSystemData<'a> {
    pub updater: &'a Read<'a, LazyUpdate>,
    pub entities: &'a Entities<'a>,
    pub room_data: &'a ReadStorage<'a, RoomData>,
    pub squad_contexts: &'a WriteStorage<'a, SquadContext>,
    pub repair_queue: &'a RepairQueue,
    pub economy: &'a EconomySnapshot,
    pub features: &'a Features,
    pub governor: GovernorSnapshot,
    /// ADR 0044 A3 (Defect 2) — the per-room opportunity floor + top unmet bids published by
    /// `publish_market_floor` at the top of `RunJobSystem` (BEFORE the per-creep loop). Read by
    /// the consumer jobs' Use-lane pickup admission (upgraders/builders shed their draw when their
    /// sink bid falls below the floor). Empty during `PreRunJobSystem` (the floor isn't published
    /// yet; pre-run does no consumer selection).
    pub market_bids: &'a MarketBidSummary,
}

pub struct JobExecutionRuntimeData<'a> {
    pub creep_entity: Entity,
    pub owner: &'a Creep,
    pub mapping: &'a EntityMappingData,
    pub transfer_queue: &'a mut TransferQueue,
    pub movement: &'a mut MovementData<Entity>,
    pub movement_results: &'a MovementResults<Entity>,
    pub visibility_queue: &'a mut VisibilityQueue,
    pub pathfinder: &'a mut PathfinderService,
    /// ADR 0044 step 2 — the pieces the hauler market pass builds a `RoverDistanceOracle` from to
    /// price the haul leg on true routed distance (shared cost matrices + the distance memo).
    pub cost_matrix_cache: &'a mut CostMatrixCache,
    pub haul_distance_service: &'a mut HaulDistanceService,
    pub intent_recorder: &'a mut IntentRecorder,
    pub breach_cache: &'a mut BreachPlanCache,
    /// Repair-leak telemetry counters (ADR 0040 §D6 `repair_leak_e` — always-on).
    pub energy_leak: &'a mut EnergyLeakStats,
}

pub struct JobDescribeData<'a> {
    pub _owner: &'a Creep,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait Job {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

    /// Produce a structured summary for the visualization overlay.
    /// Reads only `self`; no system data required.
    fn summarize(&self) -> SummaryContent {
        SummaryContent::Text("Job".to_string())
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData);
}

pub struct PreRunJobSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // The market floor is not published until `RunJobSystem`; pre-run does no consumer
        // selection, so an empty summary (floor 0 ⇒ everything admits) is correct here.
        let empty_market_bids = MarketBidSummary::default();
        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            squad_contexts: &data.squad_contexts,
            repair_queue: &data.repair_queue,
            economy: &data.economy,
            features: &data.features,
            governor: *data.governor,
            market_bids: &empty_market_bids,
        };

        for (creep_entity, creep, job_data) in (&data.entities, &data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    creep_entity,
                    owner: &owner,
                    mapping: &data.mapping,
                    transfer_queue: &mut data.transfer_queue,
                    movement: &mut data.movement,
                    movement_results: &data.movement_results,
                    visibility_queue: &mut data.visibility_queue,
                    pathfinder: &mut data.pathfinder,
                    cost_matrix_cache: &mut data.cost_matrix_cache,
                    haul_distance_service: &mut data.haul_distance_service,
                    intent_recorder: &mut data.intent_recorder,
                    breach_cache: &mut data.breach_cache,
                    energy_leak: &mut data.energy_leak,
                };

                job_data.as_job().pre_run_job(&system_data, &mut runtime_data);
            }
        }
    }
}

pub struct RunJobSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunJobSystem {
    type SystemData = JobSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // The per-tick TransferSnapshot (ADR 0040 M3 / ADR 0007 Q5 item 1): built ONCE at the
        // top of the hauling pass — every generator flushes here (generation provably paid
        // once; missions already ran and registered), and every hauler selection below runs
        // against the frozen view, mutating only the booking layer. Cleared with the queue by
        // `TransferQueueUpdateSystem`.
        {
            let generator_data = TransferQueueGeneratorData {
                cause: "Econ Snapshot",
                room_data: &data.room_data,
            };
            data.transfer_queue.build_econ_snapshot(&generator_data);
            // ADR 0040 M5a: publish the live market floor + top unmet bids (§D8 #5). The demand
            // is fully materialized by the snapshot build; the floor read off the numeric-bid
            // deposit keys is the same quantity every hauler selection admits against this tick.
            data.transfer_queue.publish_market_floor(&mut data.market_bids);
        }
        // ADR 0044 step 2: start the haul-distance CPU-benchmark window for this tick (the ship gate).
        data.haul_distance_service.reset_tick_counters();

        let system_data = JobExecutionSystemData {
            updater: &data.updater,
            entities: &data.entities,
            room_data: &data.room_data,
            squad_contexts: &data.squad_contexts,
            repair_queue: &data.repair_queue,
            economy: &data.economy,
            features: &data.features,
            governor: *data.governor,
            // The floor published above (line ~169) — read by consumer Use-lane pickup admission.
            market_bids: &data.market_bids,
        };

        for (creep_entity, creep, job_data) in (&data.entities, &data.creep_owners, &mut data.jobs).join() {
            if let Some(owner) = creep.owner.resolve() {
                let mut runtime_data = JobExecutionRuntimeData {
                    creep_entity,
                    owner: &owner,
                    mapping: &data.mapping,
                    transfer_queue: &mut data.transfer_queue,
                    movement: &mut data.movement,
                    movement_results: &data.movement_results,
                    visibility_queue: &mut data.visibility_queue,
                    pathfinder: &mut data.pathfinder,
                    cost_matrix_cache: &mut data.cost_matrix_cache,
                    haul_distance_service: &mut data.haul_distance_service,
                    intent_recorder: &mut data.intent_recorder,
                    breach_cache: &mut data.breach_cache,
                    energy_leak: &mut data.energy_leak,
                };

                job_data.as_job().run_job(&system_data, &mut runtime_data);
            }
        }
    }
}
