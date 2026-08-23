#![allow(dead_code)] // TRIAGE 2026-08-23 (ws-triage.md): the describe/visualize overlay layer is inert until ADR 0016 dispatches it — file-level by design.
use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::haulbehavior::*;
use super::utility::movebehavior::*;
use super::utility::repair::*;
use super::utility::repairbehavior::*;
use super::utility::waitbehavior::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct HaulJobContext {
    pickup_rooms: EntityVec<Entity>,
    delivery_rooms: EntityVec<Entity>,
    allow_repair: bool,
    storage_delivery_only: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum HaulState {
        Idle,
        Pickup { withdrawl: TransferWithdrawTicket, deposits: Vec<TransferDepositTicket> },
        Delivery { deposits: Vec<TransferDepositTicket> },
        Wait { ticks: u32 },
        MoveToRoom { room_name: RoomName },
        /// Fleeing a nearby invader / Source Keeper (P2.K0). Owns the move so it
        /// never competes with hauling; returns to `Idle` once clear.
        Flee,
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        Idle, MoveToRoom, Wait, Flee => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, MoveToRoom, Wait, Flee => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState>;
    }
);

impl Idle {
    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        // P2.K0: a nearby invader/keeper (remote room) transitions us into Flee.
        if is_threatened(tick_context) {
            return Some(HaulState::flee());
        }

        // The governor-gated re-match cadence (ADR 0007 Q5 item 2 via ADR 0040 M3): the POLICY
        // lives in `screeps_econ_decision::cadence`; the governor tier read stays here. At
        // Normal this is exactly the pre-M3 behavior (attempt every Idle tick, wait(5) on a
        // failed match); Conserve stretches the failed-match backoff; Critical skips idle
        // re-selection for the backoff window (committed plans — the Pickup/Delivery states —
        // never consult this: hauling itself is never shed, only the re-decision).
        let cadence = screeps_econ_decision::cadence::rematch_policy(tick_context.system_data.governor.tier.into());
        if !cadence.attempt {
            return Some(HaulState::wait(cadence.backoff_ticks));
        }

        // Live store (projected = None) → byte-identical to the pre-extraction cascade (test:
        // idle_selection_is_byte_identical_pure_extraction).
        select_next_haul_state(state_context, tick_context, None)
    }
}

/// The shared HAUL market/tier selection cascade, extracted from `Idle::tick` so `Delivery::tick`
/// can run the EXACT same selection on a deposit-tick reselect (identical determinism, identical
/// booking). `projected`:
/// - `None` (from `Idle`): sizes against the live `creep.store()` and runs the full fallback
///   chain — byte-identical to the pre-extraction `Idle::tick`.
/// - `Some(..)` (deposit-tick reselect): the market head sizes against the projected `(free,
///   carried)` pair, and the chain STOPS at the market head — a drained lane returns `Wait`
///   directly rather than dropping into a store-reading fallback, since `creep.store()` is stale
///   on the deposit tick. This keeps the no-store-read-on-deposit-tick invariant total.
fn select_next_haul_state(
    state_context: &HaulJobContext,
    tick_context: &mut JobTickContext,
    projected: Option<ProjectedStore>,
) -> Option<HaulState> {
    // Recomputed here (pure, deterministic match — see screeps_econ_decision::cadence) so the
    // drained-lane / fallback tails have the backoff without threading it from the caller.
    let cadence = screeps_econ_decision::cadence::rematch_policy(tick_context.system_data.governor.tier.into());

    let creep = tick_context.runtime_data.owner;
    let pickup_rooms = state_context
        .pickup_rooms
        .iter()
        .filter_map(|e| tick_context.system_data.room_data.get(*e))
        .collect_vec();

    let delivery_rooms = state_context
        .delivery_rooms
        .iter()
        .filter_map(|e| tick_context.system_data.room_data.get(*e))
        .collect_vec();

    let transfer_queue_data = TransferQueueGeneratorData {
        cause: "Haul Idle",
        room_data: tick_context.system_data.room_data,
    };

    let target_filter = if state_context.storage_delivery_only {
        target_filters::storage
    } else {
        target_filters::all
    };

    // ADR 0040 M5a — the LIVE bid-native HAUL selection: rank (pickup, delivery) pairs by RAW
    // bid-density via the SHARED market kernel (reproducing the sim's MARKET tournament arm),
    // for BOTH a loaded hauler (delivers carried cargo) and an empty one (pickup+deliver). The
    // tier-interleave / nearest-wins path below is the fallback the market falls through to
    // (drained lane / non-market lanes) — it keeps the crate tier-capable for the sim's arms.
    // ADR 0044 step 2: the haul leg is priced on TRUE routed distance via a rover-backed oracle
    // (the SAME model the sim mover uses). Built here from the shared cost-matrix cache + the
    // distance memo; the transfer layer sees only the `HaulDistance` trait.
    let mut haul_dist = crate::pathing::hauldistance::RoverDistanceOracle::new(
        tick_context.runtime_data.haul_distance_service,
        tick_context.runtime_data.cost_matrix_cache,
        game::time(),
    );
    let head = get_new_market_pickup_and_delivery_state(
        creep,
        &transfer_queue_data,
        &pickup_rooms,
        &delivery_rooms,
        tick_context.runtime_data.transfer_queue,
        &mut haul_dist,
        target_filter,
        HaulState::pickup,
        HaulState::delivery,
        projected,
    );

    match (head, projected) {
        // Market assigned a target — common path, no store read either way.
        (Some(state), _) => Some(state),
        // Deposit-tick reselect on a drained lane: STOP at the market head — the fallbacks below
        // read the (stale) live store, so entering them would re-open the phantom-cargo bug.
        // Return the same Wait tail the full cascade would reach anyway. No store read.
        (None, Some(_)) => Some(HaulState::wait(cadence.backoff_ticks)),
        // Idle (live store): run the full fallback chain exactly as the pre-extraction cascade.
        (None, None) => None
            .or_else(|| {
                get_new_delivery_current_resources_state(
                    creep,
                    &transfer_queue_data,
                    &delivery_rooms,
                    TransferPriorityFlags::ACTIVE,
                    TransferTypeFlags::HAUL,
                    tick_context.runtime_data.transfer_queue,
                    target_filter,
                    HaulState::delivery,
                )
            })
            .or_else(|| {
                get_new_delivery_current_resources_state(
                    creep,
                    &transfer_queue_data,
                    &delivery_rooms,
                    TransferPriorityFlags::NONE,
                    TransferTypeFlags::HAUL,
                    tick_context.runtime_data.transfer_queue,
                    target_filter,
                    HaulState::delivery,
                )
            })
            .or_else(|| {
                let transfer_queue_data = TransferQueueGeneratorData {
                    cause: "Haul Idle",
                    room_data: tick_context.system_data.room_data,
                };

                get_new_pickup_and_delivery_full_capacity_state(
                    creep,
                    &transfer_queue_data,
                    &pickup_rooms,
                    &delivery_rooms,
                    TransferPriorityFlags::ALL,
                    TransferPriorityFlags::ALL,
                    10,
                    TransferType::Haul,
                    tick_context.runtime_data.transfer_queue,
                    target_filter,
                    HaulState::pickup,
                )
            })
            .or_else(|| {
                for room in &pickup_rooms {
                    if room.get_dynamic_visibility_data().map(|v| !v.visible()).unwrap_or(true) {
                        if let Some(state) = get_new_move_to_room_state(creep, room.name, HaulState::move_to_room) {
                            return Some(state);
                        }
                    }
                }

                None
            })
            .or_else(|| Some(HaulState::wait(cadence.backoff_ticks))),
    }
}

impl Pickup {
    fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        runtime_data.transfer_queue.register_pickup(&self.withdrawl);

        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(delivery_ticket);
        }
    }

    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        // P2.K0: a nearby invader/keeper (remote room) transitions us into Flee.
        if is_threatened(tick_context) {
            return Some(HaulState::flee());
        }
        //
        // NOTE: All haulers run this at the same time so that transfer data is only hydrated on this tick.
        //

        if game::time().is_multiple_of(5) {
            let creep = tick_context.runtime_data.owner;

            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Pickup Tick",
                room_data: tick_context.system_data.room_data,
            };

            let delivery_rooms = state_context
                .delivery_rooms
                .iter()
                .filter_map(|e| tick_context.system_data.room_data.get(*e))
                .collect_vec();

            // get_used_capacity(None) is a memoized single sum in the current
            // engine -- safe on general stores (engine-mechanics folklore row 26).
            let free_capacity = creep.store().get_free_capacity(None).max(0) as u32;

            let mut available_capacity = TransferCapacity::Finite(free_capacity);

            for entries in self.withdrawl.resources().values() {
                for entry in entries {
                    available_capacity.consume(entry.amount());
                }
            }

            let target_filter = if state_context.storage_delivery_only {
                target_filters::storage
            } else {
                target_filters::all
            };

            get_additional_deliveries(
                &transfer_queue_data,
                &delivery_rooms,
                TransferPriorityFlags::ALL,
                TransferType::Haul,
                available_capacity,
                tick_context.runtime_data.transfer_queue,
                &mut self.withdrawl,
                &mut self.deposits,
                target_filter,
                10,
            );
        }

        let deposits = &self.deposits;

        tick_pickup(tick_context, &mut self.withdrawl, move || HaulState::delivery(deposits.clone()))
    }
}

/// Posture room for the repair stress gate + `repair_leak_e` telemetry
/// (ADR 0040 §D6): multi-home haul missions carry several delivery rooms
/// (HashSet-ordered at mission build), so the home the creep is STANDING IN
/// wins — a hauler repairing inside a refill-deficient home is exactly the
/// leak the counter measures — else the first entry as a per-job-stable
/// approximation.
fn repair_posture_room(state_context: &HaulJobContext, tick_context: &JobTickContext) -> Option<Entity> {
    let current = tick_context
        .runtime_data
        .mapping
        .get_room(&tick_context.runtime_data.owner.pos().room_name());

    match current {
        Some(entity) if state_context.delivery_rooms.contains(&entity) => Some(entity),
        _ => state_context.delivery_rooms.first().copied(),
    }
}

impl Delivery {
    fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(delivery_ticket);
        }
    }

    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        // P2.K0: a nearby invader/keeper (remote room) transitions us into Flee.
        if is_threatened(tick_context) {
            return Some(HaulState::flee());
        }
        if state_context.allow_repair {
            let posture_room = repair_posture_room(state_context, tick_context);
            if let Some(consumed_energy) = tick_opportunistic_repair(tick_context, Some(RepairPriority::Low), posture_room) {
                consume_resource_from_deposits(&mut self.deposits, ResourceType::Energy, consumed_energy);
            }
        }

        // Civilian: the delivery leg bids its carried-cargo rate on the numeric lane (decision (4)).
        //
        // Move+deposit concurrency: capture the capacities from the STILL-TRUSTWORTHY store at
        // entry (before tick_delivery issues the transfer), then on a completing deposit reselect
        // the next target same-tick against the PROJECTED (free, carried) pair — never re-reading
        // the now-stale store — so the move fires on the still-free MOVE pipeline this tick instead
        // of losing a tick through Idle. The context is threaded into the closure (not captured) so
        // it can be re-borrowed for select_next_haul_state while tick_delivery holds &mut on it.
        let creep = tick_context.runtime_data.owner;
        let free_before = creep.store().get_free_capacity(None).max(0) as u32;
        let carried_before = creep.store().get_used_capacity(Some(ResourceType::Energy));

        tick_delivery(
            tick_context,
            &mut self.deposits,
            true,
            HaulState::idle,
            |tc, deposited_total| {
                let projected = ProjectedStore::after_deposit(free_before, carried_before, deposited_total);
                select_next_haul_state(state_context, tc, Some(projected))
            },
        )
    }
}

impl MoveToRoom {
    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        // P2.K0: a nearby invader/keeper (traveling through a remote room) transitions us into Flee.
        if is_threatened(tick_context) {
            return Some(HaulState::flee());
        }
        if state_context.allow_repair {
            let posture_room = repair_posture_room(state_context, tick_context);
            tick_opportunistic_repair(tick_context, Some(RepairPriority::Low), posture_room);
        }

        tick_move_to_room(tick_context, self.room_name, None, HaulState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        if is_threatened(tick_context) {
            return Some(HaulState::flee());
        }
        mark_idle(tick_context);
        tick_wait(&mut self.ticks, HaulState::idle)
    }
}

impl Flee {
    fn tick(&mut self, _state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        // Owns the move; competes with no other action. Resume hauling when clear.
        if issue_flee(tick_context) {
            None
        } else {
            Some(HaulState::idle())
        }
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct HaulJob {
    context: HaulJobContext,
    state: HaulState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl HaulJob {
    pub fn new(pickup_rooms: &[Entity], delivery_rooms: &[Entity], allow_repair: bool, storage_delivery_only: bool) -> HaulJob {
        HaulJob {
            context: HaulJobContext {
                pickup_rooms: pickup_rooms.into(),
                delivery_rooms: delivery_rooms.into(),
                allow_repair,
                storage_delivery_only,
            },
            state: HaulState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for HaulJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Haul - {}", self.state.status_description()))
    }

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        self.state.gather_data(system_data, runtime_data);
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let mut tick_context = JobTickContext {
            system_data,
            runtime_data,
            action_flags: SimultaneousActionFlags::UNSET,
        };

        crate::machine_tick::run_state_machine(&mut self.state, "HaulJob", |state| state.tick(&mut self.context, &mut tick_context));
    }
}
