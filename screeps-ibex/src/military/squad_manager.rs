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
use crate::combat::kite::MAX_KITE_OPS;
use crate::combat::{decide_squad_with_pathing, CombatCreepDto, CombatStructureDto, SquadDecision, SquadMemberView, SquadOrderState, SquadView};
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
/// be a spawn source (keeps a squad from being spawned across the map).
const MAX_SPAWN_DISTANCE: u32 = 6;

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
            // A duplicate squad for an already-covered objective is retired (keep one).
            let objective_gone = data.objective_queue.get(obj_id).is_none();
            if objective_gone || covered.contains(&obj_id) {
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
        for (squad_entity, obj_id) in &live_managed {
            let target_room = match data.objective_queue.get(*obj_id) {
                Some(obj) => objective_target(&obj.kind).1,
                None => continue,
            };
            compute_squad_orders(&data.room_data, &data.mapping, &mut data.squad_contexts, &data.creep_owner, *squad_entity, target_room);
        }

        // ── Phase C: claim new objectives up to the global cap. ──
        let mut active = live_managed.len();
        while active < MAX_CONCURRENT_SQUADS {
            // Anchor proximity selection on the closest owned room (any home).
            let anchor = homes.first().map(|h| h.name);
            let obj_id = match data.objective_queue.best_unclaimed_near(anchor, now) {
                Some(id) => id,
                None => break,
            };

            let (composition, target) = match data.objective_queue.get(obj_id) {
                Some(obj) => match obj.force.squads.first() {
                    Some(comp) => (comp.clone(), objective_target(&obj.kind)),
                    None => {
                        // No force requested → nothing to field; claim it anyway so we
                        // don't spin on it this tick, and move on.
                        let placeholder = data.entities.create();
                        data.objective_queue.claim(obj_id, placeholder);
                        let _ = data.entities.delete(placeholder);
                        active += 1;
                        continue;
                    }
                },
                None => break,
            };

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

/// Build the squad view, run the pure `decide_squad`, and apply the result to the
/// `SquadContext` (state + per-member orders). The live adapter for P2.G3 tactics.
fn compute_squad_orders(
    room_data: &ReadStorage<RoomData>,
    mapping: &EntityMappingData,
    squad_contexts: &mut WriteStorage<SquadContext>,
    creep_owner: &ReadStorage<CreepOwner>,
    squad_entity: Entity,
    target_room: RoomName,
) {
    // Read the roster's cached status (immutable). `pos`/`has_ranged` feed the centroid + the kite
    // plan; `has_ranged` resolves the creep body (the adapter's job — the pure crate stays JS-free).
    let (member_views, current_state, retreat_threshold) = match squad_contexts.get(squad_entity) {
        Some(ctx) => (
            ctx.members
                .iter()
                .map(|m| {
                    let has_ranged = creep_owner
                        .get(m.entity)
                        .and_then(|co| co.owner.resolve())
                        .map(|c| c.body().iter().any(|p| p.part() == Part::RangedAttack && p.hits() > 0))
                        .unwrap_or(false);
                    SquadMemberView {
                        hits: m.current_hits,
                        hits_max: m.max_hits,
                        heal_power: m.heal_power,
                        pos: m.position,
                        has_ranged,
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

    let view = SquadView {
        members: &member_views,
        hostiles: &hostiles,
        structures: &structures,
        retreat_threshold,
        current_state,
    };

    // Build the target room's movement cost matrix (terrain walls baked in — the headless
    // `LocalPathfinder` reads walls from the matrix) for the ONE bounded kite search. Same recipe
    // the squad anchor mover uses (formation.rs); the search itself is the pure `LocalPathfinder`.
    let mut cache = CostMatrixCache::default();
    let mut cms = CostMatrixSystem::new(&mut cache, Box::new(screeps_rover::screeps_impl::ScreepsCostMatrixDataSource));
    let opts = CostMatrixOptions::default();
    let mut room_cb = |r: RoomName| {
        let mut matrix = cms.build_local_cost_matrix(r, &opts).ok()?;
        if let Some(terrain) = game::map::get_room_terrain(r) {
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
    };

    let decision = decide_squad_with_pathing(&view, &mut room_cb, MAX_KITE_OPS);

    if let Some(ctx) = squad_contexts.get_mut(squad_entity) {
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
            let attack_target = decision
                .focus
                .map(|f| f.id.map(AttackTarget::Creep).unwrap_or(AttackTarget::Structure(f.pos)));
            for member in ctx.members.iter_mut() {
                member.tick_orders = Some(TickOrders {
                    attack_target,
                    movement: TickMovement::Formation,
                    squad_movement: decision.movement,
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
        // Forming / Moving: no combat orders — the job self-drives to the room.
        _ => {}
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
}
