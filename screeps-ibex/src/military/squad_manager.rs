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

use super::composition::{SquadComposition, SquadSlot};
use super::objective_queue::{CombatObjectiveQueue, ObjectiveId, ObjectiveKind, OBJECTIVE_PRIORITY_HIGH};
use super::squad::{AttackTarget, SquadContext, SquadState, SquadTarget, TickMovement, TickOrders};
use crate::combat::kite::{PositionLayers, SquadTacticParams, MAX_KITE_OPS};
use crate::combat::{
    build_room_layers, decide_squad_with_pathing, CombatCreepDto, CombatStructureDto, SquadDecision, SquadMemberView,
    SquadMovement, SquadOrderState, SquadView,
};
use std::collections::HashMap;
use crate::creep::{spawning, CreepOwner};
use crate::entitymappingsystem::EntityMappingData;
use crate::jobs::squad_combat::{creep_to_dto, structure_to_dto};
use crate::room::data::RoomData;
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

/// Max room distance from a candidate home to the objective room for that home to
/// be a spawn source (keeps a squad from being spawned across the map). Matches
/// the legacy `MAX_DEFENSE_SOURCE_DISTANCE` (10) so the defense migration does not
/// narrow the set of rooms a defender can be sourced from.
const MAX_SPAWN_DISTANCE: u32 = 10;

/// Chebyshev distance between two rooms.
fn room_distance(a: RoomName, b: RoomName) -> u32 {
    let delta = a - b;
    delta.0.unsigned_abs().max(delta.1.unsigned_abs())
}

/// Map an objective's selection priority to a spawn-queue priority, so a
/// high-priority objective (defense) is not starved below economy. (Defense
/// objectives upsert at `OBJECTIVE_PRIORITY_HIGH`; farms at `..._LOW`.)
fn spawn_priority_for(objective_priority: f32) -> f32 {
    if objective_priority >= OBJECTIVE_PRIORITY_HIGH {
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
    role: crate::military::squad::SquadRole,
    slot_index: usize,
    target_room: RoomName,
    squad_entity: Entity,
) -> SpawnQueueCallback {
    let squad_entity_id = squad_entity.id();
    Box::new(move |system_data, name| {
        let name = name.to_string();
        system_data.updater.exec_mut(move |world| {
            let sq_entity = world.entities().entity(squad_entity_id);

            let creep_job = crate::jobs::data::JobData::SquadCombat(crate::jobs::squad_combat::SquadCombatJob::new_with_squad(
                target_room,
                sq_entity,
            ));

            let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();

            if let Some(squad_ctx) = world.write_storage::<SquadContext>().get_mut(sq_entity) {
                squad_ctx.add_member(creep_entity, role, slot_index);
            } else {
                log::warn!(
                    "[SquadManager] Spawn callback: SquadContext missing for {:?}, creep {} (slot {}) not registered",
                    sq_entity,
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
            let objective_gone = data.objective_queue.get(obj_id).is_none();
            // Wave-wipe (P2.G4-O4): the squad had members and all are now dead — overwhelmed.
            let wiped = data
                .squad_contexts
                .get(squad_entity)
                .map(|ctx| squad_is_wiped(ctx.total_members_added, ctx.members.len()))
                .unwrap_or(false);

            // Retire a duplicate, an orphaned (objective gone), or a wiped squad.
            if objective_gone || covered.contains(&obj_id) || wiped {
                // On a wave-wipe of a non-`Defend` objective, back off: mark the room unwinnable so the
                // manager stops feeding squads into an unwinnable siege (the queue's exponential backoff
                // makes `best_unclaimed_near` skip it until `retry_after`; the producer's re-assert is
                // ignored meanwhile, and a fresh squad is fielded once the backoff lapses). Defense is
                // exempt — we never abandon an owned room; a wiped defense squad is simply re-staffed.
                if wiped && !objective_gone {
                    let backoff_room = data
                        .objective_queue
                        .get(obj_id)
                        .and_then(|obj| (!matches!(obj.kind, ObjectiveKind::Defend { .. })).then(|| obj.kind.room()));
                    if let Some(room) = backoff_room {
                        data.objective_queue.mark_unwinnable(room, now);
                    }
                }
                retire_squad(&data.updater, &data.entities, squad_entity);
                data.objective_queue.release_entity(squad_entity);
                continue;
            }
            // Re-establish the (ephemeral) claim — idempotent; self-heals post-reset.
            data.objective_queue.claim(obj_id, squad_entity);
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
                queue_slot_spawn(&mut data.spawn_queue, &homes, slot, slot_index, target_room, *squad_entity, spawn_priority);
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
            let (target_room, formation) = match data.objective_queue.get(*obj_id) {
                Some(obj) => (objective_target(&obj.kind).1, is_formation_objective(&obj.kind)),
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
            );
        }

        // ── Phase C: claim new objectives up to the global cap. ──
        // `skipped` holds objectives we cannot field THIS tick (no requested force,
        // or no spawn-home in range). We pass over them WITHOUT claiming — claiming
        // an unfieldable objective would leak a concurrency slot to a `SquadContext`
        // that never spawns (the pre-removal slot-leak vector for a far operator
        // `defend`-flag room) — and exclude them so the selection loop doesn't spin.
        let mut active = live_managed.len();
        let mut skipped: Vec<ObjectiveId> = Vec::new();
        while active < MAX_CONCURRENT_SQUADS {
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
                skipped.push(obj_id);
                continue;
            }

            field_new_squad(&data.updater, &data.entities, &mut data.objective_queue, obj_id, &composition, target);
            active += 1;
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
fn queue_slot_spawn(
    spawn_queue: &mut SpawnQueue,
    homes: &[HomeRoom],
    slot: &SquadSlot,
    slot_index: usize,
    target_room: RoomName,
    squad_entity: Entity,
    priority: f32,
) {
    let token = spawn_queue.token();
    for home in homes.iter().filter(|h| room_distance(h.name, target_room) <= MAX_SPAWN_DISTANCE) {
        let body_def = slot.body_type.body_definition(home.energy_capacity);
        match spawning::create_body(&body_def) {
            Ok(body) => {
                let request = SpawnRequest::new(
                    format!("Squad-{:?} {}", slot.role, target_room),
                    &body,
                    priority,
                    Some(token),
                    create_spawn_callback(slot.role, slot_index, target_room, squad_entity),
                );
                spawn_queue.request(home.entity, request);
            }
            Err(()) => {
                // This room can't build the body; try the next.
            }
        }
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

/// Build a room's movement cost matrix with terrain walls overlaid (the headless `LocalPathfinder`
/// reads walls from the matrix, so the `Terrain::Wall` overlay is mandatory). Extracted so the
/// per-room `PositionLayers` cache (build-once-per-room) and the kite search share one matrix build.
fn build_target_matrix(
    cms: &mut CostMatrixSystem,
    opts: &CostMatrixOptions,
    room: RoomName,
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
) {
    // Read the roster's cached status (immutable). `pos`/`has_ranged` feed the centroid + the kite
    // plan; `has_ranged` resolves the creep body (the adapter's job — the pure crate stays JS-free).
    let (member_views, current_state, retreat_threshold) = match squad_contexts.get(squad_entity) {
        Some(ctx) => (
            ctx.members
                .iter()
                .map(|m| {
                    // Resolve the body ONCE for has_ranged + the per-tick attack output (the engage
                    // DMG reward's melee/ranged power, ADR 0019 focus_damage richness).
                    let (has_ranged, melee_power, ranged_power) = creep_owner
                        .get(m.entity)
                        .and_then(|co| co.owner.resolve())
                        .map(|c| {
                            let mut atk = 0u32;
                            let mut rng = 0u32;
                            for p in c.body().iter().filter(|p| p.hits() > 0) {
                                match p.part() {
                                    Part::Attack => atk += 1,
                                    Part::RangedAttack => rng += 1,
                                    _ => {}
                                }
                            }
                            (rng > 0, atk * screeps::constants::ATTACK_POWER, rng * screeps::constants::RANGED_ATTACK_POWER)
                        })
                        .unwrap_or((false, 0, 0));
                    SquadMemberView {
                        hits: m.current_hits,
                        hits_max: m.max_hits,
                        heal_power: m.heal_power,
                        pos: m.position,
                        has_ranged,
                        melee_power,
                        ranged_power,
                        damage_taken_last_tick: m.damage_taken_last_tick,
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
        if let Some(matrix) = build_target_matrix(&mut cms, &opts, target_room) {
            let layers = build_room_layers(&hostiles, &structures, target_room, &matrix, MAX_KITE_OPS);
            slot.insert((matrix, layers));
        }
    }

    let decision = match room_layers.get(&target_room) {
        Some((matrix, layers)) => {
            let mut room_cb = |_r: RoomName| Some(matrix.clone());
            decide_squad_with_pathing(&view, Some(layers), SquadTacticParams::default(), &mut room_cb, MAX_KITE_OPS)
        }
        None => {
            let mut room_cb = |_r: RoomName| None;
            decide_squad_with_pathing(&view, None, SquadTacticParams::default(), &mut room_cb, MAX_KITE_OPS)
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

    if let Some(ctx) = squad_contexts.get_mut(squad_entity) {
        if !all_arrived {
            // Traveling (both styles): advance the anchor toward the room centre so the squad
            // arrives cohesively (O1). The job's `MoveToRoom` follows `virtual_pos`.
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
                crate::military::formation::advance_squad_virtual_position(ctx, focus.pos);
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
}
