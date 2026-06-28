//! `CombatObjectiveQueue` — the global combat-goal layer (ADR 0008 §2, P2.G1).
//!
//! Modeled **directly** on the scout [`VisibilityQueue`](crate::room::visibilitysystem)
//! (the operator's chosen reference): a global, persistent, priority/TTL request
//! queue of *objectives* (per-room/target, not per-creep). Producers
//! (war / defense-scan / claim / attack / the SK coordinator) upsert idempotently;
//! the [`SquadManager`](super) pulls. This is **the decoupling seam** that makes
//! combat work *queue-owned-and-pulled* instead of *mission-owned-and-pushed* —
//! which is precisely why a completed or aborted producer never strands a squad.
//!
//! Two-layer split, exactly the scout discipline:
//! - **Persistent** ([`CombatObjectiveData`], a serialized component on a singleton
//!   entity): durable FACTS only — the objectives, the `UnwinnableTarget` give-up
//!   backoff, and the monotonic id counter.
//! - **Ephemeral** ([`CombatObjectiveQueue`], a runtime resource): a working copy
//!   of the persistent data plus the per-tick **assignment** (`claimed_by`) — which
//!   squad claims each objective. Assignment is **never serialized**: it self-heals
//!   on a VM reset (the runtime map starts empty, the manager re-claims next tick)
//!   and so **cannot dangle** (kills the IBEX-002b aliasing for the goal layer).
//!
//! **Success is observed, not stored.** Like the scout queue (which holds no
//! `SuccessPredicate`), an objective stays alive only while a producer keeps
//! re-asserting it; when the producer stops caring it simply stops re-requesting,
//! the TTL lapses, and the manager retasks/retires the squad. The manager
//! additionally observes world-state to retire early. There is therefore no
//! serialized predicate closure here.
//!
//! The claimant is the squad's ECS [`Entity`] (the `SquadContext` entity), mirroring
//! the scout queue's `claimed_by: Option<Entity>` + `release_dead`. When the
//! `SquadStore`/`SquadId` lands (P2.I1) the claim key becomes a `SquadId`; until
//! then the runtime `Entity` handle is the natural ephemeral key.

use crate::serialize::*;
use screeps_combat_decision::composition::SquadComposition;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::prelude::*;
use specs::saveload::*;
use specs::Component;
use std::collections::HashMap;

pub const OBJECTIVE_PRIORITY_CRITICAL: f32 = 100.0;
pub const OBJECTIVE_PRIORITY_HIGH: f32 = 75.0;
pub const OBJECTIVE_PRIORITY_MEDIUM: f32 = 50.0;
pub const OBJECTIVE_PRIORITY_LOW: f32 = 25.0;
pub const OBJECTIVE_PRIORITY_NONE: f32 = 0.0;

/// Default TTL for objectives (ticks). Must exceed the longest interval between
/// a producer's re-requests (the SK coordinator and the war scans all re-assert
/// well inside this), so a still-wanted objective never lapses between pushes.
const DEFAULT_OBJECTIVE_TTL: u32 = 200;

/// Base backoff (ticks) after the first give-up; doubles per repeat. Matches the
/// scout reachability backoff — one creep lifetime ≈ 1500, so a safe-moded /
/// over-towered room is retried on a longer horizon, not thrashed.
const UNWINNABLE_BACKOFF_BASE: u32 = 2000;
/// Cap on the give-up backoff.
const UNWINNABLE_BACKOFF_MAX: u32 = 20000;

// ─── Identity ────────────────────────────────────────────────────────────────

/// A minted, monotonic objective id (ADR 0001 discipline — never an `Entity`
/// index). Stable across re-requests of the same target and across serialize/
/// reset; the ephemeral claim map is keyed by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectiveId(pub u32);

/// What flavour of farm a `Farm` objective clears + exploits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FarmKind {
    /// Power-bank harvesting (ADR 0013 power economy). **Planned-future, no producer yet**
    /// (combat-overhaul-plan.md §5(g) "Power-bank farming" + ADR 0013: the deferred farming
    /// workstream names `Farm{PowerBank, room}` as the squad's objective home — a boosted
    /// attacker+healer set with the bank tile as an explicit focus). Producer = a future
    /// `PowerBankOperation`/coordinator mission mirroring the SK pipeline.
    PowerBank,
    /// Source-keeper farming — **LIVE** (the SK farm). Produced by `SourceKeeperFarmMission`
    /// (sourcekeeperfarm.rs) → `duo_sk_farmer` via the `SquadManager`.
    SourceKeeper,
    /// Farm a level-0 highway invader-core room for the **denied-reservation income** it
    /// otherwise costs us (a reserver-based passive farm — DISTINCT from razing a core via
    /// `Dismantle`). **Planned-future, no producer yet**: classified in ADR 0030 §"Per-objective
    /// classification" (`Farm{Core}` → `KillImmuneStructure`, `SingleLifetime`, win-in-time ranged
    /// kill) and ADR 0032 §"EV currency" (`Farm{Core} (lvl0 reserver)` = denied-reservation income
    /// recovered; the `value_e`/`ObjectiveValueKind::FarmCore` valuation already exists for it).
    /// Live lvl0 cores in OUR remotes are razed via `Dismantle` (war.rs InvaderCore arm); this kind
    /// is the future passive-income farm the ADRs plan, not the razing path.
    Core,
}

/// The objective's target. Two objectives are "the same request" (and so
/// upsert-merge) iff their `ObjectiveKind` is equal — the natural target key.
/// Every kind carries a room, so proximity selection and the give-up backoff
/// have a room to work from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectiveKind {
    /// Clear all hostile threats from a room (offense / NPC policing).
    Secure { room: RoomName },
    /// Hold an owned room against a present threat.
    Defend { room: RoomName },
    /// Destroy a specific blocking structure.
    Dismantle { room: RoomName, pos: Position },
    /// Harass a hostile room (deny, not hold).
    Harass { room: RoomName },
    /// Clear + suppress a farmable resource room (power bank / source keeper / core).
    Farm { kind: FarmKind, room: RoomName },
    /// Pre-clear + escort a marginal claim target while the claimer commits.
    /// **Planned-future, no producer yet** (defined + manager-handled; claimers are unescorted
    /// today). Planned producer = the claim/build escort in `claim.rs` — combat-overhaul-plan.md
    /// §W3 ("`Escort{room}` pre-clear producer in `claim.rs` for marginal claim targets"; :168
    /// "W3 adds one") + ADR 0017 expansion pre-clear. ADR 0030 classifies it (`SingleLifetime`,
    /// win-in-time pre-clear while the claimer commits).
    Escort { room: RoomName },
    /// Neutralize a derelict controller (`attackController`) so the waiting mining outpost can take the room
    /// over (ADR 0027 v1.1 P2). `controller` is the controller tile (the squad's `AttackController` target);
    /// fielded as a CLAIM `SquadRole::Declaimer` squad by the `DeclaimAttack` always-field doctrine. Emitted
    /// by `SalvageMission` once the corridor is open (`ControllerAccess::ReachableNow`).
    Declaim { room: RoomName, controller: Position },
}

impl ObjectiveKind {
    /// The room this objective acts in (used for proximity selection + backoff).
    pub fn room(&self) -> RoomName {
        match self {
            ObjectiveKind::Secure { room }
            | ObjectiveKind::Defend { room }
            | ObjectiveKind::Dismantle { room, .. }
            | ObjectiveKind::Harass { room }
            | ObjectiveKind::Farm { room, .. }
            | ObjectiveKind::Escort { room }
            | ObjectiveKind::Declaim { room, .. } => *room,
        }
    }
}

/// Producer hint for status/visualization only — **NOT** ownership of the squad
/// (the manager owns squads). Lets the HUD attribute an objective to who asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ObjectiveOwner {
    War,
    Defense,
    Claim,
    Attack,
    SourceKeeper,
    Manual,
    #[default]
    Unknown,
}

/// What force an objective wants fielded. The manager sizes/spawns from this
/// (`= Vec<PlannedSquad>` in ADR 0008 §2; here the composition(s) only — the
/// target is the objective's room/kind and the deploy condition is the manager's
/// to decide). One entry per squad the objective wants concurrently fielded.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ForceRequirement {
    pub squads: Vec<SquadComposition>,
}

impl ForceRequirement {
    /// A single-squad force (the common case — one duo/quad per objective).
    pub fn single(composition: SquadComposition) -> Self {
        Self {
            squads: vec![composition],
        }
    }
}

// ─── Persistent layer: CombatObjectiveData (serialized component) ────────────

/// A single persistent combat objective entry. Durable facts only — the
/// assignment lives in the ephemeral runtime map.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CombatObjective {
    /// Minted monotonic id (stable across re-requests of the same target).
    pub id: ObjectiveId,
    /// The target (room/structure) — also the upsert/dedup key.
    pub kind: ObjectiveKind,
    /// Selection priority; max-merged on re-request.
    pub priority: f32,
    /// Desired squad composition(s) — the manager fields these.
    pub force: ForceRequirement,
    /// Per-kind wall-clock deadline the manager enforces (Forming/Engaged/clear).
    pub deadline: Option<u32>,
    /// TTL: kept alive by re-request, dies if the producer stops asserting.
    pub expires_at: u32,
    /// Producer hint (status/visualization only).
    pub owner: ObjectiveOwner,
}

/// A target a squad repeatedly failed to win, with an exponential retry backoff
/// (ADR 0008 §2 `UnwinnableTarget`). Persisted so a safe-moded / over-towered
/// room is not thrown squads at forever; cleared when it becomes winnable. Keyed
/// by room (the unwinnable unit — a safe-moded room blocks every objective there).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnwinnableTarget {
    pub room: RoomName,
    /// Earliest tick a squad may be dispatched here again.
    pub retry_after: u32,
    /// Consecutive give-ups (drives the backoff).
    pub attempts: u32,
}

/// Persistent combat objective queue. Serialized as a component on a singleton
/// entity. Holds only data meaningful across ticks and safe to serialize;
/// ephemeral assignment (`claimed_by`) lives in the [`CombatObjectiveQueue`]
/// resource instead.
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatObjectiveData {
    pub objectives: Vec<CombatObjective>,
    /// Targets squads gave up on, with retry backoff (survives across cycles + resets).
    pub unwinnable: Vec<UnwinnableTarget>,
    /// Monotonic id counter for minting `ObjectiveId`s.
    pub next_id: u32,
}

// ─── Runtime layer: CombatObjectiveQueue (ephemeral resource) ────────────────

/// Per-tick runtime state for a single objective.
#[derive(Debug, Clone, Default)]
pub struct ObjectiveRuntimeEntry {
    /// The squad (its `SquadContext` entity) currently assigned to this objective,
    /// if any. NEVER serialized — self-heals on reset, cannot dangle.
    pub claimed_by: Option<Entity>,
}

/// Runtime combat objective queue resource. Holds a working copy of the
/// persistent data (synced from/to [`CombatObjectiveData`] by the cleanup and
/// sync systems) plus ephemeral per-tick assignment. Callers interact with this
/// resource only.
#[derive(Default)]
pub struct CombatObjectiveQueue {
    /// Working copy of persistent objectives.
    pub objectives: Vec<CombatObjective>,
    /// Working copy of the persisted give-up backoffs.
    pub unwinnable: Vec<UnwinnableTarget>,
    /// Working copy of the id counter.
    pub next_id: u32,
    /// Per-tick assignment, keyed by objective id.
    pub runtime: HashMap<ObjectiveId, ObjectiveRuntimeEntry>,
}

impl CombatObjectiveQueue {
    /// Upsert an objective. If an objective with the same target (`kind`) already
    /// exists, merge priority upward, extend the TTL, and refresh the force /
    /// deadline / owner to the producer's current ask (a re-asserting producer is
    /// authoritative for the live force). Otherwise mint a new id and insert.
    ///
    /// `now` is `game::time()` at the live call site (explicit so the core stays
    /// kernel-testable without `game::*`). Returns the objective's id.
    pub fn request(&mut self, request: ObjectiveRequest, now: u32) -> ObjectiveId {
        let expires_at = now.saturating_add(request.ttl);

        if let Some(existing) = self.objectives.iter_mut().find(|o| o.kind == request.kind) {
            existing.priority = existing.priority.max(request.priority);
            existing.expires_at = existing.expires_at.max(expires_at);
            existing.force = request.force;
            existing.deadline = request.deadline;
            existing.owner = request.owner;
            let id = existing.id;
            self.runtime.entry(id).or_default();
            id
        } else {
            let id = ObjectiveId(self.next_id);
            self.next_id = self.next_id.wrapping_add(1);
            self.objectives.push(CombatObjective {
                id,
                kind: request.kind,
                priority: request.priority,
                force: request.force,
                deadline: request.deadline,
                expires_at,
                owner: request.owner,
            });
            self.runtime.entry(id).or_default();
            id
        }
    }

    /// Look up an objective by id.
    pub fn get(&self, id: ObjectiveId) -> Option<&CombatObjective> {
        self.objectives.iter().find(|o| o.id == id)
    }

    /// Find an objective's id by target, if present.
    pub fn find_by_kind(&self, kind: &ObjectiveKind) -> Option<ObjectiveId> {
        self.objectives.iter().find(|o| &o.kind == kind).map(|o| o.id)
    }

    /// Explicitly withdraw an objective (the producer/manager observed success or
    /// gave up). Producers that simply stop re-asserting let it TTL-expire instead.
    pub fn withdraw(&mut self, id: ObjectiveId) {
        self.objectives.retain(|o| o.id != id);
        self.runtime.remove(&id);
    }

    /// Claim an objective for a squad entity.
    pub fn claim(&mut self, id: ObjectiveId, squad_entity: Entity) {
        self.runtime.entry(id).or_default().claimed_by = Some(squad_entity);
    }

    /// Which squad (if any) holds the given objective.
    pub fn claimed_by(&self, id: ObjectiveId) -> Option<Entity> {
        self.runtime.get(&id).and_then(|r| r.claimed_by)
    }

    /// True if the objective is currently claimed by a (live) squad.
    pub fn is_claimed(&self, id: ObjectiveId) -> bool {
        self.claimed_by(id).is_some()
    }

    /// Release all objectives claimed by the given squad entity.
    pub fn release_entity(&mut self, squad_entity: Entity) {
        for entry in self.runtime.values_mut() {
            if entry.claimed_by == Some(squad_entity) {
                entry.claimed_by = None;
            }
        }
    }

    /// Release claims held by squad entities that are no longer alive (the scout
    /// `release_dead` discipline — a vanished squad frees its objective so the
    /// manager re-pulls it).
    pub fn release_dead(&mut self, entities: &Entities) {
        for entry in self.runtime.values_mut() {
            if let Some(e) = entry.claimed_by {
                if !entities.is_alive(e) {
                    entry.claimed_by = None;
                }
            }
        }
    }

    /// Record a give-up for `room`: bump the attempt count and set an exponential
    /// retry backoff (capped). Suppresses claiming any objective in that room
    /// until `retry_after`.
    pub fn mark_unwinnable(&mut self, room: RoomName, now: u32) {
        if let Some(existing) = self.unwinnable.iter_mut().find(|u| u.room == room) {
            existing.attempts = existing.attempts.saturating_add(1);
            let shift = existing.attempts.saturating_sub(1).min(31);
            let backoff = UNWINNABLE_BACKOFF_BASE.saturating_mul(1u32 << shift).min(UNWINNABLE_BACKOFF_MAX);
            existing.retry_after = now.saturating_add(backoff);
        } else {
            self.unwinnable.push(UnwinnableTarget {
                room,
                retry_after: now.saturating_add(UNWINNABLE_BACKOFF_BASE),
                attempts: 1,
            });
        }
    }

    /// Whether `room` is currently in give-up backoff.
    pub fn is_unwinnable_now(&self, room: RoomName, now: u32) -> bool {
        self.unwinnable.iter().any(|u| u.room == room && u.retry_after > now)
    }

    /// Clear any give-up record for `room` — call when the target becomes winnable.
    pub fn clear_unwinnable(&mut self, room: RoomName) {
        self.unwinnable.retain(|u| u.room != room);
    }

    /// Remove objectives that have expired, pruning their runtime entries. An objective is kept past its
    /// TTL while it is CLAIMED (a squad is on it right now — `claimed_by`, a within-session resource) or
    /// while its `deadline` (a manager-stamped commitment lease, serialized so it survives a VM reset) is
    /// still in the future. This is the P-OBJ #23 churn fix: a still-forming/en-route committed squad is
    /// never retired underneath by a producer that fell silent on stale intel. The objective dies only on
    /// explicit `withdraw()` (resolved / given up) or once BOTH the TTL and the commitment lease lapse.
    pub fn expire(&mut self, now: u32) {
        // Compute the claimed set up front: `retain` borrows `objectives` mutably, so the predicate
        // cannot also borrow `self.runtime`.
        let claimed: std::collections::HashSet<ObjectiveId> =
            self.runtime.iter().filter_map(|(id, r)| r.claimed_by.map(|_| *id)).collect();
        self.objectives
            .retain(|o| o.expires_at > now || claimed.contains(&o.id) || o.deadline.is_some_and(|d| d > now));
        let live: std::collections::HashSet<ObjectiveId> = self.objectives.iter().map(|o| o.id).collect();
        self.runtime.retain(|id, _| live.contains(id));
    }

    /// Stamp/refresh an objective's commitment `deadline` (the manager's lease — see [`Self::expire`]).
    /// No-op if the objective is gone. The producer's `request` also writes `deadline`, but the manager
    /// re-stamps each tick a squad is live, so the lease is manager-owned regardless of producer silence.
    pub fn set_deadline(&mut self, id: ObjectiveId, deadline: Option<u32>) {
        if let Some(o) = self.objectives.iter_mut().find(|o| o.id == id) {
            o.deadline = deadline;
        }
    }

    /// Select the best unclaimed objective: highest priority, then (optionally)
    /// nearest to `home`. Skips claimed objectives and any whose room is in
    /// give-up backoff. The manager calls this with a candidate home room.
    pub fn best_unclaimed_near(&self, home: Option<RoomName>, now: u32) -> Option<ObjectiveId> {
        self.best_unclaimed_near_excluding(home, now, &[])
    }

    /// As [`Self::best_unclaimed_near`], but also skips any id in `exclude`. The
    /// manager uses this to pass over objectives it cannot field *this tick*
    /// (no requested force, or no spawn-home in range) without claiming them —
    /// so an unfieldable objective neither spins the selection loop nor leaks a
    /// concurrency slot to a squad that would never spawn.
    pub fn best_unclaimed_near_excluding(&self, home: Option<RoomName>, now: u32, exclude: &[ObjectiveId]) -> Option<ObjectiveId> {
        self.objectives
            .iter()
            .filter(|o| !self.is_claimed(o.id))
            .filter(|o| !exclude.contains(&o.id))
            .filter(|o| !self.is_unwinnable_now(o.kind.room(), now))
            .max_by(|a, b| {
                a.priority
                    .partial_cmp(&b.priority)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| match home {
                        // Prefer the closer objective (smaller distance ranks higher).
                        Some(h) => room_distance(h, b.kind.room()).cmp(&room_distance(h, a.kind.room())),
                        None => std::cmp::Ordering::Equal,
                    })
            })
            .map(|o| o.id)
    }

    /// ADR 0027 v1 (whole-squad REASSIGN): the best unclaimed objective COMPATIBLE with `compatible`, near
    /// `home`, excluding `exclude` (the squad's current objective). The capability gate (`compatible`, a
    /// pure predicate over the objective's `ObjectiveKind`) keeps the kernel's `best_unclaimed_near_excluding`
    /// selection while letting a lower-priority but COMPATIBLE objective win over a higher-priority
    /// incompatible one (so a freed defender reassigns to another defense objective, never onto an offense
    /// core it can't crack). Same priority-then-proximity ordering + backoff/claim skips; deterministic
    /// (a `max_by` over a `Vec`, the predicate is a pure fn — no `HashMap`).
    pub fn best_reassignment_near<F>(&self, home: Option<RoomName>, now: u32, exclude: &[ObjectiveId], compatible: F) -> Option<ObjectiveId>
    where
        F: Fn(&ObjectiveKind) -> bool,
    {
        self.objectives
            .iter()
            .filter(|o| !self.is_claimed(o.id))
            .filter(|o| !exclude.contains(&o.id))
            .filter(|o| !self.is_unwinnable_now(o.kind.room(), now))
            .filter(|o| compatible(&o.kind))
            .max_by(|a, b| {
                a.priority
                    .partial_cmp(&b.priority)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| match home {
                        Some(h) => room_distance(h, b.kind.room()).cmp(&room_distance(h, a.kind.room())),
                        None => std::cmp::Ordering::Equal,
                    })
            })
            .map(|o| o.id)
    }

    /// ADR 0032 v1.1 — the EV-MAXIMIZING per-squad selection that SUPERSEDES the `priority.then(proximity)`
    /// ranking of [`Self::best_reassignment_near`]/[`Self::best_unclaimed_near_excluding`]: rank the
    /// claimable, compatible objectives by the caller-supplied `ev_q` (a QUANTIZED `EV = P(win | caps vs
    /// defense) · value_e − travel`, computed in `squad_manager` from the squad's caps + the objective's
    /// value_e/defense) and return the MAX-EV id. The caller owns the EV (the queue has no intel/defense), so
    /// this stays a thin, pure ranking; ties break on the smallest id (deterministic, no `HashMap`). Skips
    /// claimed / excluded / backoff / capability-incompatible objectives (the column-feasibility filter).
    pub fn best_by_ev<F, C>(&self, now: u32, exclude: &[ObjectiveId], compatible: C, ev_q: F) -> Option<(ObjectiveId, i64)>
    where
        F: Fn(&CombatObjective) -> i64,
        C: Fn(&ObjectiveKind) -> bool,
    {
        self.objectives
            .iter()
            .filter(|o| !self.is_claimed(o.id))
            .filter(|o| !exclude.contains(&o.id))
            .filter(|o| !self.is_unwinnable_now(o.kind.room(), now))
            .filter(|o| compatible(&o.kind))
            .map(|o| (o.id, ev_q(o)))
            // Max EV; deterministic tie-break: the SMALLER id wins (stable `ObjectiveId` order, never an
            // `Entity` index — ADR 0032 §Determinism). The compare is over INTEGER EV (already quantized),
            // so no float feeds the discrete branch (ADR 0020 §6).
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
    }

    /// The quantized EV of ONE objective for this squad (the StayPut column of the gate) — looks the
    /// objective up by id + runs the caller's `ev_q`. `None` if the objective is gone. Used by the EV-positive
    /// gate to compare a candidate reassign/claim against CONTINUING the current objective.
    pub fn objective_ev_q<F>(&self, id: ObjectiveId, ev_q: F) -> Option<i64>
    where
        F: Fn(&CombatObjective) -> i64,
    {
        self.objectives.iter().find(|o| o.id == id).map(ev_q)
    }

    /// Whether there is any unclaimed, non-backoff objective at all.
    pub fn has_unclaimed(&self, now: u32) -> bool {
        self.objectives
            .iter()
            .any(|o| !self.is_claimed(o.id) && !self.is_unwinnable_now(o.kind.room(), now))
    }

    /// Load entries from the persistent component into the working copy.
    fn load_from(&mut self, data: &CombatObjectiveData) {
        self.objectives = data.objectives.clone();
        self.unwinnable = data.unwinnable.clone();
        self.next_id = data.next_id;
        for o in &self.objectives {
            self.runtime.entry(o.id).or_default();
        }
    }

    /// Write the working copy back to the persistent component.
    fn save_to(&self, data: &mut CombatObjectiveData) {
        data.objectives = self.objectives.clone();
        data.unwinnable = self.unwinnable.clone();
        data.next_id = self.next_id;
    }
}

/// Chebyshev distance between two rooms.
fn room_distance(a: RoomName, b: RoomName) -> u32 {
    let delta = a - b;
    delta.0.unsigned_abs().max(delta.1.unsigned_abs())
}

// ─── ObjectiveRequest (input struct for CombatObjectiveQueue::request) ───────

/// Builder for pushing an objective into the [`CombatObjectiveQueue`]. Producers
/// construct one and pass it to [`CombatObjectiveQueue::request`].
pub struct ObjectiveRequest {
    pub kind: ObjectiveKind,
    pub priority: f32,
    pub force: ForceRequirement,
    pub deadline: Option<u32>,
    pub owner: ObjectiveOwner,
    /// TTL override (defaults to [`DEFAULT_OBJECTIVE_TTL`]).
    pub ttl: u32,
}

impl ObjectiveRequest {
    pub fn new(kind: ObjectiveKind, priority: f32, force: ForceRequirement) -> Self {
        // Tripwire (IBEX-046 discipline): the selection comparator coalesces NaN
        // to Equal; assert finiteness where the priority is produced instead.
        debug_assert!(priority.is_finite(), "combat objective priority not finite: {priority}");

        Self {
            kind,
            priority,
            force,
            deadline: None,
            owner: ObjectiveOwner::Unknown,
            ttl: DEFAULT_OBJECTIVE_TTL,
        }
    }

    pub fn owner(mut self, owner: ObjectiveOwner) -> Self {
        self.owner = owner;
        self
    }

    pub fn deadline(mut self, deadline: Option<u32>) -> Self {
        self.deadline = deadline;
        self
    }

    pub fn ttl(mut self, ttl: u32) -> Self {
        self.ttl = ttl;
        self
    }
}

// ─── CombatObjectiveCleanupSystem ─────────────────────────────────────────────

/// Runs at tick start (Main-pass: Cleanup, before operations). Loads persistent
/// data into the resource, expires stale objectives, and releases claims held by
/// dead squads. Creates the singleton component entity on first run. Mirrors
/// [`VisibilityQueueCleanupSystem`](crate::room::visibilitysystem).
pub struct CombatObjectiveCleanupSystem;

#[derive(SystemData)]
pub struct CombatObjectiveCleanupSystemData<'a> {
    combat_objective_queue: Write<'a, CombatObjectiveQueue>,
    combat_objective_data: WriteStorage<'a, CombatObjectiveData>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CombatObjectiveCleanupSystem {
    type SystemData = CombatObjectiveCleanupSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        // Find or create the singleton CombatObjectiveData entity.
        let singleton = (&data.entities, &mut data.combat_objective_data).join().next().map(|(e, _)| e);
        if singleton.is_none() {
            data.updater
                .create_entity(&data.entities)
                .marked::<SerializeMarker>()
                .with(CombatObjectiveData::default())
                .build();
            // No data to load yet; the resource starts empty.
            return;
        }

        let singleton_entity = singleton.unwrap();
        let data_component = data.combat_objective_data.get(singleton_entity).unwrap();

        // Load persistent data into the resource working copy.
        data.combat_objective_queue.load_from(data_component);

        let now = game::time();

        // Expire stale objectives.
        data.combat_objective_queue.expire(now);

        // Release claims for dead squads.
        data.combat_objective_queue.release_dead(&data.entities);
    }
}

// ─── CombatObjectiveSyncSystem ────────────────────────────────────────────────

/// Writes the resource working copy back to the persistent component. Runs late
/// (Main-pass: Persistence), after all producers have pushed requests and before
/// serialization. Mirrors [`VisibilityQueueSyncSystem`](crate::room::visibilitysystem).
pub struct CombatObjectiveSyncSystem;

#[derive(SystemData)]
pub struct CombatObjectiveSyncSystemData<'a> {
    combat_objective_queue: Read<'a, CombatObjectiveQueue>,
    combat_objective_data: WriteStorage<'a, CombatObjectiveData>,
    entities: Entities<'a>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for CombatObjectiveSyncSystem {
    type SystemData = CombatObjectiveSyncSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let count = data.combat_objective_queue.objectives.len();
        if let Some((_, data_component)) = (&data.entities, &mut data.combat_objective_data).join().next() {
            data.combat_objective_queue.save_to(data_component);
        } else if count > 0 {
            log::warn!(
                "CombatObjectiveSync: {} objectives in resource but no singleton entity to write to",
                count
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(name: &str) -> RoomName {
        name.parse().expect("valid room name")
    }

    fn farm_request(room_name: &str, priority: f32) -> ObjectiveRequest {
        ObjectiveRequest::new(
            ObjectiveKind::Farm {
                kind: FarmKind::SourceKeeper,
                room: room(room_name),
            },
            priority,
            ForceRequirement::default(),
        )
    }

    // ── Upsert / idempotency ────────────────────────────────────────────────

    #[test]
    fn request_mints_ids_and_dedups_by_target() {
        let mut q = CombatObjectiveQueue::default();

        let id_a = q.request(farm_request("W5N5", 10.0), 1000);
        let id_b = q.request(farm_request("W6N6", 10.0), 1000);
        // Same target as id_a → upsert, not a new entry.
        let id_a2 = q.request(farm_request("W5N5", 10.0), 1000);

        assert_eq!(id_a, id_a2, "same target re-request returns the same id");
        assert_ne!(id_a, id_b, "distinct targets mint distinct ids");
        assert_eq!(q.objectives.len(), 2, "re-request did not duplicate");
    }

    #[test]
    fn request_max_merges_priority_and_extends_ttl() {
        let mut q = CombatObjectiveQueue::default();

        let id = q.request(farm_request("W5N5", 10.0), 1000);
        // Lower priority must NOT lower the stored priority; TTL extends with now.
        q.request(farm_request("W5N5", 4.0), 1100);
        let o = q.get(id).unwrap();
        assert_eq!(o.priority, 10.0, "priority is max-merged, never lowered");
        assert_eq!(o.expires_at, 1100 + DEFAULT_OBJECTIVE_TTL, "TTL extended to the latest push");

        // Higher priority raises it.
        q.request(farm_request("W5N5", 20.0), 1200);
        assert_eq!(q.get(id).unwrap().priority, 20.0);
    }

    // ── TTL expiry ────────────────────────────────────────────────────────────

    #[test]
    fn expire_drops_stale_objectives_and_prunes_runtime() {
        let mut q = CombatObjectiveQueue::default();
        let id = q.request(farm_request("W5N5", 10.0), 1000);
        assert!(q.runtime.contains_key(&id));

        // Not yet expired.
        q.expire(1000 + DEFAULT_OBJECTIVE_TTL - 1);
        assert!(q.get(id).is_some());

        // Expired exactly at expires_at (retain keeps `expires_at > now`).
        q.expire(1000 + DEFAULT_OBJECTIVE_TTL);
        assert!(q.get(id).is_none(), "stale objective dropped");
        assert!(!q.runtime.contains_key(&id), "runtime entry pruned with the objective");
    }

    /// P-OBJ #23: a CLAIMED objective is immune to TTL lapse (a committed squad is on it) — it dies only
    /// once the claim is released AND it is past its TTL. This is the within-session arm of the fix.
    #[test]
    fn expire_keeps_claimed_objective_past_ttl() {
        use specs::WorldExt;
        let mut world = World::new();
        let squad = world.create_entity().build();
        let mut q = CombatObjectiveQueue::default();
        let id = q.request(farm_request("W5N5", 10.0), 1000);
        q.claim(id, squad);
        q.expire(1000 + DEFAULT_OBJECTIVE_TTL + 50);
        assert!(q.get(id).is_some(), "a claimed objective is immune to TTL lapse");
        q.release_entity(squad);
        q.expire(1000 + DEFAULT_OBJECTIVE_TTL + 60);
        assert!(q.get(id).is_none(), "released + past-TTL objective is dropped");
    }

    /// P-OBJ #23: the manager's serialized commitment `deadline` keeps an objective alive past its TTL
    /// even with no live claim (the cross-reset / cross-system-ordering arm) — until the deadline lapses.
    #[test]
    fn expire_keeps_deadline_committed_objective_past_ttl() {
        let mut q = CombatObjectiveQueue::default();
        let id = q.request(farm_request("W5N5", 10.0).deadline(Some(1000 + DEFAULT_OBJECTIVE_TTL + 300)), 1000);
        q.expire(1000 + DEFAULT_OBJECTIVE_TTL + 100);
        assert!(q.get(id).is_some(), "deadline lease keeps it alive past TTL");
        q.expire(1000 + DEFAULT_OBJECTIVE_TTL + 400);
        assert!(q.get(id).is_none(), "lapsed deadline + TTL → dropped");
    }

    // ── Claim / release single-owner ──────────────────────────────────────────

    #[test]
    fn best_unclaimed_skips_claimed_and_prefers_priority() {
        let mut q = CombatObjectiveQueue::default();
        let low = q.request(farm_request("W5N5", 5.0), 1000);
        let high = q.request(farm_request("W6N6", 50.0), 1000);

        // Highest priority first.
        assert_eq!(q.best_unclaimed_near(None, 1000), Some(high));

        // Claiming the high one excludes it; the low one is then selected.
        use specs::WorldExt;
        let mut world = World::new();
        let squad = world.create_entity().build();
        q.claim(high, squad);
        assert!(q.is_claimed(high));
        assert_eq!(q.best_unclaimed_near(None, 1000), Some(low));

        // Releasing frees it again.
        q.release_entity(squad);
        assert!(!q.is_claimed(high));
        assert_eq!(q.best_unclaimed_near(None, 1000), Some(high));
    }

    #[test]
    fn best_unclaimed_near_prefers_closer_on_priority_tie() {
        let mut q = CombatObjectiveQueue::default();
        // Equal priority; near is one room away, far is many.
        let near = q.request(farm_request("W1N1", 10.0), 1000);
        let _far = q.request(farm_request("W9N9", 10.0), 1000);
        assert_eq!(q.best_unclaimed_near(Some(room("W0N0")), 1000), Some(near));
    }

    // ── Unwinnable backoff ────────────────────────────────────────────────────

    #[test]
    fn mark_unwinnable_sets_exponential_backoff() {
        let r = room("W5N5");
        let mut q = CombatObjectiveQueue::default();

        q.mark_unwinnable(r, 1000);
        assert!(q.is_unwinnable_now(r, 1000));
        assert!(q.is_unwinnable_now(r, 1000 + UNWINNABLE_BACKOFF_BASE - 1));
        assert!(!q.is_unwinnable_now(r, 1000 + UNWINNABLE_BACKOFF_BASE));

        // Second give-up doubles it.
        q.mark_unwinnable(r, 5000);
        assert!(q.is_unwinnable_now(r, 5000 + UNWINNABLE_BACKOFF_BASE));
        assert!(q.is_unwinnable_now(r, 5000 + 2 * UNWINNABLE_BACKOFF_BASE - 1));
        assert!(!q.is_unwinnable_now(r, 5000 + 2 * UNWINNABLE_BACKOFF_BASE));
    }

    #[test]
    fn unwinnable_backoff_is_capped_and_clearable() {
        let r = room("W5N5");
        let mut q = CombatObjectiveQueue::default();
        for _ in 0..20 {
            q.mark_unwinnable(r, 0);
        }
        assert!(q.is_unwinnable_now(r, UNWINNABLE_BACKOFF_MAX - 1));
        assert!(!q.is_unwinnable_now(r, UNWINNABLE_BACKOFF_MAX));

        q.clear_unwinnable(r);
        assert!(!q.is_unwinnable_now(r, 0));
    }

    #[test]
    fn best_unclaimed_skips_unwinnable_rooms() {
        let mut q = CombatObjectiveQueue::default();
        let id = q.request(farm_request("W5N5", 50.0), 1000);
        assert_eq!(q.best_unclaimed_near(None, 1000), Some(id));

        q.mark_unwinnable(room("W5N5"), 1000);
        assert_eq!(q.best_unclaimed_near(None, 1000), None, "backoff room is not selectable");
        assert!(!q.has_unclaimed(1000));
    }

    // ── Persistence round-trip ────────────────────────────────────────────────

    #[test]
    fn load_save_round_trips_persistent_state() {
        let mut q = CombatObjectiveQueue::default();
        let id = q.request(farm_request("W5N5", 10.0), 1000);
        q.mark_unwinnable(room("W7N7"), 1000);

        let mut data = CombatObjectiveData::default();
        q.save_to(&mut data);
        assert_eq!(data.objectives.len(), 1);
        assert_eq!(data.unwinnable.len(), 1);
        assert_eq!(data.next_id, 1);

        let mut q2 = CombatObjectiveQueue::default();
        q2.load_from(&data);
        assert_eq!(q2.get(id).unwrap().priority, 10.0);
        assert!(q2.is_unwinnable_now(room("W7N7"), 1000));
        // Claims are ephemeral: they do NOT survive the persistent round-trip.
        assert!(!q2.is_claimed(id));
        // The id counter survives so future mints never collide with loaded ids.
        assert_eq!(q2.next_id, 1);
    }

    /// ADR 0027 v1: the capability-aware reassignment selector excludes the current id, skips claimed +
    /// backoff rooms, and applies the capability predicate — so a freed defender reassigns only to a
    /// COMPATIBLE objective, even when a higher-priority incompatible one exists.
    #[test]
    fn best_reassignment_excludes_current_and_honors_capability() {
        use specs::WorldExt;
        let mut world = World::new();
        let other = world.create_entity().build();
        let mut q = CombatObjectiveQueue::default();
        // The squad's current Secure (defense); a sibling Secure (defense); a higher-prio Harass (offense).
        let cur = q.request(ObjectiveRequest::new(ObjectiveKind::Secure { room: room("W1N1") }, 50.0, ForceRequirement::default()), 1000);
        let sibling = q.request(ObjectiveRequest::new(ObjectiveKind::Secure { room: room("W2N2") }, 40.0, ForceRequirement::default()), 1000);
        let offense = q.request(ObjectiveRequest::new(ObjectiveKind::Harass { room: room("W3N3") }, 90.0, ForceRequirement::default()), 1000);

        let is_defense = |k: &ObjectiveKind| matches!(k, ObjectiveKind::Defend { .. } | ObjectiveKind::Secure { .. });
        // Excludes the current; the higher-prio Harass is incompatible → the defense sibling wins.
        assert_eq!(q.best_reassignment_near(None, 1000, &[cur], is_defense), Some(sibling));
        // A claimed sibling is skipped (back to the offense path picks nothing compatible → None).
        q.claim(sibling, other);
        assert_eq!(q.best_reassignment_near(None, 1000, &[cur], is_defense), None);
        let _ = offense;
    }

    #[test]
    fn withdraw_removes_objective_and_runtime() {
        let mut q = CombatObjectiveQueue::default();
        let id = q.request(farm_request("W5N5", 10.0), 1000);
        assert!(q.get(id).is_some());
        q.withdraw(id);
        assert!(q.get(id).is_none());
        assert!(!q.runtime.contains_key(&id));
    }
}
