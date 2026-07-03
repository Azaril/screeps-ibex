//! Worker FSM shells (M1 spec Part C.3b): the harvester (harvest→deliver loop with the K3
//! opportunistic repair on the work lane) and the hauler (K2 pickup→deliver with the transcribed
//! — and locally INERT — drive-by repair gate). **Static container miners are SKIPPED in v1**
//! (the spec's option, taken): wiring the live `source_mining` container-miner arm needs the
//! miner/hauler/link triage the M2+ slices own; the harvester arm runs with its no-container
//! branch semantics instead (baseline.rs K4 docs).
//!
//! Each worker advances one FSM step per tick inside [`crate::runner`]: Travel legs teleport
//! along the analytic trace (booking `ROAD_WEAROUT` per tile entered and firing K3 en-route
//! repairs at real range-3 geometry); stationary states emit engine intents. All decisions go
//! through the pure [`crate::baseline`] kernels.

use crate::baseline::{RepairRef, SinkKey, SrcKey};
use screeps::Position;
use std::rc::Rc;

/// What a worker is (assigned at spawn-request time — K4's per-source harvester requests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Harvester { source_idx: usize },
    Hauler,
}

/// One worker's current activity. Targets are held by STABLE identity (SinkKey/SrcKey/RepairRef
/// use indices only for never-removed structures, tiles for mortal ones); the runner re-resolves
/// them against the current world every tick and replans if the target died.
#[derive(Clone, Debug)]
pub enum Activity {
    /// No task — the runner consults the role's selection chain this tick.
    Idle,
    /// Walking the analytic trace; `idx` = the next trace position to enter. On completion,
    /// `then` begins (same tick).
    Travel { trace: Rc<Vec<Position>>, idx: usize, then: Box<Activity> },
    /// Stationed at range 1 of the assigned source: emit Harvest (or a K3 opportunistic Repair,
    /// which MASKS the harvest — the S1 mechanic) each tick until the store is FULL or the
    /// source is DRAINED (the live tick_harvest Err arm exits to Idle; regen brings it back).
    Harvest,
    /// Stationed adjacent to `sink`: transfer `amount` (adjusted down by any same-tick drive-by
    /// repair — the consume-from-deposits mechanic), then Idle (haulers) or
    /// [`Activity::PostDelivery`] (harvesters — the live FinishedDelivery mirror).
    Deliver { sink: SinkKey, amount: u32 },
    /// The harvester's post-delivery re-try — live `FinishedDelivery` (harvest.rs:283-310): with
    /// leftover cargo, re-try deliveries across ALL tiers (High→Medium→Low→None, nearest within
    /// each, NO repair arm), else fall through to Idle.
    PostDelivery,
    /// Stationed adjacent to `src`: withdraw/pickup `take`, then travel to the paired delivery.
    PickupFor { src: SrcKey, take: u32, sink: SinkKey, sink_pos: Position, give: u32 },
    /// The harvester idle full-repair (harvest.rs:177-193): stationed within range 3 of the
    /// target, repair every tick until it is full or the cargo runs out.
    FullRepair { target: RepairRef },
    /// The harvest.rs:219 `wait(5)` idle backoff.
    Wait { until: u32 },
}

/// One worker: role + FSM state.
#[derive(Clone, Debug)]
pub struct Worker {
    pub role: Role,
    pub activity: Activity,
}

impl Worker {
    pub fn new(role: Role) -> Self {
        Worker { role, activity: Activity::Idle }
    }
}
