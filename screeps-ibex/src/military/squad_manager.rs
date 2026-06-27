//! `SquadManager` — the single combat squad lifecycle owner (ADR 0008 §3, P2.G2).
//!
//! A perpetual ECS system (like `ScoutOperation` / the visibility queue's systems)
//! that is the **one** layer owning squad state for objective-driven combat. Each
//! tick it reconciles the [`CombatObjectiveQueue`](super::objective_queue) against
//! the live squads:
//!
//! 1. **Reconcile** existing manager-owned squads (those whose `SquadContext`
//!    carries an `objective_id`): retire — delete the squad entity — when the
//!    objective has been withdrawn (the producer stopped re-asserting → TTL lapse,
//!    or it was explicitly withdrawn); otherwise re-establish the ephemeral claim
//!    (self-heals the claim map after a VM reset, where claims are not serialized).
//! 2. **Field rosters** — spawn any unfilled composition slot for a live squad,
//!    broadcasting one shared spawn token to the in-range home rooms (the proven
//!    `AttackMission` pattern). Members are `SquadCombatJob`s that **self-drive** to
//!    the target room and engage (status-log (ac)); the manager need not push
//!    per-tick movement (job-owns-movement, ADR 0008 §5 ⚑).
//! 3. **Claim new objectives** up to a global cap, minting a `SquadContext` bound to
//!    the objective.
//!
//! **Scope (P2.G2-minimal — "enough to field a `Farm{sk}` squad"):** *replacement*,
//! not pre-spawn (a dead member's slot unfills and is re-spawned; no `request_renew`
//! — the ADR's "never renew" already holds). Pre-spawn-before-death, per-tick
//! tactical orders (G3), retask-on-complete, and SquadId/`SquadStore` keying (P2.I1
//! — the squad is keyed by its `SquadContext` `Entity` until then) are follow-ons.
//! Retirement deletes the squad entity; orphaned members fall to the existing
//! `SquadCombatJob` fallback (no dangling `SquadContext` — no leak) until the general
//! `Recall` terminal state (P2.M0) lands.

use super::objective_queue::{CombatObjectiveQueue, ObjectiveId, ObjectiveKind, OBJECTIVE_PRIORITY_MEDIUM};
use screeps_combat_decision::composition::{SquadComposition, SquadSlot};
use screeps_combat_decision::lifecycle; // P-OBJ #23 / ADR 0027 — the pure reconcile kernel (shared, tested offline)
use super::squad::{AttackTarget, SquadContext, SquadState, SquadTarget, TickMovement, TickOrders};
use crate::combat::kite::{PositionLayers, ThreatField, MAX_KITE_OPS};
use crate::combat::{
    build_room_layers, build_room_threat_field, decide_squad_with_pathing, CombatCreepDto, CombatStructureDto,
    SquadDecision, SquadMemberView, SquadMovement, SquadOrderState, SquadView,
};
// ADR 0026 — the objective/information-dependent strategy-selection layer: pick the per-squad weight
// profile by objective class + room information, instead of the one fixed `SquadTacticParams::default()`.
use crate::combat::strategy::{decide_strategy, default_strategies, CombatObjectiveClass, StrategyContext, StrategyInfo};
use std::collections::HashMap;
use crate::creep::{spawning, CreepOwner};
use crate::entitymappingsystem::EntityMappingData;
use crate::jobs::squad_combat::{creep_to_dto, structure_to_dto};
use crate::room::data::RoomData;
use crate::room::visibilitysystem::{VisibilityQueue, VisibilityRequest, VisibilityRequestFlags, VISIBILITY_PRIORITY_HIGH};
use crate::serialize::SerializeMarker;
use crate::spawnsystem::*;
use screeps::*;
use screeps_rover::{CostMatrixCache, CostMatrixOptions, CostMatrixSystem};
use specs::prelude::*;
use specs::saveload::*;

/// Global cap on concurrently-fielded manager squads. Objectives above this
/// compete by priority via `best_unclaimed_near`. (Per-objective-kind caps —
/// e.g. SK `max_concurrent_farms` — are enforced by the producers.)
const MAX_CONCURRENT_SQUADS: usize = 4;

/// Cap on squads still FORMING (incomplete roster) at once. A forming squad's slots spawn at HIGH (above
/// the economy bulk — see `spawn_priority_for`), so letting many form together starves logistics AND
/// splits the scarce high-priority spawn-ticks so none completes (observed: two squads co-stalled at 3/5
/// and 1/2 for thousands of ticks). Serializing finishes one or two rosters before the next is claimed.
/// Complete squads (out fighting) do NOT count toward this, so it never reduces total concurrent offense
/// below `MAX_CONCURRENT_SQUADS` — it only paces how fast new rosters are started.
const MAX_FORMING_SQUADS: usize = 2;

/// While a squad is still FORMING (incomplete roster), renew a present member whose remaining TTL drops
/// below this so a slow/contested form does not bleed out its early members to old age before the roster
/// completes (ADR 0028 — the live no-renew member-death; `request_renew` previously had zero callers). The
/// spawn system's renew pass only uses spawns no pending spawn claimed + is gated on room energy, so this
/// never starves spawning or a poor colony.
const RENEW_WHILE_FORMING_TTL: u32 = 300;

/// Max room distance from a candidate home to the objective room for that home to
/// be a spawn source (keeps a squad from being spawned across the map). Matches
/// the legacy `MAX_DEFENSE_SOURCE_DISTANCE` (10) so the defense migration does not
/// narrow the set of rooms a defender can be sourced from.
const MAX_SPAWN_DISTANCE: u32 = 10;

/// P-OBJ #23 commitment lease (ticks). When the manager fields a squad it stamps the objective's
/// `deadline = now + COMMITMENT_BUDGET` and refreshes it every tick the squad still has a focus (is
/// actively closing on / fighting a target). The objective then survives producer silence on stale intel
/// for this whole window — generous cover for form (~120) + travel (~150) + a clear margin (~130) — so a
/// committed squad is never retired underneath before it can arrive and engage. If the lease lapses with
/// no active focus (stuck en route, or fought-and-withdrew without a clean clear) the manager gives up
/// and backs the room off; a clean clear resolves earlier via `engaged_once && no-focus && in-room`.
const COMMITMENT_BUDGET: u32 = 400;

/// Chebyshev distance between two rooms.
fn room_distance(a: RoomName, b: RoomName) -> u32 {
    let delta = a - b;
    delta.0.unsigned_abs().max(delta.1.unsigned_abs())
}

/// Map an objective's selection priority to a spawn-queue priority so a FORMING combat squad is not
/// starved below economy. The spawnsystem head-of-line break (`spawnsystem.rs`: a request with
/// `body_cost > available_energy` but `<= energy_capacity` → `break`) reserves each idle home's energy for
/// the highest-priority pending request and spawns nothing below it that tick. MEDIUM offense slots
/// therefore sat permanently last behind the colony's constant economy demand and rosters never completed
/// (observed dead-stuck at 3/5 for thousands of ticks despite idle in-range spawns). MEDIUM+ objectives
/// (active offense/defense) map to HIGH so their slots form in the gaps between CRITICAL miners and
/// transient-HIGH haulers without preempting energy income; only LOW farms stay at MEDIUM. The
/// `MAX_FORMING_SQUADS` cap bounds how many spawn at HIGH at once. (Defense objectives upsert at
/// `OBJECTIVE_PRIORITY_HIGH`; invader-core offense at `..._MEDIUM`; farms at `..._LOW`.)
fn spawn_priority_for(objective_priority: f32) -> f32 {
    if objective_priority >= OBJECTIVE_PRIORITY_MEDIUM {
        SPAWN_PRIORITY_HIGH
    } else {
        SPAWN_PRIORITY_MEDIUM
    }
}

/// A squad is *wiped* (overwhelmed — all members lost) when it had spawned members but none remain
/// alive. Gradual losses are refilled by the unfilled-slot spawns (Phase B) and never reach
/// all-empty; only a squad that lost everyone does. Pure so it's host-testable without an ECS world.
fn squad_is_wiped(total_members_added: u32, living_members: usize) -> bool {
    total_members_added > 0 && living_members == 0
}

/// Whether an objective's squad fights as an oriented **formation box** (siege: keep the anchor
/// when engaged, advance to the focus, present armor toward the threat) vs **skirmishes** (kite via
/// `decide_movement`). Today only `Dismantle` (structure siege) is a formation; defense / farm /
/// harass kite. (Offense `Secure`'s style is decided when its producer lands — P2.G4-O6.)
fn is_formation_objective(kind: &ObjectiveKind) -> bool {
    matches!(kind, ObjectiveKind::Dismantle { .. })
}

/// ADR 0026 — classify a squad's objective for the strategy-selection layer. `StructureBreach` = an
/// explicit dismantle objective (`formation`), OR a room whose only remaining hostiles are STRUCTURES
/// (creeps cleared ⇒ switch to breaching the ring); everything else is open-creep combat. Recomputed each
/// tick, so a squad self-corrects as the room state changes (clears the creeps → flips to breach).
fn classify_objective(formation: bool, has_structures: bool, has_live_hostiles: bool) -> CombatObjectiveClass {
    if formation || (has_structures && !has_live_hostiles) {
        CombatObjectiveClass::StructureBreach
    } else {
        CombatObjectiveClass::OpenCombat
    }
}

/// Map an objective to the squad's target + the room its members travel to.
fn objective_target(kind: &ObjectiveKind) -> (SquadTarget, RoomName) {
    match kind {
        ObjectiveKind::Defend { room } => (SquadTarget::DefendRoom { room: *room }, *room),
        ObjectiveKind::Harass { room } => (SquadTarget::HarassRoom { room: *room }, *room),
        ObjectiveKind::Dismantle { room, pos } => (SquadTarget::AttackStructure { position: *pos }, *room),
        // Secure / Farm / Escort all reduce to "go to the room and clear it";
        // the SquadCombatJob self-drives there and engages whatever is hostile.
        ObjectiveKind::Secure { room } | ObjectiveKind::Farm { room, .. } | ObjectiveKind::Escort { room } => {
            (SquadTarget::AttackRoom { room: *room }, *room)
        }
    }
}

/// The spawn-completion callback: mints the creep entity with a squad-bound
/// `SquadCombatJob` and registers it on the `SquadContext`. Mirrors
/// `AttackMission::create_spawn_callback`.
fn create_spawn_callback(
    role: screeps_combat_decision::composition::SquadRole,
    slot_index: usize,
    target_room: RoomName,
    squad_entity: Entity,
) -> SpawnQueueCallback {
    Box::new(move |system_data, name| {
        let name = name.to_string();
        system_data.updater.exec_mut(move |world| {
            // Generation-safe: the squad may have died during the spawn delay and its ECS slot been
            // recycled. `is_alive` on the FULL entity (generation included) rejects a recycled slot,
            // so we never register the fresh creep onto a *different* squad that now occupies the
            // index (the recycled-slot aliasing bug). `squad_entity` is captured whole — not as a
            // bare `.id()` reconstructed via `entity(id)`, which would alias.
            if !world.entities().is_alive(squad_entity) {
                log::warn!(
                    "[SquadManager] Spawn callback: squad {:?} no longer alive; creep {} (slot {}) not registered",
                    squad_entity,
                    name,
                    slot_index
                );
                return;
            }

            let creep_job = crate::jobs::data::JobData::SquadCombat(crate::jobs::squad_combat::SquadCombatJob::new_with_squad(
                target_room,
                squad_entity,
            ));

            let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();

            if let Some(squad_ctx) = world.write_storage::<SquadContext>().get_mut(squad_entity) {
                squad_ctx.add_member(creep_entity, role, slot_index);
            } else {
                log::warn!(
                    "[SquadManager] Spawn callback: SquadContext missing for {:?}, creep {} (slot {}) not registered",
                    squad_entity,
                    name,
                    slot_index
                );
            }
        });
    })
}

pub struct SquadManagerSystem;

#[derive(SystemData)]
pub struct SquadManagerSystemData<'a> {
    entities: Entities<'a>,
    updater: Read<'a, LazyUpdate>,
    objective_queue: Write<'a, CombatObjectiveQueue>,
    squad_contexts: WriteStorage<'a, SquadContext>,
    spawn_queue: Write<'a, SpawnQueue>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    visibility: Write<'a, VisibilityQueue>,
    features: Read<'a, crate::features::Features>,
}

/// A home room that can act as a spawn source for a squad.
struct HomeRoom {
    entity: Entity,
    name: RoomName,
    energy_capacity: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for SquadManagerSystem {
    type SystemData = SquadManagerSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let now = game::time();
        // P-OBJ #23 lifecycle introspection: reuse the war debug flag for low-noise, per-event squad/
        // objective lifecycle logs (field / reach / engage / retire-reason) so a live capture pinpoints
        // WHICH stage a squad fails at, instead of guessing from Docker.
        let debug = data.features.military.debug_log;

        // ── Gather candidate home rooms (owned, has an idle-capable spawn). ──
        let homes: Vec<HomeRoom> = (&data.entities, &data.room_data)
            .join()
            .filter_map(|(entity, rd)| {
                let dvd = rd.get_dynamic_visibility_data()?;
                if !dvd.owner().mine() {
                    return None;
                }
                let structures = rd.get_structures()?;
                if structures.spawns().iter().all(|s| !s.my()) {
                    return None;
                }
                let energy_capacity = game::rooms().get(rd.name).map(|r| r.energy_capacity_available()).unwrap_or(0);
                if energy_capacity == 0 {
                    return None;
                }
                Some(HomeRoom {
                    entity,
                    name: rd.name,
                    energy_capacity,
                })
            })
            .collect();

        // ── Phase A: reconcile existing manager-owned squads. ──
        let managed: Vec<(Entity, ObjectiveId)> = (&data.entities, &data.squad_contexts)
            .join()
            .filter_map(|(e, ctx)| ctx.objective_id.map(|id| (e, id)))
            .collect();

        let mut live_managed: Vec<(Entity, ObjectiveId)> = Vec::new();
        let mut covered: std::collections::HashSet<ObjectiveId> = std::collections::HashSet::new();

        for (squad_entity, obj_id) in managed {
            // Snapshot the objective once (all Copy) so the queue mutations below don't fight a borrow.
            let obj_info =
                data.objective_queue.get(obj_id).map(|o| (o.kind.room(), matches!(o.kind, ObjectiveKind::Defend { .. }), o.deadline));
            let objective_gone = obj_info.is_none();
            let squad_room = obj_info.map(|(r, _, _)| r);
            let is_defend = obj_info.map(|(_, d, _)| d).unwrap_or(false);
            // P-OBJ #23: has the commitment lease lapsed (the squad failed to make progress in time)?
            let deadline_lapsed = obj_info.and_then(|(_, _, dl)| dl).is_some_and(|d| now >= d);

            // Snapshot the squad facts (Copy) in one borrow.
            let (wiped, has_focus, engaged_once, in_target_room, has_members) = data
                .squad_contexts
                .get(squad_entity)
                .map(|ctx| {
                    // Wave-wipe (P2.G4-O4): the squad had members and all are now dead — overwhelmed.
                    let wiped = squad_is_wiped(ctx.total_members_added, ctx.members.len());
                    let in_room = squad_room
                        .map(|room| ctx.members.iter().any(|m| m.position.is_some_and(|p| p.room_name() == room)))
                        .unwrap_or(false);
                    (wiped, ctx.focus_target.is_some(), ctx.engaged_once, in_room, !ctx.members.is_empty())
                })
                .unwrap_or((false, false, false, false, false));

            // P-OBJ #23 / ADR 0027: the pure reconcile kernel decides retire-vs-keep (unit-tested offline
            // in `screeps_combat_decision::lifecycle`). The manager only builds the snapshot and applies the
            // action — single source of truth, shared with the offline lifecycle harness (no drift).
            let snapshot = lifecycle::ReconcileSnapshot {
                objective_gone,
                duplicate: covered.contains(&obj_id),
                is_defend,
                deadline_lapsed,
                wiped,
                has_focus,
                engaged_once,
                in_target_room,
                has_members,
            };
            let action = lifecycle::reconcile(snapshot);
            if let lifecycle::ReconcileAction::Retire { reason, withdraw, mark_unwinnable } = action {
                if debug {
                    log::info!(
                        "[Lifecycle] RETIRE squad={:?} obj={:?} reason={:?} engaged_once={} in_room={} focus={} deadline_lapsed={} members={}",
                        squad_entity, obj_id, reason, engaged_once, in_target_room, has_focus, deadline_lapsed, has_members
                    );
                }
                if withdraw {
                    data.objective_queue.withdraw(obj_id); // clean win — clear the objective so no one re-fields it
                } else if mark_unwinnable {
                    // Defense is exempt (kernel never sets this for is_defend) — we never abandon an owned room.
                    if let Some(room) = squad_room {
                        data.objective_queue.mark_unwinnable(room, now);
                    }
                }
                retire_squad(&data.updater, &data.entities, squad_entity);
                data.objective_queue.release_entity(squad_entity);
                continue;
            }
            // Live (Keep / KeepRefreshLease): re-establish the (ephemeral) claim — idempotent, self-heals
            // post-reset. Refresh the commitment lease ONLY while actively engaging (KeepRefreshLease) so a
            // long fight or a brief visibility gap never lets the objective lapse underneath the squad.
            data.objective_queue.claim(obj_id, squad_entity);
            if action == lifecycle::ReconcileAction::KeepRefreshLease {
                data.objective_queue.set_deadline(obj_id, Some(now + COMMITMENT_BUDGET));
            }
            // Intel coverage: keep eyes on a committed objective's room so its intel never goes stale
            // underneath the producer. OBSERVE-only + HIGH so an in-range RCL8 observer refreshes it free;
            // if no observer covers it, commitment + the deadline lease bridge the gap instead.
            if let Some(room) = squad_room {
                data.visibility
                    .request(VisibilityRequest::new(room, VISIBILITY_PRIORITY_HIGH, VisibilityRequestFlags::OBSERVE));
            }
            covered.insert(obj_id);
            live_managed.push((squad_entity, obj_id));
        }

        // ── Phase B: field rosters (spawn unfilled slots) for live squads. ──
        for (squad_entity, obj_id) in &live_managed {
            // Read the composition off the objective each tick (the producer owns it).
            let (slots, target_room, spawn_priority) = match data.objective_queue.get(*obj_id) {
                Some(obj) => match obj.force.squads.first() {
                    Some(comp) => (comp.slots.clone(), objective_target(&obj.kind).1, spawn_priority_for(obj.priority)),
                    None => continue,
                },
                None => continue,
            };

            for (slot_index, slot) in slots.iter().enumerate() {
                let already_filled = data
                    .squad_contexts
                    .get(*squad_entity)
                    .map(|ctx| ctx.is_slot_filled(slot_index))
                    .unwrap_or(false);
                if already_filled {
                    continue;
                }
                queue_slot_spawn(&mut data.spawn_queue, &homes, slot, slot_index, target_room, *squad_entity, spawn_priority, debug);
            }
        }

        // ── Phase B-renew: keep a FORMING squad's early members alive while it rallies for the full
        // roster (ADR 0028). Without renew, a slow/contested form loses its early members to old age →
        // they drop to unfilled → re-spawn → churn → never all-present. Request a renew for any present
        // member with low TTL; the spawn system renews creeps adjacent to a free spawn (the rally point is
        // a home spawn — see compute_squad_orders) and is gated on room energy, so it never starves
        // spawning or a poor colony.
        for (squad_entity, obj_id) in &live_managed {
            let requested = data
                .objective_queue
                .get(*obj_id)
                .and_then(|o| o.force.squads.first())
                .map(|c| c.slots.len())
                .unwrap_or(0);
            let Some(ctx) = data.squad_contexts.get(*squad_entity) else {
                continue;
            };
            if requested == 0 || ctx.filled_slot_count() >= requested {
                continue; // not forming (full or unknown roster) — the squad departs; no renew needed
            }
            // Collect first (immutable ctx + creep_owner borrow), then issue (mutable spawn_queue).
            let renews: Vec<(Entity, Entity, u32)> = ctx
                .members
                .iter()
                .filter_map(|m| {
                    let pos = m.position?;
                    let home = homes.iter().find(|h| h.name == pos.room_name())?;
                    let ttl = data.creep_owner.get(m.entity).and_then(|co| co.owner.resolve()).and_then(|c| c.ticks_to_live())?;
                    (ttl < RENEW_WHILE_FORMING_TTL).then_some((home.entity, m.entity, ttl))
                })
                .collect();
            for (room, member, ttl) in renews {
                data.spawn_queue.request_renew(room, member, ttl);
                if debug {
                    log::info!("[Lifecycle] RENEW squad={:?} obj={:?} ttl={} (forming — keep the roster alive)", squad_entity, obj_id, ttl);
                }
            }
        }

        // ── Phase B2: compute per-squad tactical orders. ──
        // The *tactics* are the pure `decide_squad` (focus + engage/retreat hysteresis,
        // ADR 0008 §4 / P2.G3) — the SAME code the sim runs. The manager is only the
        // live adapter: it builds the JS-free `SquadView` from `SquadContext` + the room,
        // calls `decide_squad`, and writes the result back as orders/state. No tactics
        // math lives here.
        // ADR 0019 Stage 3b build-once-per-room sharing: the threat field + reachability flood depend
        // only on a room's enemies, not the deciding squad, so they are built ONCE per room (this tick)
        // and reused by every squad fighting there. Per-squad work (the cohesion search) is unaffected.
        let mut room_layers: HashMap<RoomName, (LocalCostMatrix, PositionLayers)> = HashMap::new();
        for (squad_entity, obj_id) in &live_managed {
            let (target_room, formation, requested_slots) = match data.objective_queue.get(*obj_id) {
                Some(obj) => (
                    objective_target(&obj.kind).1,
                    is_formation_objective(&obj.kind),
                    obj.force.squads.first().map(|c| c.slots.len()).unwrap_or(0),
                ),
                None => continue,
            };
            compute_squad_orders(
                &data.room_data,
                &data.mapping,
                &mut data.squad_contexts,
                &data.creep_owner,
                *squad_entity,
                target_room,
                formation,
                &mut room_layers,
                debug,
                requested_slots,
            );
        }

        // ── Phase C: claim new objectives up to the global cap. ──
        // `skipped` holds objectives we cannot field THIS tick (no requested force,
        // or no spawn-home in range). We pass over them WITHOUT claiming — claiming
        // an unfieldable objective would leak a concurrency slot to a `SquadContext`
        // that never spawns (the pre-removal slot-leak vector for a far operator
        // `defend`-flag room) — and exclude them so the selection loop doesn't spin.
        let mut active = live_managed.len();
        // Count squads still FORMING (incomplete roster). We pace new claims so at most
        // `MAX_FORMING_SQUADS` are forming at once — their slots spawn at HIGH and would otherwise split
        // the scarce high-priority spawn-ticks and starve logistics (see MAX_FORMING_SQUADS).
        let mut forming = live_managed
            .iter()
            .filter(|(se, oid)| {
                let requested = data
                    .objective_queue
                    .get(*oid)
                    .and_then(|o| o.force.squads.first())
                    .map(|c| c.slots.len())
                    .unwrap_or(0);
                let filled = data.squad_contexts.get(*se).map(|c| c.filled_slot_count()).unwrap_or(0);
                requested > 0 && filled < requested
            })
            .count();
        let mut skipped: Vec<ObjectiveId> = Vec::new();
        while active < MAX_CONCURRENT_SQUADS && forming < MAX_FORMING_SQUADS {
            // Anchor proximity selection on the closest owned room (any home).
            let anchor = homes.first().map(|h| h.name);
            let obj_id = match data.objective_queue.best_unclaimed_near_excluding(anchor, now, &skipped) {
                Some(id) => id,
                None => break,
            };

            let (composition, target) = match data.objective_queue.get(obj_id) {
                Some(obj) => match obj.force.squads.first() {
                    Some(comp) => (comp.clone(), objective_target(&obj.kind)),
                    None => {
                        // Malformed objective (no force requested) — can't field it.
                        skipped.push(obj_id);
                        continue;
                    }
                },
                None => break,
            };

            // No in-range home can spawn this squad → don't claim it (a claimed-but-
            // never-spawned `SquadContext` would linger forever holding a cap slot).
            // Skip and try the next-best objective.
            if !homes.iter().any(|h| room_distance(h.name, target.1) <= MAX_SPAWN_DISTANCE) {
                if debug {
                    log::info!("[Lifecycle] SKIP obj={:?} room={} reason=no_home_in_range", obj_id, target.1);
                }
                skipped.push(obj_id);
                continue;
            }

            if debug {
                log::info!("[Lifecycle] FIELD obj={:?} room={} members={}", obj_id, target.1, composition.member_count());
            }
            field_new_squad(&data.updater, &data.entities, &mut data.objective_queue, obj_id, &composition, target, now);
            active += 1;
            forming += 1; // the newly-claimed squad starts forming (slot 0 spawns next tick)
        }
    }
}

/// Delete a squad entity (retire). Orphaned members detach via the job fallback.
fn retire_squad(updater: &Read<LazyUpdate>, entities: &Entities, squad_entity: Entity) {
    if entities.is_alive(squad_entity) {
        updater.exec_mut(move |world| {
            if world.entities().is_alive(squad_entity) {
                let _ = world.delete_entity(squad_entity);
            }
        });
    }
}

/// Queue one slot's spawn to every in-range home room, sharing a token so exactly
/// one room fulfills it per tick.
#[allow(clippy::too_many_arguments)]
fn queue_slot_spawn(
    spawn_queue: &mut SpawnQueue,
    homes: &[HomeRoom],
    slot: &SquadSlot,
    slot_index: usize,
    target_room: RoomName,
    squad_entity: Entity,
    priority: f32,
    debug: bool,
) {
    // Size the member's body ONCE to the STRONGEST in-range home (capped by the body's
    // `maximum_repeat`) — the composition's intended size — NOT per-home. Per-home sizing let a cheaper
    // idle home win the shared-token spawn and field an UNDERSIZED creep (e.g. a 3-repeat SK duo too
    // weak to survive the keepers, even though the operation's affordability gate passed on the strong
    // home's capacity). Because the spawn system skips any home whose capacity is below the body cost
    // (`spawnsystem`: `body_cost > energy_capacity` → `continue`) and the shared token then routes the
    // spawn to an affording home, queuing the one intended-size body on every in-range home is correct —
    // no separate room-affordability filter is needed.
    let best_capacity = homes
        .iter()
        .filter(|h| room_distance(h.name, target_room) <= MAX_SPAWN_DISTANCE)
        .map(|h| h.energy_capacity)
        .max();
    let Some(best_capacity) = best_capacity else {
        return;
    };
    // Build via `build_body` so a force-SIZED slot (BodyType::Sized, R3) goes through the dynamic builder
    // and a template slot through create_body. CAP the build energy at PREFERRED_MEMBER_ENERGY: a force-
    // sized spec is already ≤ that ceiling (sized_for capped it), but a TEMPLATE fallback (a defense shape
    // when sized_for defers) would otherwise scale to the strongest in-range home and spawn a ~5000e blob
    // that never banks at HIGH priority while CRITICAL economy drains the home (the live W5N2/W4N7 defense
    // squads that re-queued forever). Capping keeps every spawned member bankable.
    let build_energy = best_capacity.min(screeps_combat_decision::composition::PREFERRED_MEMBER_ENERGY);
    let body = match slot.body_type.build_body(build_energy, screeps_combat_decision::bodies::MoveProfile::Plains) {
        Some(body) => body,
        // Even the strongest in-range home can't build it (template min OR the sized spec) — don't field
        // an undersized one. (A sized slot that doesn't fit was already vetoed upstream by sized_for.)
        None => {
            // This is a silent roster-stall point: the slot is NEVER queued, so the squad rallies forever
            // at present<full. Surface it so an over-sized per-member spec (or no strong-enough in-range
            // home) is diagnosable instead of invisible.
            if debug {
                log::warn!(
                    "[SpawnQueue] slot={} role={:?} target={} CANNOT BUILD: build_body None at best_cap={} (per-member spec exceeds the strongest IN-RANGE home, or >50 parts) — slot never queued, roster stalls here",
                    slot_index,
                    slot.role,
                    target_room,
                    best_capacity,
                );
            }
            return;
        }
    };

    // Observability: dump the ACTUAL body queued for this slot so we can confirm sizing live (e.g. is the
    // whole force piled onto one member, vs split across members). Behind features.military.debug_log.
    if debug {
        let n = |p: Part| body.iter().filter(|b| **b == p).count();
        let cost: u32 = body.iter().map(|p| p.cost()).sum();
        let in_range = homes.iter().filter(|h| room_distance(h.name, target_room) <= MAX_SPAWN_DISTANCE).count();
        log::info!(
            "[SpawnQueue] slot={} role={:?} target={} parts={} (rng={} heal={} atk={} work={} tough={} carry={} move={}) cost={} prio={} homes_in_range={} (best_cap={})",
            slot_index,
            slot.role,
            target_room,
            body.len(),
            n(Part::RangedAttack),
            n(Part::Heal),
            n(Part::Attack),
            n(Part::Work),
            n(Part::Tough),
            n(Part::Carry),
            n(Part::Move),
            cost,
            priority,
            in_range,
            best_capacity,
        );
    }

    let token = spawn_queue.token();
    for home in homes.iter().filter(|h| room_distance(h.name, target_room) <= MAX_SPAWN_DISTANCE) {
        let request = SpawnRequest::new(
            format!("Squad-{:?} {}", slot.role, target_room),
            &body,
            priority,
            Some(token),
            create_spawn_callback(slot.role, slot_index, target_room, squad_entity),
        );
        spawn_queue.request(home.entity, request);
    }
}

/// Mint a `SquadContext` bound to the objective and claim it. Members spawn next
/// tick once the lazily-created component exists (the AttackMission create-then-
/// wait discipline).
fn field_new_squad(
    updater: &Read<LazyUpdate>,
    entities: &Entities,
    queue: &mut CombatObjectiveQueue,
    obj_id: ObjectiveId,
    composition: &SquadComposition,
    target: (SquadTarget, RoomName),
    now: u32,
) {
    let mut ctx = SquadContext::from_composition(composition);
    ctx.objective_id = Some(obj_id);
    ctx.target = Some(target.0);

    let squad_entity = updater
        .create_entity(entities)
        .with(ctx)
        .marked::<SerializeMarker>()
        .build();

    queue.claim(obj_id, squad_entity);
    // P-OBJ #23: open the commitment lease so the objective outlives producer silence on stale intel
    // while this squad forms + travels (the manager refreshes it each tick the squad has a focus).
    queue.set_deadline(obj_id, Some(now + COMMITMENT_BUDGET));
}

/// Map the live squad state to the pure decision's combat-state subset.
fn squad_state_to_order(state: SquadState) -> SquadOrderState {
    match state {
        SquadState::Forming | SquadState::Rallying => SquadOrderState::Forming,
        SquadState::Moving => SquadOrderState::Moving,
        SquadState::Engaged => SquadOrderState::Engaged,
        SquadState::Retreating => SquadOrderState::Retreating,
        SquadState::Complete => SquadOrderState::Moving,
    }
}

/// Map the pure decision's combat state back to the live squad state.
fn order_state_to_squad(state: SquadOrderState) -> SquadState {
    match state {
        SquadOrderState::Forming => SquadState::Forming,
        SquadOrderState::Moving => SquadState::Moving,
        SquadOrderState::Engaged => SquadState::Engaged,
        SquadOrderState::Retreating => SquadState::Retreating,
    }
}

/// Read a room's hostiles + structures into JS-free combat DTOs (the live adapter leaf;
/// the shared `squad_combat` adapters preserve ordering so the decision's tie-breaks match).
fn build_room_combat_dtos(
    room_data: &ReadStorage<RoomData>,
    mapping: &EntityMappingData,
    room: RoomName,
) -> (Vec<CombatCreepDto>, Vec<CombatStructureDto>) {
    let entity = match mapping.get_room(&room) {
        Some(e) => e,
        None => return (Vec::new(), Vec::new()),
    };
    let rd = match room_data.get(entity) {
        Some(rd) => rd,
        None => return (Vec::new(), Vec::new()),
    };
    let hostiles = rd
        .get_creeps()
        .map(|c| c.hostile().iter().map(creep_to_dto).collect())
        .unwrap_or_default();
    let structures = rd
        .get_structures()
        .map(|s| s.all().iter().map(structure_to_dto).collect())
        .unwrap_or_default();
    (hostiles, structures)
}

/// ADR 0024 Stage 1 (live mirror of `screeps_combat_agent::pathing`): scales the [`ThreatField`]'s
/// raw incoming hits/tick into a small ADDITIVE per-tile traversal penalty — `add = (raw / DIV) cap
/// CAP` — kept tiny + HARD-CAPPED so a threatened tile is *preferred against* but always cheaply
/// PASSABLE (never impassable): a fully-threatened approach must stay traversable or the squad can
/// never close. Seed values; the EXP-*/`SquadTacticParams` sweep is the sanctioned tuner.
const THREAT_PATH_DIV: i32 = 150;
const THREAT_PATH_CAP: i32 = 8;

/// Build a room's movement cost matrix with terrain walls overlaid (the headless `LocalPathfinder`
/// reads walls from the matrix, so the `Terrain::Wall` overlay is mandatory). Extracted so the
/// per-room `PositionLayers` cache (build-once-per-room) and the kite search share one matrix build.
///
/// When `threat` is `Some`, the field is folded into the traversal cost (ADR 0024 Stage 1, "the
/// safest route") so live paths route AROUND tower/enemy kill-zones — the penalty is added ON TOP of
/// the live matrix (preserving road discounts / structure costs), skips impassable tiles, and clamps
/// below `u8::MAX` so no tile is ever sealed. Inert (byte-identical) when there are no threats.
fn build_target_matrix(
    cms: &mut CostMatrixSystem,
    opts: &CostMatrixOptions,
    room: RoomName,
    threat: Option<&ThreatField>,
) -> Option<LocalCostMatrix> {
    let mut matrix = cms.build_local_cost_matrix(room, opts).ok()?;
    if let Some(terrain) = game::map::get_room_terrain(room) {
        for x in 0..50u8 {
            for y in 0..50u8 {
                if terrain.get(x, y) == Terrain::Wall {
                    if let Ok(xy) = RoomXY::checked_new(x, y) {
                        matrix.set(xy, u8::MAX);
                    }
                }
            }
        }
    }
    if let Some(tf) = threat {
        for x in 0..50u8 {
            for y in 0..50u8 {
                let xy = match RoomXY::checked_new(x, y) {
                    Ok(xy) => xy,
                    Err(_) => continue,
                };
                let cur = matrix.get(xy);
                if cur == u8::MAX {
                    continue; // wall / impassable structure — never weaken it
                }
                let raw = tf.raw_at(Position::new(xy.x, xy.y, room));
                if raw <= 0 {
                    continue;
                }
                let add = (raw / THREAT_PATH_DIV).min(THREAT_PATH_CAP);
                if add > 0 {
                    matrix.set(xy, (cur as i32 + add).min(254) as u8);
                }
            }
        }
    }
    Some(matrix)
}

/// Build the squad view, run the pure `decide_squad`, and apply the result to the `SquadContext`
/// (state + per-member orders). The live adapter for P2.G3 tactics. (Many args: distinct ECS borrows
/// that can't be cheaply bundled — the live adapter shim, like the haul builders.)
#[allow(clippy::too_many_arguments)]
fn compute_squad_orders(
    room_data: &ReadStorage<RoomData>,
    mapping: &EntityMappingData,
    squad_contexts: &mut WriteStorage<SquadContext>,
    creep_owner: &ReadStorage<CreepOwner>,
    squad_entity: Entity,
    target_room: RoomName,
    formation: bool,
    room_layers: &mut HashMap<RoomName, (LocalCostMatrix, PositionLayers)>,
    debug: bool,
    requested_slots: usize,
) {
    // Read the roster's cached status (immutable). `pos`/`has_ranged` feed the centroid + the kite
    // plan; `has_ranged` resolves the creep body (the adapter's job — the pure crate stays JS-free).
    let (member_views, current_state, retreat_threshold) = match squad_contexts.get(squad_entity) {
        Some(ctx) => (
            ctx.members
                .iter()
                .map(|m| {
                    // Resolve the body ONCE for has_ranged + per-tick outputs (the engage DMG reward's
                    // melee/ranged power, ADR 0019; + ADR 0025 dismantle/claim caps) and the creep id (so
                    // the EV kernel's heal intent can target this ally).
                    let (id, has_ranged, melee_power, ranged_power, dismantle_power, claim_power) = creep_owner
                        .get(m.entity)
                        .and_then(|co| co.owner.resolve())
                        .map(|c| {
                            let (mut atk, mut rng, mut work, mut claim) = (0u32, 0u32, 0u32, 0u32);
                            for p in c.body().iter().filter(|p| p.hits() > 0) {
                                match p.part() {
                                    Part::Attack => atk += 1,
                                    Part::RangedAttack => rng += 1,
                                    Part::Work => work += 1,
                                    Part::Claim => claim += 1,
                                    _ => {}
                                }
                            }
                            (
                                c.try_raw_id(),
                                rng > 0,
                                atk * screeps::constants::ATTACK_POWER,
                                rng * screeps::constants::RANGED_ATTACK_POWER,
                                work * screeps::constants::DISMANTLE_POWER,
                                claim * 300, // CONTROLLER_ATTACK_PER_PART (engine const; declaim is deferred in v1)
                            )
                        })
                        .unwrap_or((None, false, 0, 0, 0, 0));
                    SquadMemberView {
                        hits: m.current_hits,
                        hits_max: m.max_hits,
                        heal_power: m.heal_power,
                        pos: m.position,
                        has_ranged,
                        melee_power,
                        ranged_power,
                        damage_taken_last_tick: m.damage_taken_last_tick,
                        id,
                        dismantle_power,
                        claim_power,
                    }
                })
                .collect::<Vec<_>>(),
            squad_state_to_order(ctx.state),
            ctx.retreat_threshold,
        ),
        None => return,
    };
    if member_views.is_empty() {
        return;
    }

    let (hostiles, structures) = build_room_combat_dtos(room_data, mapping, target_room);

    // Enemy safe mode → all our combat in the room is nullified (engage-veto, ADR 0020 §8). Only known
    // when the room is visible; default false otherwise (we discover + retreat on arrival).
    let enemy_safe_mode = game::rooms()
        .get(target_room)
        .and_then(|r| r.controller())
        .map(|c| !c.my() && c.safe_mode().unwrap_or(0) > 0)
        .unwrap_or(false);

    let view = SquadView {
        members: &member_views,
        hostiles: &hostiles,
        structures: &structures,
        retreat_threshold,
        current_state,
        enemy_safe_mode,
        // Offense closes in and finishes the enemy (the close-to-kill gradient is now live). `Hold` is
        // for a future pin/harass objective. enemy_stalled stays false until SquadContext tracks the
        // no-progress counter (a fast-follow; the sim already validates the stalemate-disengage path).
        engage_objective: screeps_combat_decision::EngageObjective::Destroy,
        enemy_stalled: false,
    };

    // Build the target room's movement cost matrix (terrain walls baked in — the headless
    // `LocalPathfinder` reads walls from the matrix) plus the per-room `PositionLayers` (threat
    // field + reachability flood) ONCE per room and share across every squad targeting it — the
    // threat field and floods depend only on the room's enemies, not on which squad is asking
    // (ADR 0019 Stage 3b build-once-per-room). Same matrix recipe the squad anchor mover uses
    // (formation.rs); the search itself is the pure `LocalPathfinder`.
    if let std::collections::hash_map::Entry::Vacant(slot) = room_layers.entry(target_room) {
        let mut cache = CostMatrixCache::default();
        let mut cms = CostMatrixSystem::new(&mut cache, Box::new(screeps_rover::screeps_impl::ScreepsCostMatrixDataSource));
        let opts = CostMatrixOptions::default();
        // ADR 0024 Stage 1: the same field `build_room_layers` prices, folded into the movement matrix
        // so the kite/strategic path routes around exposure (the layers' own threat field is rebuilt
        // internally — identical inputs).
        let threat = build_room_threat_field(&hostiles, &structures);
        if let Some(matrix) = build_target_matrix(&mut cms, &opts, target_room, Some(&threat)) {
            let layers = build_room_layers(&hostiles, &structures, target_room, &matrix, MAX_KITE_OPS);
            slot.insert((matrix, layers));
        }
    }

    // ADR 0026 — pick the weight profile by objective class + room information (instead of one fixed
    // default). StructureBreach = an explicit dismantle objective OR a room whose only remaining hostiles
    // are structures (creeps cleared → switch to breaching the ring); everything else is open-creep
    // combat. v1 keys on `enemy_safe_mode` (the in-scope safe-mode veto); `assault_mode` is the
    // force-sizing follow-on (None ⇒ a towered base defaults to a straight breach).
    let class = classify_objective(formation, !structures.is_empty(), !hostiles.is_empty());
    let strat_ctx = StrategyContext { class, info: StrategyInfo { enemy_safe_mode, assault_mode: None } };
    let tactics = decide_strategy(&strat_ctx, &default_strategies());

    let decision = match room_layers.get(&target_room) {
        Some((matrix, layers)) => {
            let mut room_cb = |_r: RoomName| Some(matrix.clone());
            decide_squad_with_pathing(&view, Some(layers), tactics, &mut room_cb, MAX_KITE_OPS)
        }
        None => {
            let mut room_cb = |_r: RoomName| None;
            decide_squad_with_pathing(&view, None, tactics, &mut room_cb, MAX_KITE_OPS)
        }
    };

    // Travel cohesion (P2.G4-O1): while the squad is still converging on the target room, the manager
    // advances the squad's footprint anchor toward the room centre — the rover `AnchorPath` via
    // `advance_squad_virtual_position` (cached, footprint-aware, holds-on-blocked). The job's
    // `MoveToRoom` reads `virtual_pos` and issues each member's `move_to` (§5 separation: the manager
    // decides the squad frame, the job owns movement issuance). Once every member has ARRIVED we drop
    // the anchor so the `Engaged` state kites via the pure `decide_movement` rather than
    // formation-follow — keeping G3 kiting intact; engaged formation/orientation is the separate O2.
    // This stops a squad from trickling into a contested room one creep at a time.
    let all_arrived = member_views
        .iter()
        .all(|m| m.pos.map(|p| p.room_name() == target_room).unwrap_or(false));

    // P-OBJ #23 killer diagnostic: the squad is fully in the target room but `decide_squad` found NOTHING
    // to attack. This one line classifies the live no-engage failure: hostiles=0 structs=0 => empty room
    // DTOs (visibility/mapping timing); structs>=1 focus=None => structure-focus selection bug;
    // safe_mode=true => correct veto. Repeats while stalled, which itself confirms a persistent stall.
    if debug && all_arrived && decision.focus.is_none() {
        log::info!(
            "[Lifecycle] IN_ROOM_NO_FOCUS squad={:?} room={} hostiles={} structs={} state={:?} safe_mode={} formation={}",
            squad_entity, target_room, hostiles.len(), structures.len(), current_state, enemy_safe_mode, formation
        );
    }

    // P-OBJ #23 RALLY-until-full gate (operator: wait + group up until the squad is ready, THEN go in
    // together). The full roster must be spawned AND present in the world before the squad leaves home —
    // otherwise the lone slot-0 lead departs alone, can't solo the objective, dies, and the squad wipes →
    // re-field → slot-0 forever (the actual invader no-engage root cause). Measured against the objective's
    // requested slot count so a death-degraded layout can't shrink "full".
    let member_positions: Vec<Option<Position>> = member_views.iter().map(|m| m.pos).collect();
    let ready_to_depart = crate::military::formation::squad_ready_to_depart(&member_positions, requested_slots);

    if let Some(ctx) = squad_contexts.get_mut(squad_entity) {
        if !ready_to_depart {
            // RALLY: hold at home and group up. Point the travel anchor at the lead's home-room centre (NOT
            // the target room) so the members formation-follow it HOME and gather — no lone lead crosses
            // toward the objective until the full roster is present. apply_squad_decision below issues the
            // Formation move orders that make the members track this (home) anchor.
            if let Some(lead) = member_views.iter().find_map(|m| m.pos) {
                // Rally AT a home spawn (not the room centre) so the gathering members stay adjacent to a
                // spawn and can be RENEWED while waiting for the full roster (ADR 0028 — the spawn renew
                // pass needs the creep next to the spawn). Fall back to the room centre if no spawn resolves.
                let rally = screeps::game::rooms()
                    .get(lead.room_name())
                    .and_then(|r| r.find(screeps::constants::find::MY_SPAWNS, None).first().map(|s| s.pos()))
                    .unwrap_or_else(|| Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), lead.room_name()));
                crate::military::formation::advance_squad_virtual_position(ctx, rally);
            }
            if debug {
                log::info!(
                    "[Lifecycle] RALLY squad={:?} room={} present={}/{} (holding home until full)",
                    squad_entity, target_room, member_positions.iter().filter(|p| p.is_some()).count(), requested_slots
                );
            }
        } else if !all_arrived {
            // Ready (full roster present): advance the anchor toward the target room centre so the squad
            // crosses cohesively (O1). The job's `MoveToRoom` follows `virtual_pos`.
            if let Ok(centre) = RoomCoordinate::new(25) {
                let dest = Position::new(centre, centre, target_room);
                crate::military::formation::advance_squad_virtual_position(ctx, dest);
            }
        } else if formation {
            // Arrived + FORMATION (siege, O2): keep the anchor and advance it toward the focus
            // (close to dismantle/weapon range) while ORIENTING the block toward the threat —
            // `reassign_slots` puts tanks/high-HP in the threat-facing slots, healers at the back
            // (`decide_squad.orientation` → `threat_direction`). The job's `squad_has_anchor`
            // branch then formation-follows. (Pure decision in the crate; manager applies; job moves.)
            if let Some(focus) = decision.focus {
                // A STRUCTURE focus (`focus.id` is None) sits on an IMPASSABLE tile: advancing the anchor
                // onto it pathfinds to range 0, finds no path, and reports `Blocked`, so the squad parks
                // SHORT of weapon range and never fires (the invader-core "enters but does nothing" bug,
                // ADR 0026 §9). Stand off one tile toward the squad so the formation holds in weapon range;
                // a creep focus keeps targeting the creep's tile (where the kite logic wants the anchor).
                let dest = match (focus.id, decision.center) {
                    (None, Some(center)) => crate::military::formation::standoff_one_tile(focus.pos, center),
                    _ => focus.pos,
                };
                crate::military::formation::advance_squad_virtual_position(ctx, dest);
            }
            ctx.threat_direction = decision.orientation;
            ctx.reassign_slots();
        } else {
            // Arrived + SKIRMISH: drop the anchor so `Engaged` kites via `decide_movement` (O1).
            ctx.squad_path = None;
        }
        apply_squad_decision(ctx, &decision, creep_owner);
    }
}

/// Write a `SquadDecision` into the `SquadContext`: the combat state, the shared focus, and per-member
/// orders. The per-member `movement` stays `Formation` — for a manager squad (no anchor) the job
/// routes it through the pure `decide_movement` (§5 ⚑ job-owns-movement), reading the squad's shared
/// directive (`squad_movement`/`squad_center`/`squad_cohesion_radius`) the manager stamps here so the
/// block kites/advances as one. Heal *assignment* still reuses `SquadContext::compute_heal_assignments`
/// until that migrates into `decide_squad` (Step 7).
fn apply_squad_decision(ctx: &mut SquadContext, decision: &SquadDecision, creep_owner: &ReadStorage<CreepOwner>) {
    ctx.state = order_state_to_squad(decision.state);
    if ctx.state == SquadState::Engaged {
        ctx.engaged_once = true; // P-OBJ #23: latch reaching combat (drives resolve vs give-up in Phase A)
    }
    ctx.focus_target = decision.focus.map(|f| f.pos);

    match decision.state {
        SquadOrderState::Retreating => {
            ctx.issue_retreat_orders(None, Some(creep_owner));
        }
        SquadOrderState::Engaged => {
            // Per-member focus with damage spill (ADR 0020 §4.2); index aligns with view.members
            // (built from ctx.members in order). `None` ⇒ the shared focus.
            for (i, member) in ctx.members.iter_mut().enumerate() {
                let focus = decision.focus_assignments.get(i).copied().flatten().or(decision.focus);
                let attack_target = focus.map(|f| f.id.map(AttackTarget::Creep).unwrap_or(AttackTarget::Structure(f.pos)));
                // ADR 0019 §8: a member with its own goal (a pure-support healer's heal-coverage tile)
                // moves to that tile instead of the shared block directive; everyone else follows the
                // block. Only the anchorless `decide_movement` path reads `squad_movement`, so this is
                // inert for a siege formation (which keeps its healers-back slots).
                let squad_movement = decision
                    .member_goals
                    .get(i)
                    .copied()
                    .flatten()
                    .map(|goal| SquadMovement::Advance { goal, range: 0 })
                    .unwrap_or(decision.movement);
                member.tick_orders = Some(TickOrders {
                    attack_target,
                    movement: TickMovement::Formation,
                    squad_movement,
                    squad_center: decision.center,
                    squad_cohesion_radius: decision.cohesion_radius,
                    ..Default::default()
                });
            }
            // Apply the pure heal assignments (Step 7): resolve member indices → the target's creep
            // ObjectId, then set each assigned healer's heal_target. (Indices match `member_views`,
            // built in the same order as `ctx.members`.) Resolve first to avoid an aliasing borrow.
            let heal_targets: Vec<(usize, Option<ObjectId<Creep>>)> = decision
                .heal_assignments
                .iter()
                .map(|a| {
                    let target_id = ctx.members.get(a.target_idx).and_then(|m| creep_owner.get(m.entity)).map(|co| co.owner);
                    (a.healer_idx, target_id)
                })
                .collect();
            for (healer_idx, target_id) in heal_targets {
                if let Some(orders) = ctx.members.get_mut(healer_idx).and_then(|m| m.tick_orders.as_mut()) {
                    orders.heal_target = target_id;
                }
            }
        }
        // Forming / Moving (traveling, no engagement yet). When the manager has set a travel
        // anchor (O1), emit a bare `Formation` directive so the job's `MoveToRoom` follows the
        // anchor (cohesive travel) instead of self-driving per-creep. Without an anchor (no layout
        // / no path) this is a no-op and the job falls back to plain room navigation.
        _ => {
            if ctx.squad_path.is_some() {
                for member in ctx.members.iter_mut() {
                    member.tick_orders = Some(TickOrders {
                        movement: TickMovement::Formation,
                        ..Default::default()
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::military::objective_queue::FarmKind;

    fn room(name: &str) -> RoomName {
        name.parse().expect("valid room name")
    }

    #[test]
    fn objective_target_maps_kind_to_squad_target_and_travel_room() {
        let r = room("W5N5");

        // Farm/Secure/Escort all reduce to "go clear the room".
        let (t, travel) = objective_target(&ObjectiveKind::Farm {
            kind: FarmKind::SourceKeeper,
            room: r,
        });
        assert!(matches!(t, SquadTarget::AttackRoom { room } if room == r));
        assert_eq!(travel, r);

        let (t, _) = objective_target(&ObjectiveKind::Defend { room: r });
        assert!(matches!(t, SquadTarget::DefendRoom { room } if room == r));

        let (t, _) = objective_target(&ObjectiveKind::Harass { room: r });
        assert!(matches!(t, SquadTarget::HarassRoom { room } if room == r));

        // Dismantle travels to the structure's room, targets the position.
        let pos = Position::new(RoomCoordinate::new(10).unwrap(), RoomCoordinate::new(10).unwrap(), r);
        let (t, travel) = objective_target(&ObjectiveKind::Dismantle { room: r, pos });
        assert!(matches!(t, SquadTarget::AttackStructure { position } if position == pos));
        assert_eq!(travel, r);
    }

    #[test]
    fn room_distance_is_chebyshev() {
        assert_eq!(room_distance(room("W0N0"), room("W0N0")), 0);
        assert_eq!(room_distance(room("W1N1"), room("W4N1")), 3); // dx dominates
        assert_eq!(room_distance(room("W1N1"), room("W4N5")), 4); // dy dominates
    }

    #[test]
    fn forming_combat_squads_spawn_above_economy_bulk() {
        use crate::military::objective_queue::{OBJECTIVE_PRIORITY_CRITICAL, OBJECTIVE_PRIORITY_HIGH, OBJECTIVE_PRIORITY_LOW};
        // Active offense (a MEDIUM objective, e.g. an invader core) MUST map to HIGH spawn priority, or the
        // spawnsystem head-of-line break strands its forming slots permanently below the economy bulk and
        // the roster never completes (the dead-stall root). Defense (HIGH) and any CRITICAL stay HIGH.
        assert_eq!(spawn_priority_for(OBJECTIVE_PRIORITY_CRITICAL), SPAWN_PRIORITY_HIGH);
        assert_eq!(spawn_priority_for(OBJECTIVE_PRIORITY_HIGH), SPAWN_PRIORITY_HIGH);
        assert_eq!(
            spawn_priority_for(OBJECTIVE_PRIORITY_MEDIUM),
            SPAWN_PRIORITY_HIGH,
            "MEDIUM offense must form at HIGH, not be starved at MEDIUM"
        );
        // Low-priority farms stay below combat so they never preempt economy.
        assert_eq!(spawn_priority_for(OBJECTIVE_PRIORITY_LOW), SPAWN_PRIORITY_MEDIUM);
    }

    #[test]
    fn squad_is_wiped_only_after_spawning_then_losing_everyone() {
        assert!(!squad_is_wiped(0, 0), "fresh squad, nothing spawned yet → not wiped");
        assert!(!squad_is_wiped(4, 2), "still has living members → not wiped");
        assert!(squad_is_wiped(4, 0), "spawned members and all are gone → wiped");
    }

    #[test]
    fn only_dismantle_fights_as_a_formation() {
        let r = room("W5N5");
        let pos = Position::new(RoomCoordinate::new(10).unwrap(), RoomCoordinate::new(10).unwrap(), r);
        assert!(is_formation_objective(&ObjectiveKind::Dismantle { room: r, pos }));
        assert!(!is_formation_objective(&ObjectiveKind::Defend { room: r }));
        assert!(!is_formation_objective(&ObjectiveKind::Farm { kind: FarmKind::SourceKeeper, room: r }));
        assert!(!is_formation_objective(&ObjectiveKind::Harass { room: r }));
        assert!(!is_formation_objective(&ObjectiveKind::Secure { room: r }));
    }

    #[test]
    fn classify_objective_routes_breach_vs_open() {
        use CombatObjectiveClass::*;
        // Explicit dismantle objective → breach, regardless of room contents.
        assert_eq!(classify_objective(true, false, false), StructureBreach);
        assert_eq!(classify_objective(true, false, true), StructureBreach);
        // Non-formation: structures present + NO live hostiles → breach (creeps cleared, raze the ring).
        assert_eq!(classify_objective(false, true, false), StructureBreach);
        // Non-formation with live hostiles → open creep combat (kill the creeps first).
        assert_eq!(classify_objective(false, true, true), OpenCombat);
        assert_eq!(classify_objective(false, false, true), OpenCombat);
        // Empty room (no structures, no hostiles) → open (nothing to breach).
        assert_eq!(classify_objective(false, false, false), OpenCombat);
    }
}
