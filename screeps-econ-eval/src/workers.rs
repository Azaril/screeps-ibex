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
    /// M2 — the transcribed UpgradeJob FSM (jobs/upgrade.rs).
    Upgrader,
    /// M2 — the transcribed BuildJob FSM (jobs/build.rs); `allow_harvest` frozen at
    /// spawn-request time (localbuild.rs:280).
    Builder { allow_harvest: bool },
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
    /// target, repair every tick until it is full or the cargo runs out. Builders reuse it for
    /// their Repair state (jobs/build.rs:168-172) — the runner branches on role at completion
    /// (harvesters CHAIN to the next target, builders fall to Idle).
    FullRepair { target: RepairRef },
    /// The harvest.rs:219 `wait(5)` idle backoff.
    Wait { until: u32 },
    /// M2 — the upgrader stationed within range 3 of the controller (controllerbehavior.rs
    /// `tick_upgrade` with `refill_when_draining = true`): emit UpgradeController every tick;
    /// on the draining tick run the pickup selection NOW — an adjacent source withdraws in
    /// PARALLEL (Pipeline D + E, same tick), a distant one starts the trip.
    Upgrade,
    /// M2 — the builder stationed within range 3 of the site at `tile` (buildbehavior.rs
    /// `tick_build`): emit Build every tick until the site completes/dies or cargo runs out.
    /// Sites are identified by TILE (one site per tile; indices compact per tick).
    Build { tile: (u8, u8) },
    /// M2 — the upgrader/builder refill trip (jobs/upgrade.rs Pickup / jobs/build.rs Pickup →
    /// tick_pickup): travel to range 1 of `src`, withdraw/pick up `take` into SELF, then Idle
    /// (the live FinishedPickup re-try collapses into the next Idle pass — 1-tick-lag
    /// convention, uniform with M1's PostDelivery).
    FillFrom { src: SrcKey, take: u32 },
    /// M2 — the upgrader/builder harvest arm (jobs/upgrade.rs:123-129 / jobs/build.rs:103-109 →
    /// tick_harvest): NEAREST source, chosen at Idle time; harvest until full or drained.
    HarvestSrc { source_idx: usize },
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
