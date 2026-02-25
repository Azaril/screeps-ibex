use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::squad_combat::*;
use crate::military::composition::*;
use crate::military::formation::advance_squad_virtual_position;
use crate::military::squad::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::visualization::SummaryContent;
use log::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// TTL below which a squad member should seek renewal (mission-side threshold).
const RENEW_TTL_THRESHOLD: u32 = 1200;
/// Minimum stored energy (per room) to allow renewal.
const RENEW_MIN_ROOM_ENERGY: u32 = 10_000;

/// When to deploy a squad within a force plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DeployCondition {
    /// Deploy immediately during spawning phase.
    Immediate,
    /// Deploy after the Nth squad reaches a given state.
    AfterSquad { index: usize, state: SquadState },
    /// Deploy after a delay (ticks from mission start).
    AfterDelay { ticks: u32 },
    /// Deploy when a target structure's HP drops below a percentage.
    /// Used for power bank haulers: spawn when bank is at ~20% HP.
    AfterTargetHPPercent { percent: f32 },
}

/// A single squad request in the force plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedSquad {
    /// What composition this squad should have.
    pub composition: SquadComposition,
    /// What the squad should do.
    pub target: SquadTarget,
    /// When to start spawning this squad.
    pub deploy_condition: DeployCondition,
}

/// Tracks a live squad managed by this mission.
///
/// Member tracking is delegated to the `SquadContext` ECS component on
/// `squad_entities[i]`. This struct only holds lifecycle and timing state.
#[derive(Clone, Debug, ConvertSaveload)]
struct ManagedSquad {
    /// Index into the original force plan (for deploy condition references).
    plan_index: usize,
    /// Squad lifecycle state.
    state: SquadState,
    /// Target room for this squad.
    target_room: RoomName,
    /// Number of slots expected (from composition).
    expected_count: usize,
    /// Whether spawning has been initiated for all slots.
    spawn_complete: bool,
}

#[derive(Clone, ConvertSaveload)]
pub struct AttackMissionContext {
    /// Target room for the overall mission.
    target_room: RoomName,
    /// Home rooms assigned for spawning.
    home_room_datas: EntityVec<Entity>,
    /// The force plan received from the parent operation.
    force_plan: Vec<PlannedSquad>,
    /// Live squads being managed.
    squads: EntityVec<ManagedSquad>,
    /// ECS entities holding SquadContext components (one per squad, parallel to `squads`).
    squad_entities: EntityVec<Entity>,
    /// Current wave number.
    current_wave: u32,
    /// Maximum waves before giving up.
    max_waves: u32,
    /// Tick when the mission started.
    start_tick: Option<u32>,
    /// Total energy invested across all spawns.
    energy_invested: u32,
    /// Whether the attack achieved its objective (reached Exploiting phase).
    /// Read by the parent AttackOperation to decide whether to exploit.
    mission_succeeded: bool,
    /// Tick when the exploit phase started.
    exploit_start_tick: Option<u32>,
    /// Whether hauler squads have been spawned for the exploit phase.
    exploit_haulers_spawned: bool,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum AttackMissionState {
        Planning {
            phantom: std::marker::PhantomData<Entity>
        },
        Spawning {
            phantom: std::marker::PhantomData<Entity>
        },
        Rallying {
            phantom: std::marker::PhantomData<Entity>
        },
        Engaging {
            phantom: std::marker::PhantomData<Entity>
        },
        Exploiting {
            phantom: std::marker::PhantomData<Entity>
        },
        Retreating {
            phantom: std::marker::PhantomData<Entity>
        },
        MissionComplete {
            phantom: std::marker::PhantomData<Entity>
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &AttackMissionContext) -> String {
            format!("AttackMission - {}", self.status_description())
        }

        _ => fn status_description(&self) -> String;

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &AttackMissionContext) {}

        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut AttackMissionContext) -> Result<Option<AttackMissionState>, String>;
    }
);

// ─── State implementations ──────────────────────────────────────────────────

impl Planning {
    fn status_description(&self) -> String {
        "Planning".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut AttackMissionContext,
    ) -> Result<Option<AttackMissionState>, String> {
        if state_context.start_tick.is_none() {
            state_context.start_tick = Some(game::time());
        }

        // Initialize managed squads from the force plan.
        if state_context.squads.is_empty() && !state_context.force_plan.is_empty() {
            for (i, planned) in state_context.force_plan.iter().enumerate() {
                let target_room = match &planned.target {
                    SquadTarget::DefendRoom { room } => *room,
                    SquadTarget::AttackRoom { room } => *room,
                    SquadTarget::HarassRoom { room } => *room,
                    SquadTarget::CollectResources { room } => *room,
                    SquadTarget::MoveToPosition { position } => position.room_name(),
                    SquadTarget::AttackStructure { position } => position.room_name(),
                    SquadTarget::EscortPosition { position } => position.room_name(),
                };

                // Create a SquadContext ECS entity for this squad.
                let squad_ctx = SquadContext::from_composition(&planned.composition);
                let squad_entity = system_data
                    .updater
                    .create_entity(system_data.entities)
                    .with(squad_ctx)
                    .marked::<SerializeMarker>()
                    .build();

                state_context.squads.push(ManagedSquad {
                    plan_index: i,
                    state: SquadState::Forming,
                    target_room,
                    expected_count: planned.composition.member_count(),
                    spawn_complete: false,
                });
                state_context.squad_entities.push(squad_entity);
            }
        }

        if state_context.squads.is_empty() {
            // No force plan -- mission has nothing to do.
            return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
        }

        // Check that SquadContext components are available before transitioning.
        // They are created via LazyUpdate above and won't exist until
        // maintain() runs at end-of-tick. Spawning needs them to track
        // filled slots; transitioning immediately would cause it to queue
        // spawns it can't track, leading to duplicate creeps.
        let contexts_ready = state_context
            .squad_entities
            .iter()
            .all(|&e| system_data.squad_contexts.get(e).is_some());

        if contexts_ready {
            return Ok(Some(AttackMissionState::spawning(std::marker::PhantomData)));
        }

        // SquadContext not yet available -- wait one tick for LazyUpdate.
        Ok(None)
    }
}

/// Nearest home room (with sufficient energy) to the engagement target; used for rally.
/// Uses RoomRouteCache to avoid repeated find_route calls.
fn nearest_home_room_to_target(
    home_room_datas: &[Entity],
    room_data: &specs::WriteStorage<'_, crate::room::data::RoomData>,
    economy: &crate::military::economy::EconomySnapshot,
    route_cache: &mut crate::military::economy::RoomRouteCache,
    target_room: RoomName,
    current_tick: u32,
) -> Option<Entity> {
    let mut best: Option<(Entity, u32)> = None;
    for &room_entity in home_room_datas {
        let rd = room_data.get(room_entity)?;
        let stored = economy.rooms.get(&room_entity).map(|r| r.stored_energy).unwrap_or(0);
        if stored < RENEW_MIN_ROOM_ENERGY {
            continue;
        }
        let cached = route_cache.get_route_distance(rd.name, target_room, current_tick);
        let dist = if cached.reachable { cached.hops } else { u32::MAX };
        if best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((room_entity, dist));
        }
    }
    best.map(|(e, _)| e)
}

fn has_claim_parts(creep: &Creep) -> bool {
    creep.body().iter().any(|p| p.part() == Part::Claim)
}

fn gather_creeps_needing_renew(
    squad_ctx: &SquadContext,
    creep_owner: &specs::ReadStorage<'_, CreepOwner>,
) -> Vec<(Entity, u32)> {
    let mut out = Vec::new();
    for member in squad_ctx.members.iter() {
        let creep = match creep_owner.get(member.entity).and_then(|co| co.owner.resolve()) {
            Some(c) => c,
            None => continue,
        };
        let ttl = match creep.ticks_to_live() {
            Some(t) => t,
            None => continue,
        };
        if ttl >= RENEW_TTL_THRESHOLD || has_claim_parts(&creep) {
            continue;
        }
        out.push((member.entity, ttl));
    }
    out
}

fn room_has_idle_spawn(
    room_entity: Entity,
    room_data: &specs::WriteStorage<'_, crate::room::data::RoomData>,
) -> bool {
    let rd = match room_data.get(room_entity) {
        Some(r) => r,
        None => return false,
    };
    let structures = match rd.get_structures() {
        Some(s) => s,
        None => return false,
    };
    structures.spawns().iter().any(|s| s.my() && s.spawning().is_none())
}

fn rally_position_in_room(
    room_entity_opt: Option<Entity>,
    room_data: &specs::WriteStorage<'_, crate::room::data::RoomData>,
) -> Option<Position> {
    let room_entity = room_entity_opt?;
    let rd = room_data.get(room_entity)?;
    Some(Position::new(
        RoomCoordinate::new(25).unwrap(),
        RoomCoordinate::new(25).unwrap(),
        rd.name,
    ))
}

fn formation_dest_adjacent_to_spawn(
    room_entity: Entity,
    room_data: &specs::WriteStorage<'_, crate::room::data::RoomData>,
) -> Option<Position> {
    let rd = room_data.get(room_entity)?;
    let structures = rd.get_structures()?;
    let spawn = structures.spawns().iter().find(|s| s.my() && s.spawning().is_none())?;
    let pos = spawn.pos();
    let x = pos.x().u8();
    let y = pos.y().u8();
    let room_name = rd.name;
    for (dx, dy) in [(0i32, 1), (1, 0), (0, -1), (-1, 0)] {
        let nx = (x as i32).saturating_add(dx);
        let ny = (y as i32).saturating_add(dy);
        if (1..=48).contains(&nx) && (1..=48).contains(&ny) {
            if let (Some(rx), Some(ry)) = (
                RoomCoordinate::new(nx as u8).ok(),
                RoomCoordinate::new(ny as u8).ok(),
            ) {
                return Some(Position::new(rx, ry, room_name));
            }
        }
    }
    None
}

impl Spawning {
    fn status_description(&self) -> String {
        "Spawning".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut AttackMissionContext,
    ) -> Result<Option<AttackMissionState>, String> {
        // If all squads that ever had members are now dead, skip straight
        // to wave progression rather than waiting forever for spawns that
        // will never fill (the creeps died before spawning completed).
        if all_squads_wiped(system_data, state_context) {
            return handle_wave_wipe(system_data, state_context);
        }

        if state_context.home_room_datas.is_empty() {
            warn!(
                "[AttackMission] Spawning tick for {} but home_room_datas is empty!",
                state_context.target_room
            );
        }

        // Process each squad that should be deploying.
        for squad_idx in 0..state_context.squads.len() {
            let plan_index = state_context.squads[squad_idx].plan_index;
            let spawn_complete = state_context.squads[squad_idx].spawn_complete;
            let expected = state_context.squads[squad_idx].expected_count;

            if !Self::should_deploy(plan_index, state_context) {
                continue;
            }

            if spawn_complete {
                continue;
            }

            // Read filled status from SquadContext.
            let squad_entity = match state_context.squad_entities.get(squad_idx).copied() {
                Some(e) => e,
                None => continue,
            };
            let filled_count = system_data
                .squad_contexts
                .get(squad_entity)
                .map(|ctx| ctx.filled_slot_count())
                .unwrap_or(0);

            if filled_count >= expected {
                state_context.squads[squad_idx].spawn_complete = true;
                state_context.squads[squad_idx].state = SquadState::Rallying;
                continue;
            }

            let planned = &state_context.force_plan[plan_index];

            // Queue all unfilled slots to all capable home rooms.
            // Each slot gets one shared token so only one room spawns it
            // per tick. The spawn callback fires on the same tick via
            // LazyUpdate, so is_slot_filled() is accurate by next tick.
            for (slot_index, slot) in planned.composition.slots.iter().enumerate() {
                let already_filled = system_data
                    .squad_contexts
                    .get(squad_entity)
                    .map(|ctx| ctx.is_slot_filled(slot_index))
                    .unwrap_or(false);

                if already_filled {
                    continue;
                }

                let token = system_data.spawn_queue.token();

                for home_entity in state_context.home_room_datas.iter() {
                    let home_room_data = match system_data.room_data.get(*home_entity) {
                        Some(rd) => rd,
                        None => continue,
                    };

                    let room = match game::rooms().get(home_room_data.name) {
                        Some(r) => r,
                        None => continue,
                    };

                    let energy_capacity = room.energy_capacity_available();
                    let body_def = if matches!(slot.body_type, BodyType::Drain) {
                        let dps = tower_dps_at_target_room(system_data, state_context.target_room).unwrap_or(150.0);
                        crate::military::bodies::drain_body_for_tower_dps(energy_capacity, dps)
                    } else {
                        slot.body_type.body_definition(energy_capacity)
                    };

                    match spawning::create_body(&body_def) {
                        Ok(body) => {
                            let target_room = state_context.target_room;
                            let spawn_request = SpawnRequest::new(
                                format!("Atk-{:?} {}", slot.role, target_room),
                                &body,
                                SPAWN_PRIORITY_MEDIUM,
                                Some(token),
                                Self::create_spawn_callback(slot.role, slot_index, target_room, squad_entity),
                            );
                            system_data.spawn_queue.request(*home_entity, spawn_request);
                        }
                        Err(()) => {
                            // This room can't produce the body; try the next.
                        }
                    }
                }
            }
        }

        let target_room_center = Position::new(
            RoomCoordinate::new(25).unwrap(),
            RoomCoordinate::new(25).unwrap(),
            state_context.target_room,
        );

        let rally_room = nearest_home_room_to_target(
            &state_context.home_room_datas,
            system_data.room_data,
            system_data.economy,
            system_data.route_cache,
            state_context.target_room,
            game::time(),
        );

        for squad_idx in 0..state_context.squads.len() {
            let squad_entity = match state_context.squad_entities.get(squad_idx).copied() {
                Some(e) if system_data.entities.is_alive(e) => e,
                _ => continue,
            };
            let squad_ctx = match system_data.squad_contexts.get_mut(squad_entity) {
                Some(ctx) => ctx,
                None => continue,
            };

            if squad_ctx.filled_slot_count() == 0 {
                continue;
            }

            let creeps_needing_renew = gather_creeps_needing_renew(squad_ctx, system_data.creep_owner);
            let renew_room = if creeps_needing_renew.is_empty() {
                None
            } else {
                rally_room.filter(|&room_entity| {
                    system_data
                        .economy
                        .rooms
                        .get(&room_entity)
                        .map(|r| r.stored_energy >= RENEW_MIN_ROOM_ENERGY)
                        .unwrap_or(false)
                        && room_has_idle_spawn(room_entity, system_data.room_data)
                })
            };

            let formation_dest = if let Some(room_entity) = renew_room {
                formation_dest_adjacent_to_spawn(room_entity, system_data.room_data)
                    .unwrap_or_else(|| rally_position_in_room(rally_room, system_data.room_data).unwrap_or(target_room_center))
            } else {
                rally_position_in_room(rally_room, system_data.room_data).unwrap_or(target_room_center)
            };

            for (entity, ttl) in &creeps_needing_renew {
                if let Some(room_entity) = renew_room.or(rally_room) {
                    system_data.spawn_queue.request_renew(room_entity, *entity, *ttl);
                }
            }

            for member in squad_ctx.members.iter_mut() {
                member.tick_orders = Some(TickOrders {
                    movement: TickMovement::Formation,
                    ..Default::default()
                });
            }

            advance_squad_virtual_position(squad_ctx, formation_dest);
        }

        // Check if all immediate squads are spawned or spawning.
        let all_immediate_ready = state_context
            .squads
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                matches!(
                    state_context.force_plan.get(s.plan_index).map(|p| &p.deploy_condition),
                    Some(DeployCondition::Immediate)
                )
            })
            .all(|(_, s)| s.spawn_complete);

        let any_has_members = state_context.squad_entities.iter().any(|&e| {
            system_data
                .squad_contexts
                .get(e)
                .map(|ctx| ctx.filled_slot_count() > 0)
                .unwrap_or(false)
        });

        if all_immediate_ready && any_has_members {
            return Ok(Some(AttackMissionState::rallying(std::marker::PhantomData)));
        }

        Ok(None)
    }

    fn should_deploy(plan_index: usize, ctx: &AttackMissionContext) -> bool {
        let planned = match ctx.force_plan.get(plan_index) {
            Some(p) => p,
            None => return false,
        };

        match &planned.deploy_condition {
            DeployCondition::Immediate => true,
            DeployCondition::AfterSquad { index, state } => ctx.squads.get(*index).map(|s| s.state >= *state).unwrap_or(false),
            DeployCondition::AfterDelay { ticks } => ctx.start_tick.map(|start| game::time() - start >= *ticks).unwrap_or(false),
            DeployCondition::AfterTargetHPPercent { .. } => {
                // HP-based deployment is checked during Engaging phase.
                false
            }
        }
    }

    fn create_spawn_callback(role: SquadRole, slot_index: usize, target_room: RoomName, squad_entity: Entity) -> SpawnQueueCallback {
        let squad_entity_id = squad_entity.id();
        Box::new(move |system_data, name| {
            let name = name.to_string();
            system_data.updater.exec_mut(move |world| {
                let sq_entity = world.entities().entity(squad_entity_id);

                // All roles use SquadCombatJob -- it detects body parts at runtime.
                let creep_job = JobData::SquadCombat(SquadCombatJob::new_with_squad(target_room, sq_entity));

                let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();

                // Register creep on the SquadContext component with its slot index.
                if let Some(squad_ctx) = world.write_storage::<SquadContext>().get_mut(sq_entity) {
                    squad_ctx.add_member(creep_entity, role, slot_index);
                } else {
                    log::warn!(
                        "[AttackMission] Spawn callback: SquadContext missing for entity {:?}, \
                         creep {} (slot {}) not registered on squad",
                        sq_entity,
                        name,
                        slot_index
                    );
                }
            });
        })
    }
}

impl Rallying {
    fn status_description(&self) -> String {
        "Rallying".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut AttackMissionContext,
    ) -> Result<Option<AttackMissionState>, String> {
        // If all squads that ever had members are now dead, skip straight
        // to wave progression rather than waiting forever for cohesion
        // that will never happen (the creeps died during rally).
        if all_squads_wiped(system_data, state_context) {
            return handle_wave_wipe(system_data, state_context);
        }

        let target_room_center = Position::new(
            RoomCoordinate::new(25).unwrap(),
            RoomCoordinate::new(25).unwrap(),
            state_context.target_room,
        );

        let rally_room = nearest_home_room_to_target(
            &state_context.home_room_datas,
            system_data.room_data,
            system_data.economy,
            system_data.route_cache,
            state_context.target_room,
            game::time(),
        );

        for (squad_idx, squad) in state_context.squads.iter().enumerate() {
            if squad.state != SquadState::Rallying {
                continue;
            }
            let squad_entity = match state_context.squad_entities.get(squad_idx).copied() {
                Some(e) if system_data.entities.is_alive(e) => e,
                _ => continue,
            };
            let squad_ctx = match system_data.squad_contexts.get_mut(squad_entity) {
                Some(ctx) => ctx,
                None => continue,
            };

            let creeps_needing_renew = gather_creeps_needing_renew(squad_ctx, system_data.creep_owner);
            let renew_room = if creeps_needing_renew.is_empty() {
                None
            } else {
                rally_room.filter(|&room_entity| {
                    system_data
                        .economy
                        .rooms
                        .get(&room_entity)
                        .map(|r| r.stored_energy >= RENEW_MIN_ROOM_ENERGY)
                        .unwrap_or(false)
                        && room_has_idle_spawn(room_entity, system_data.room_data)
                })
            };

            // Rally in a safe room (nearest home to target), not in the target room.
            // This ensures squads group up before moving into engagement.
            let formation_dest = if let Some(room_entity) = renew_room {
                formation_dest_adjacent_to_spawn(room_entity, system_data.room_data)
                    .unwrap_or_else(|| rally_position_in_room(rally_room, system_data.room_data).unwrap_or(target_room_center))
            } else {
                rally_position_in_room(rally_room, system_data.room_data).unwrap_or(target_room_center)
            };

            for (entity, ttl) in &creeps_needing_renew {
                if let Some(room_entity) = renew_room.or(rally_room) {
                    system_data.spawn_queue.request_renew(room_entity, *entity, *ttl);
                }
            }

            for member in squad_ctx.members.iter_mut() {
                member.tick_orders = Some(TickOrders {
                    movement: TickMovement::Formation,
                    ..Default::default()
                });
            }

            advance_squad_virtual_position(squad_ctx, formation_dest);
        }

        // Check if all rallying squads are ready (filled and cohesive). Do not transition
        // just because no squads are in Rallying state — require actual cohesion.
        let all_rallied = state_context
            .squads
            .iter()
            .enumerate()
            .filter(|(_, s)| s.state == SquadState::Rallying)
            .all(|(idx, s)| {
                // Check filled count from SquadContext.
                let squad_entity = match state_context.squad_entities.get(idx).copied() {
                    Some(e) => e,
                    None => return true,
                };
                let squad_ctx = match system_data.squad_contexts.get(squad_entity) {
                    Some(ctx) => ctx,
                    None => return true,
                };

                if squad_ctx.filled_slot_count() < s.expected_count {
                    return false;
                }

                // For multi-member squads, check that members are in the same room and tight.
                if s.expected_count <= 1 {
                    return true;
                }

                if !Self::squad_is_cohesive(squad_ctx) {
                    return false;
                }

                // Require squad to be grouped outside the target room (rally point, not already in engagement).
                let in_target = squad_ctx
                    .members
                    .iter()
                    .filter_map(|m| m.position)
                    .next()
                    .map(|p| p.room_name() == state_context.target_room)
                    .unwrap_or(false);
                !in_target
            });

        // Only transition when at least one squad was rallying and all such squads are cohesive.
        // Do not transition when no squads are rallying (e.g. all still Forming) — wait for group-up.
        let any_rallying = state_context.squads.iter().any(|s| s.state == SquadState::Rallying);

        if any_rallying && all_rallied {
            // Transition rallying squads to Moving.
            for squad in state_context.squads.iter_mut() {
                if squad.state == SquadState::Rallying {
                    squad.state = SquadState::Moving;
                }
            }

            info!("[AttackMission] Squads rallied for {}, engaging", state_context.target_room);
            return Ok(Some(AttackMissionState::engaging(std::marker::PhantomData)));
        }

        Ok(None)
    }

    /// Check if all living members of a squad are cohesive enough to engage.
    ///
    /// For multi-member squads, requires members to be:
    /// 1. All in the same room.
    /// 2. Within 1 tile of each other (combat range).
    /// 3. Each member within 1 tile of their formation target (if a
    ///    formation layout and virtual position exist).
    ///
    /// If the squad has been held for `STRICT_HOLD_MAX_TICKS` ticks
    /// (pathfinding blocked), the formation offset check (3) is relaxed
    /// to avoid permanently blocking the squad.
    fn squad_is_cohesive(squad_ctx: &SquadContext) -> bool {
        let positions: Vec<Position> = squad_ctx.members.iter().filter_map(|m| m.position).collect();

        if positions.len() < 2 {
            return !positions.is_empty();
        }

        let anchor = positions[0];

        // Requirement 1 & 2: same room, within 1 tile of each other.
        let spatially_tight = positions
            .iter()
            .all(|p| p.room_name() == anchor.room_name() && anchor.get_range_to(*p) <= 1);

        if !spatially_tight {
            return false;
        }

        // Requirement 3: each member near their formation offset.
        // Relaxed if the squad has been held long enough (pathfinding blocked).
        let pathfinding_blocked = squad_ctx.strict_hold_ticks >= STRICT_HOLD_MAX_TICKS;
        if pathfinding_blocked {
            // Members are spatially tight (≤1 tile) -- good enough.
            return true;
        }

        if let (Some(layout), Some(squad_path)) = (&squad_ctx.layout, &squad_ctx.squad_path) {
            let virtual_pos = squad_path.virtual_pos;
            let formation_tight = squad_ctx.members.iter().all(|m| {
                if let (Some(pos), Some(target)) = (
                    m.position,
                    crate::military::formation::virtual_anchor_target(virtual_pos, layout, m.formation_slot),
                ) {
                    pos.get_range_to(target) <= 1
                } else {
                    // No position or target -- assume ok to avoid blocking.
                    true
                }
            });
            formation_tight
        } else {
            // No formation layout -- spatial check is sufficient.
            true
        }
    }
}

/// Check if every squad that ever had members spawned now has zero living
/// members. Returns false if no squad ever had members (nothing to wipe),
/// or if any squad hasn't finished spawning yet.
fn all_squads_wiped(system_data: &MissionExecutionSystemData, state_context: &AttackMissionContext) -> bool {
    // At least one squad must have "ever had members" (or we treat spawn_complete as proxy
    // when SquadContext is missing, e.g. entity deleted). Otherwise we'd never trigger wipe
    // when all squad entities are gone or creeps were never registered.
    let any_ever_had = state_context.squads.iter().enumerate().any(|(idx, s)| {
        let squad_ctx = state_context
            .squad_entities
            .get(idx)
            .and_then(|&e| system_data.squad_contexts.get(e));
        squad_ctx
            .map(|ctx| ctx.ever_had_members())
            .unwrap_or(s.spawn_complete)
    });

    if !any_ever_had {
        return false;
    }

    state_context.squads.iter().enumerate().all(|(idx, s)| {
        let squad_ctx = state_context
            .squad_entities
            .get(idx)
            .and_then(|&e| system_data.squad_contexts.get(e));
        let living = squad_ctx.map(|ctx| ctx.members.len()).unwrap_or(0);
        let ever_had = squad_ctx.map(|ctx| ctx.ever_had_members()).unwrap_or(false);
        // A squad is wiped if it has no living members AND: spawning is done
        // (spawn_complete), it had members at some point (ever_had), or it's
        // still in Forming (never filled this wave — don't block progression).
        living == 0 && (s.spawn_complete || ever_had || s.state == SquadState::Forming)
    })
}

/// Tower DPS at room edge for the target room (hostile towers only).
/// Used to size drain bodies. Prefers cached value from dynamic room data (any past visibility), then live structures if visible this tick.
fn tower_dps_at_target_room(system_data: &MissionExecutionSystemData, target_room: RoomName) -> Option<f32> {
    let room_entity = system_data.mapping.get_room(&target_room)?;
    let rd = system_data.room_data.get(room_entity)?;
    if let Some(dvd) = rd.get_dynamic_visibility_data() {
        if let Some(dps) = dvd.tower_dps_at_edge() {
            return Some(dps);
        }
    }
    let structures = rd.get_structures()?;
    let positions: Vec<Position> = structures
        .towers()
        .iter()
        .filter(|t| !t.my())
        .map(|t| t.pos())
        .collect();
    Some(crate::military::damage::tower_dps_at_room_edge(target_room, &positions))
}

/// Shared wave-wipe handler: increment the wave counter and either complete
/// the mission (max waves reached) or reset squads for the next wave.
fn handle_wave_wipe(
    system_data: &mut MissionExecutionSystemData,
    state_context: &mut AttackMissionContext,
) -> Result<Option<AttackMissionState>, String> {
    state_context.current_wave += 1;
    if state_context.current_wave >= state_context.max_waves {
        info!(
            "[AttackMission] All squads wiped, max waves reached for {}",
            state_context.target_room
        );
        return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
    }

    info!(
        "[AttackMission] Wave {} failed for {}, respawning",
        state_context.current_wave, state_context.target_room
    );

    // Delete old squad entities and create fresh ones for the new wave.
    for &squad_entity in state_context.squad_entities.iter() {
        let entity = squad_entity;
        system_data.updater.exec_mut(move |world| {
            if world.entities().is_alive(entity) {
                let _ = world.delete_entity(entity);
            }
        });
    }
    state_context.squad_entities.clear();

    for planned in state_context.force_plan.iter() {
        let squad_ctx = SquadContext::from_composition(&planned.composition);
        let squad_entity = system_data
            .updater
            .create_entity(system_data.entities)
            .with(squad_ctx)
            .marked::<SerializeMarker>()
            .build();
        state_context.squad_entities.push(squad_entity);
    }

    for squad in state_context.squads.iter_mut() {
        squad.state = SquadState::Forming;
        squad.spawn_complete = false;
    }

    Ok(Some(AttackMissionState::planning(std::marker::PhantomData)))
}

impl Engaging {
    fn status_description(&self) -> String {
        "Engaging".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut AttackMissionContext,
    ) -> Result<Option<AttackMissionState>, String> {
        // ── Squad coordination: pre-compute tick orders before jobs run ──

        // Phase 1: Compute focus targets for each squad (requires room_data, not squad_contexts).
        let mut focus_targets: Vec<Option<(Position, Option<RawObjectId>)>> = Vec::new();
        for squad_idx in 0..state_context.squads.len() {
            let squad_entity = match state_context.squad_entities.get(squad_idx).copied() {
                Some(e) if system_data.entities.is_alive(e) => e,
                _ => {
                    focus_targets.push(None);
                    continue;
                }
            };

            // Read squad centroid from cached member positions (updated by SquadUpdateSystem).
            let squad_center =
                if let Some(squad_ctx) = system_data.squad_contexts.get(squad_entity) {
                    squad_ctx.members.iter().filter_map(|m| m.position).next()
                } else {
                    None
                };

            let target_room = state_context.squads[squad_idx].target_room;
            let focus = Self::compute_focus_target(target_room, squad_center, system_data);
            focus_targets.push(focus);
        }

        // Phase 2: Write tick orders (member state is updated by SquadUpdateSystem).
        for squad_idx in 0..state_context.squads.len() {
            let squad_entity = match state_context.squad_entities.get(squad_idx).copied() {
                Some(e) if system_data.entities.is_alive(e) => e,
                _ => continue,
            };

            let squad_ctx = match system_data.squad_contexts.get_mut(squad_entity) {
                Some(ctx) => ctx,
                None => continue,
            };

            // 1. Convert focus target to AttackTarget enum.
            let focus_data = focus_targets.get(squad_idx).copied().flatten();
            let attack_target: Option<AttackTarget> = focus_data.map(|(pos, id)| {
                if let Some(raw_id) = id {
                    AttackTarget::Creep(raw_id)
                } else {
                    AttackTarget::Structure(pos)
                }
            });
            // Store position on SquadContext for squad-level movement direction.
            squad_ctx.focus_target = focus_data.map(|(pos, _)| pos);

            // 2. Check retreat.
            if squad_ctx.should_retreat() {
                squad_ctx.state = SquadState::Retreating;
                squad_ctx.issue_retreat_orders(None, Some(system_data.creep_owner));
            } else {
                // 3. Write tick orders for each living member.
                for member in squad_ctx.members.iter_mut() {
                    member.tick_orders = Some(TickOrders {
                        attack_target,
                        movement: TickMovement::Formation,
                        ..Default::default()
                    });
                }

                // 4. Compute and apply heal assignments on top of attack orders.
                let heal_assignments =
                    squad_ctx.compute_heal_assignments(Some(system_data.creep_owner));
                squad_ctx.apply_heal_assignments(&heal_assignments);

                // 5. Advance the squad's virtual position toward the focus target
                //    (or room center if no focus). Individual creeps read this in
                //    their job tick to compute formation offset movement.
                let target_room = state_context.squads[squad_idx].target_room;
                let destination = attack_target
                    .and_then(|t| t.pos())
                    .unwrap_or_else(|| {
                        Position::new(
                            RoomCoordinate::new(25).unwrap(),
                            RoomCoordinate::new(25).unwrap(),
                            target_room,
                        )
                    });
                advance_squad_virtual_position(squad_ctx, destination);
            }
        }

        // ── Lifecycle management ────────────────────────────────────────

        // Update squad states based on SquadContext member counts.
        for (squad_idx, squad) in state_context.squads.iter_mut().enumerate() {
            let squad_ctx = state_context
                .squad_entities
                .get(squad_idx)
                .and_then(|&e| system_data.squad_contexts.get(e));

            let living = squad_ctx.map(|ctx| ctx.members.len()).unwrap_or(0);

            // A squad that ever had members registered means spawning happened.
            // If all members are now dead, the squad is complete regardless of
            // the spawn_complete flag (which may not have been set if the
            // Spawning state transitioned before the SquadContext was updated).
            let ever_had_members = squad_ctx.map(|ctx| ctx.ever_had_members()).unwrap_or(false);

            if living == 0 && (squad.spawn_complete || ever_had_members) {
                squad.state = SquadState::Complete;
            } else if living > 0 {
                squad.state = SquadState::Engaged;
            }
        }

        // Check for squads with AfterTargetHPPercent deploy condition.
        // These are typically haulers that should deploy when the target is low HP.
        for squad_idx in 0..state_context.squads.len() {
            if state_context.squads[squad_idx].spawn_complete {
                continue;
            }
            let plan_idx = state_context.squads[squad_idx].plan_index;
            if let Some(planned) = state_context.force_plan.get(plan_idx) {
                if let DeployCondition::AfterTargetHPPercent { percent } = &planned.deploy_condition {
                    let should_deploy = Self::check_target_hp_threshold(state_context.target_room, *percent, system_data);
                    if should_deploy {
                        Spawning::spawn_deferred_squad(system_data, mission_entity, state_context, squad_idx);
                    }
                }
            }
        }

        // Check if all squads are complete (wiped or objective done).
        let all_complete = state_context.squads.iter().all(|s| s.state == SquadState::Complete);

        if all_complete {
            // Check if the objective was actually achieved: no dangerous
            // hostiles remain in the target room. If hostiles are still
            // present (e.g. our creeps expired from TTL while enemies
            // survived), treat it as a failed wave instead.
            let hostiles_remain = Self::room_has_hostile_threats(state_context.target_room, system_data);

            if hostiles_remain {
                info!(
                    "[AttackMission] All squads complete but hostiles remain in {}",
                    state_context.target_room
                );
                // Fall through to the all_dead respawn logic below.
            } else {
                state_context.mission_succeeded = true;
                return Ok(Some(AttackMissionState::exploiting(std::marker::PhantomData)));
            }
        }

        if all_squads_wiped(system_data, state_context) {
            return handle_wave_wipe(system_data, state_context);
        }

        Ok(None)
    }

    /// Compute the squad's focus target: position + optional object ID.
    ///
    /// Returns `(position, raw_object_id)`. The ID is available for creep
    /// targets (allows the job to resolve the exact hostile without scanning
    /// the room). Structure targets only return a position since
    /// `StructureObject` doesn't expose a uniform `RawObjectId`.
    ///
    /// Priority:
    /// 1. Hostile creeps with HEAL parts (kill healers first to prevent regen).
    /// 2. Hostile creep with lowest HP (focus fire to get kills).
    /// 3. Hostile structures: invader cores > spawns > towers > others.
    fn compute_focus_target(
        target_room: RoomName,
        squad_center: Option<Position>,
        system_data: &MissionExecutionSystemData,
    ) -> Option<(Position, Option<RawObjectId>)> {
        let squad_center = squad_center?;

        // Only target things in the room we can see.
        if squad_center.room_name() != target_room {
            return None;
        }

        // Check hostile creeps via room data cache.
        let room_entity = system_data.mapping.get_room(&target_room);

        if let Some(room_entity) = room_entity {
            if let Some(room_data) = system_data.room_data.get(room_entity) {
                if let Some(creep_data) = room_data.get_creeps() {
                    let hostiles = creep_data.hostile();

                    if !hostiles.is_empty() {
                        // Priority 1: hostiles with HEAL parts.
                        let healer = hostiles
                            .iter()
                            .filter(|c| {
                                c.body()
                                    .iter()
                                    .any(|p| p.part() == Part::Heal && p.hits() > 0)
                            })
                            .min_by_key(|c| c.hits());

                        if let Some(target) = healer {
                            return Some((target.pos(), target.try_raw_id()));
                        }

                        // Priority 2: lowest HP hostile (focus fire).
                        if let Some(target) = hostiles.iter().min_by_key(|c| c.hits()) {
                            return Some((target.pos(), target.try_raw_id()));
                        }
                    }
                }

                // Priority 3: hostile structures (no object ID -- structures
                // don't move so position is sufficient for targeting).
                if let Some(structures) = room_data.get_structures() {
                    let hostile_structures: Vec<_> = structures
                        .all()
                        .iter()
                        .filter(|s| s.as_owned().map(|o| !o.my()).unwrap_or(false))
                        .collect();

                    if !hostile_structures.is_empty() {
                        // Prioritize: invader cores > spawns > towers > other.
                        let best =
                            hostile_structures
                                .iter()
                                .min_by_key(|s| match s.structure_type() {
                                    StructureType::InvaderCore => 0u32,
                                    StructureType::Spawn => 1,
                                    StructureType::Tower => 2,
                                    _ => 10,
                                });

                        if let Some(target) = best {
                            return Some((target.pos(), None));
                        }
                    }
                }
            }
        }

        None
    }

    /// Check if the primary attackable structure in the target room has dropped
    /// below the given HP fraction. Used for `AfterTargetHPPercent` deployment
    /// (e.g. spawning haulers when a power bank is nearly destroyed).
    fn check_target_hp_threshold(target_room: RoomName, percent: f32, system_data: &MissionExecutionSystemData) -> bool {
        // Find room data for the target room.
        let room_entity = match system_data.mapping.get_room(&target_room) {
            Some(e) => e,
            None => return false,
        };

        let room_data = match system_data.room_data.get(room_entity) {
            Some(rd) => rd,
            None => return false,
        };

        let structures = match room_data.get_structures() {
            Some(s) => s,
            None => return false,
        };

        // Look for the primary target structure: power banks, invader cores.
        for structure in structures.all() {
            let hp_ratio = match structure.as_attackable() {
                Some(a) => {
                    let max = a.hits_max();
                    if max == 0 {
                        continue;
                    }
                    a.hits() as f32 / max as f32
                }
                None => continue,
            };

            match structure.structure_type() {
                StructureType::PowerBank | StructureType::InvaderCore => {
                    if hp_ratio <= percent {
                        return true;
                    }
                }
                _ => {}
            }
        }

        false
    }

    /// Check whether the target room still has hostile threats that require
    /// combat squads. Returns `false` when only the controller remains
    /// (controllers can't be destroyed, only unclaimed over time) or when
    /// we have no visibility.
    fn room_has_hostile_threats(target_room: RoomName, system_data: &MissionExecutionSystemData) -> bool {
        let room_entity = match system_data.mapping.get_room(&target_room) {
            Some(e) => e,
            None => return false, // No visibility -- assume clear.
        };

        let room_data = match system_data.room_data.get(room_entity) {
            Some(rd) => rd,
            None => return false,
        };

        // Check for hostile creeps (excluding NPCs like Source Keepers which
        // respawn regardless).
        if let Some(creep_data) = room_data.get_creeps() {
            let dangerous_hostiles = creep_data
                .hostile()
                .iter()
                .filter(|c| {
                    let body = c.body();
                    body.iter()
                        .any(|p| matches!(p.part(), Part::Attack | Part::RangedAttack | Part::Heal) && p.hits() > 0)
                })
                .count();

            if dangerous_hostiles > 0 {
                return true;
            }
        }

        // Check for hostile structures that pose a threat (towers, spawns).
        // Controllers are explicitly excluded -- they can't be destroyed and
        // will downgrade on their own once all other structures are gone.
        if let Some(structures) = room_data.get_structures() {
            let threatening_structures = structures
                .all()
                .iter()
                .filter(|s| s.as_owned().map(|o| !o.my()).unwrap_or(false))
                .filter(|s| {
                    matches!(
                        s.structure_type(),
                        StructureType::Tower | StructureType::Spawn | StructureType::InvaderCore
                    )
                })
                .count();

            if threatening_structures > 0 {
                return true;
            }
        }

        false
    }
}

impl Spawning {
    /// Spawn members for a deferred squad (e.g., haulers triggered by HP threshold).
    /// Queues ALL unfilled slots to ALL capable rooms with shared tokens.
    fn spawn_deferred_squad(
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut AttackMissionContext,
        squad_idx: usize,
    ) {
        let squad = &state_context.squads[squad_idx];
        let plan_index = squad.plan_index;
        let planned = match state_context.force_plan.get(plan_index) {
            Some(p) => p,
            None => return,
        };

        let squad_entity = match state_context.squad_entities.get(squad_idx).copied() {
            Some(e) => e,
            None => return,
        };

        // Check filled status from SquadContext.
        let filled_count = system_data
            .squad_contexts
            .get(squad_entity)
            .map(|ctx| ctx.filled_slot_count())
            .unwrap_or(0);

        if filled_count >= planned.composition.slots.len() {
            state_context.squads[squad_idx].spawn_complete = true;
            state_context.squads[squad_idx].state = SquadState::Rallying;
            return;
        }

        // Queue all unfilled slots to all capable rooms.
        for (slot_index, slot) in planned.composition.slots.iter().enumerate() {
            let already_filled = system_data
                .squad_contexts
                .get(squad_entity)
                .map(|ctx| ctx.is_slot_filled(slot_index))
                .unwrap_or(false);

            if already_filled {
                continue;
            }

            let token = system_data.spawn_queue.token();

            for home_entity in state_context.home_room_datas.iter() {
                let home_room_data = match system_data.room_data.get(*home_entity) {
                    Some(rd) => rd,
                    None => continue,
                };

                let room = match game::rooms().get(home_room_data.name) {
                    Some(r) => r,
                    None => continue,
                };

                let energy_capacity = room.energy_capacity_available();
                let body_def = if matches!(slot.body_type, BodyType::Drain) {
                    let dps = tower_dps_at_target_room(system_data, state_context.target_room).unwrap_or(150.0);
                    crate::military::bodies::drain_body_for_tower_dps(energy_capacity, dps)
                } else {
                    slot.body_type.body_definition(energy_capacity)
                };

                if let Ok(body) = spawning::create_body(&body_def) {
                    let target_room = state_context.target_room;
                    let spawn_request = SpawnRequest::new(
                        format!("Atk-{:?} {}", slot.role, target_room),
                        &body,
                        SPAWN_PRIORITY_MEDIUM,
                        Some(token),
                        Self::create_spawn_callback(slot.role, slot_index, target_room, squad_entity),
                    );
                    system_data.spawn_queue.request(*home_entity, spawn_request);
                }
            }
        }
    }
}

/// Maximum ticks to spend in the exploit phase before completing.
const EXPLOIT_TIMEOUT_TICKS: u32 = 600;
/// Maximum ticks to wait for haulers to spawn before giving up.
const EXPLOIT_SPAWN_TIMEOUT_TICKS: u32 = 200;

impl Exploiting {
    fn status_description(&self) -> String {
        "Exploiting".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut AttackMissionContext,
    ) -> Result<Option<AttackMissionState>, String> {
        let tick = game::time();

        // Initialize exploit timer.
        if state_context.exploit_start_tick.is_none() {
            state_context.exploit_start_tick = Some(tick);
        }

        let exploit_age = state_context.exploit_start_tick.map(|s| tick - s).unwrap_or(0);

        // Timeout: don't exploit forever.
        if exploit_age > EXPLOIT_TIMEOUT_TICKS {
            info!("[AttackMission] Exploit timeout for {}, completing", state_context.target_room);
            return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
        }

        // SquadContext member alive status is maintained by PreRunSquadUpdateSystem.

        // Resolve the target room entity via the mapping.
        let room_entity = system_data.mapping.get_room(&state_context.target_room);

        // Check if the target room has resources worth collecting.
        let has_loot = room_entity
            .and_then(|e| system_data.room_data.get(e))
            .map(Self::estimate_loot)
            .unwrap_or(0);

        // Check if hostiles have returned (need to retreat or re-engage).
        let hostile_count = room_entity
            .and_then(|e| system_data.room_data.get(e))
            .and_then(|rd| rd.get_creeps())
            .map(|creeps| {
                creeps
                    .hostile()
                    .iter()
                    .filter(|c| !crate::military::is_npc_owner(&c.owner().username()))
                    .count()
            })
            .unwrap_or(0);

        if hostile_count > 0 {
            // Hostiles returned -- check if we have combat squads still alive.
            let combat_alive = state_context.squads.iter().enumerate().any(|(idx, s)| {
                let living = state_context
                    .squad_entities
                    .get(idx)
                    .and_then(|&e| system_data.squad_contexts.get(e))
                    .map(|ctx| ctx.members.len())
                    .unwrap_or(0);
                living > 0 && s.state != SquadState::Complete
            });

            if !combat_alive {
                info!(
                    "[AttackMission] Hostiles returned to {} with no combat squads, retreating",
                    state_context.target_room
                );
                return Ok(Some(AttackMissionState::retreating(std::marker::PhantomData)));
            }
        }

        // Spawn hauler squads if not already done and there's loot.
        if !state_context.exploit_haulers_spawned && has_loot > 500 {
            state_context.exploit_haulers_spawned = true;

            // Determine hauler count based on loot amount.
            let hauler_count = if has_loot > 50_000 {
                4
            } else if has_loot > 20_000 {
                3
            } else if has_loot > 5_000 {
                2
            } else {
                1
            };

            let hauler_composition = SquadComposition::power_bank_haulers(hauler_count);
            let hauler_plan_idx = state_context.force_plan.len();

            // Create a SquadContext entity for the hauler squad.
            let hauler_squad_ctx = SquadContext::from_composition(&hauler_composition);

            state_context.force_plan.push(PlannedSquad {
                composition: hauler_composition,
                target: SquadTarget::CollectResources {
                    room: state_context.target_room,
                },
                deploy_condition: DeployCondition::Immediate,
            });
            let hauler_squad_entity = system_data
                .updater
                .create_entity(system_data.entities)
                .with(hauler_squad_ctx)
                .marked::<SerializeMarker>()
                .build();

            state_context.squads.push(ManagedSquad {
                plan_index: hauler_plan_idx,
                state: SquadState::Forming,
                target_room: state_context.target_room,
                expected_count: hauler_count,
                spawn_complete: false,
            });
            state_context.squad_entities.push(hauler_squad_entity);

            info!(
                "[AttackMission] Spawning {} haulers for {} (estimated loot: {})",
                hauler_count, state_context.target_room, has_loot
            );

            // Also spawn a protection squad if the room had significant defenses.
            let had_defenses = state_context
                .force_plan
                .iter()
                .any(|p| matches!(p.target, SquadTarget::AttackRoom { .. }));

            if had_defenses && has_loot > 10_000 {
                let guard_plan_idx = state_context.force_plan.len();
                state_context.force_plan.push(PlannedSquad {
                    composition: SquadComposition::solo_ranged(),
                    target: SquadTarget::DefendRoom {
                        room: state_context.target_room,
                    },
                    deploy_condition: DeployCondition::Immediate,
                });

                let guard_composition = SquadComposition::solo_ranged();
                let guard_squad_ctx = SquadContext::from_composition(&guard_composition);
                let guard_squad_entity = system_data
                    .updater
                    .create_entity(system_data.entities)
                    .with(guard_squad_ctx)
                    .marked::<SerializeMarker>()
                    .build();

                state_context.squads.push(ManagedSquad {
                    plan_index: guard_plan_idx,
                    state: SquadState::Forming,
                    target_room: state_context.target_room,
                    expected_count: 1,
                    spawn_complete: false,
                });
                state_context.squad_entities.push(guard_squad_entity);

                info!("[AttackMission] Spawning guard for exploit in {}", state_context.target_room);
            }
        }

        // Spawn exploit-phase squad members (haulers + guards).
        for squad_idx in 0..state_context.squads.len() {
            if state_context.squads[squad_idx].spawn_complete {
                continue;
            }

            let plan_idx = state_context.squads[squad_idx].plan_index;
            let is_exploit_squad = state_context
                .force_plan
                .get(plan_idx)
                .map(|p| matches!(p.target, SquadTarget::CollectResources { .. } | SquadTarget::DefendRoom { .. }))
                .unwrap_or(false);

            if !is_exploit_squad {
                continue;
            }

            Spawning::spawn_deferred_squad(system_data, mission_entity, state_context, squad_idx);
        }

        // Check if all exploit-phase squads are done (spawned and dead, or timed out).
        // This includes both hauler (CollectResources) and guard (DefendRoom) squads.
        let exploit_squads_active = state_context.squads.iter().enumerate().any(|(idx, s)| {
            let is_exploit = state_context
                .force_plan
                .get(s.plan_index)
                .map(|p| matches!(p.target, SquadTarget::CollectResources { .. } | SquadTarget::DefendRoom { .. }))
                .unwrap_or(false);

            if !is_exploit {
                return false;
            }

            if !s.spawn_complete {
                return true;
            }

            let living = state_context
                .squad_entities
                .get(idx)
                .and_then(|&e| system_data.squad_contexts.get(e))
                .map(|ctx| ctx.members.len())
                .unwrap_or(0);
            living > 0
        });

        // Complete if: no loot left, or all exploit squads done, or global timeout.
        if state_context.exploit_haulers_spawned && !exploit_squads_active {
            info!(
                "[AttackMission] Exploit complete for {} (all exploit squads done)",
                state_context.target_room
            );
            return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
        }

        if has_loot == 0 && exploit_age > 50 {
            info!(
                "[AttackMission] No loot remaining in {}, completing exploit",
                state_context.target_room
            );
            return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
        }

        if !state_context.exploit_haulers_spawned && exploit_age > EXPLOIT_SPAWN_TIMEOUT_TICKS {
            info!(
                "[AttackMission] No loot found in {} after {}t, completing exploit",
                state_context.target_room, exploit_age
            );
            return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
        }

        Ok(None)
    }

    /// Estimate the total loot value in the target room using cached room data.
    fn estimate_loot(room_data: &crate::room::data::RoomData) -> u32 {
        let mut total: u32 = 0;

        // Loot from structures (storages, terminals, containers, power banks).
        if let Some(structures) = room_data.get_structures() {
            for storage in structures.storages() {
                if !storage.my() {
                    total += storage.store().get_used_capacity(Some(ResourceType::Energy)).min(100_000);
                }
            }

            for terminal in structures.terminals() {
                if !terminal.my() {
                    total += terminal.store().get_used_capacity(Some(ResourceType::Energy)).min(100_000);
                }
            }

            for container in structures.containers() {
                total += container.store().get_used_capacity(Some(ResourceType::Energy)).min(10_000);
            }

            for power_bank in structures.power_banks() {
                total += power_bank.power();
            }
        }

        // Loot from dropped resources, tombstones, and ruins.
        if let Some(dropped) = room_data.get_dropped_resources() {
            total += dropped.total_loot_value();
        }

        total
    }
}

impl Retreating {
    fn status_description(&self) -> String {
        "Retreating".to_string()
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut AttackMissionContext,
    ) -> Result<Option<AttackMissionState>, String> {
        // SquadContext member alive status is maintained by PreRunSquadUpdateSystem.

        // If all squads are dead, complete.
        let all_dead = state_context
            .squad_entities
            .iter()
            .all(|&e| system_data.squad_contexts.get(e).map(|ctx| ctx.members.is_empty()).unwrap_or(true));

        if all_dead {
            return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
        }

        // Retreat timeout: after 200 ticks of retreating, abort.
        let retreat_age = state_context.start_tick.map(|s| game::time() - s).unwrap_or(0);

        if retreat_age > 200 {
            return Ok(Some(AttackMissionState::mission_complete(std::marker::PhantomData)));
        }

        Ok(None)
    }
}

impl MissionComplete {
    fn status_description(&self) -> String {
        "Complete".to_string()
    }

    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut AttackMissionContext,
    ) -> Result<Option<AttackMissionState>, String> {
        Ok(None)
    }
}

// ─── AttackMission ──────────────────────────────────────────────────────────

#[derive(ConvertSaveload)]
pub struct AttackMission {
    owner: EntityOption<Entity>,
    context: AttackMissionContext,
    state: AttackMissionState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl AttackMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, target_room: RoomName, force_plan: Vec<PlannedSquad>, max_waves: u32) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = AttackMission::new(owner, target_room, force_plan, max_waves);

        builder
            .with(MissionData::AttackMission(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, target_room: RoomName, force_plan: Vec<PlannedSquad>, max_waves: u32) -> AttackMission {
        AttackMission {
            owner: owner.into(),
            context: AttackMissionContext {
                target_room,
                home_room_datas: EntityVec::new(),
                force_plan,
                squads: EntityVec::new(),
                squad_entities: EntityVec::new(),
                current_wave: 0,
                max_waves,
                start_tick: None,
                energy_invested: 0,
                mission_succeeded: false,
                exploit_start_tick: None,
                exploit_haulers_spawned: false,
            },
            state: AttackMissionState::planning(std::marker::PhantomData),
        }
    }

    /// Update the home rooms used for spawning. Called by the parent
    /// `AttackOperation` via `LazyUpdate` whenever home room assignments change.
    pub fn set_home_rooms(&mut self, rooms: EntityVec<Entity>) {
        self.context.home_room_datas = rooms;
    }

    /// Get the target room for this mission.
    pub fn target_room(&self) -> RoomName {
        self.context.target_room
    }

    /// Get the current wave number.
    pub fn current_wave(&self) -> u32 {
        self.context.current_wave
    }

    /// Get total energy invested.
    pub fn energy_invested(&self) -> u32 {
        self.context.energy_invested
    }

    /// Whether the attack achieved its objective (defenses cleared).
    pub fn mission_succeeded(&self) -> bool {
        self.context.mission_succeeded
    }

    /// Whether the mission is currently in the Exploiting state.
    pub fn is_exploiting(&self) -> bool {
        matches!(self.state, AttackMissionState::Exploiting(_))
    }

    /// Rich tree summary with per-squad state, orders, and each member's room/status/orders.
    /// Used by SummarizeMissionSystem when SquadContext and CreepOwner storages are available.
    pub fn summarize_with(
        &self,
        squad_contexts: &specs::ReadStorage<SquadContext>,
        creep_owner: &specs::ReadStorage<CreepOwner>,
    ) -> SummaryContent {
        let ctx = &self.context;
        let mut children = Vec::new();

        // Mission-level: vertical tree items to save horizontal space.
        let age = ctx.start_tick.map(|s| game::time().saturating_sub(s));
        let exploit_age = ctx.exploit_start_tick.map(|s| game::time().saturating_sub(s));

        children.push(SummaryContent::Tree {
            label: "mission".to_string(),
            children: vec![
                SummaryContent::Text(format!("wave {}/{}", ctx.current_wave, ctx.max_waves)),
                SummaryContent::Text(format!("succeeded: {}", ctx.mission_succeeded)),
                SummaryContent::Text(format!(
                    "age: {}",
                    age.map(|a| a.to_string()).unwrap_or_else(|| "-".into())
                )),
                SummaryContent::Text(format!(
                    "exploit_age: {}",
                    exploit_age.map(|a| a.to_string()).unwrap_or_else(|| "-".into())
                )),
                SummaryContent::Text(format!("haulers_spawned: {}", ctx.exploit_haulers_spawned)),
                SummaryContent::Text(format!("homes: {} squads: {} plans: {}", ctx.home_room_datas.len(), ctx.squad_entities.len(), ctx.force_plan.len())),
            ],
        });

        // Per-squad: state tree, orders tree, then one tree per member (name + room, status, orders).
        for (i, squad) in ctx.squads.iter().enumerate() {
            let squad_entity = match ctx.squad_entities.get(i).copied() {
                Some(e) => e,
                None => {
                    let plan = ctx.force_plan.get(squad.plan_index);
                    let comp_label = plan.map(|p| p.composition.label.as_str()).unwrap_or("?");
                    children.push(SummaryContent::Tree {
                        label: format!("sq[{}] {} (no entity)", i, comp_label),
                        children: vec![
                            SummaryContent::Text(format!("state: {:?}", squad.state)),
                            SummaryContent::Text(format!("spawn_done: {}", squad.spawn_complete)),
                        ],
                    });
                    continue;
                }
            };

            let squad_ctx = match squad_contexts.get(squad_entity) {
                Some(c) => c,
                None => {
                    let plan = ctx.force_plan.get(squad.plan_index);
                    let comp_label = plan.map(|p| p.composition.label.as_str()).unwrap_or("?");
                    children.push(SummaryContent::Tree {
                        label: format!("sq[{}] {} (no context)", i, comp_label),
                        children: vec![
                            SummaryContent::Text(format!("state: {:?}", squad.state)),
                            SummaryContent::Text(format!("spawn_done: {}", squad.spawn_complete)),
                        ],
                    });
                    continue;
                }
            };

            let plan = ctx.force_plan.get(squad.plan_index);
            let comp_label = plan.map(|p| p.composition.label.as_str()).unwrap_or("?");
            let target_label = plan
                .map(|p| match &p.target {
                    SquadTarget::DefendRoom { room } => format!("defend {}", room),
                    SquadTarget::AttackRoom { room } => format!("attack {}", room),
                    SquadTarget::HarassRoom { room } => format!("harass {}", room),
                    SquadTarget::CollectResources { room } => format!("loot {}", room),
                    SquadTarget::MoveToPosition { position } => format!("move {}", position),
                    SquadTarget::AttackStructure { position } => format!("struct {}", position),
                    SquadTarget::EscortPosition { position } => format!("escort {}", position),
                })
                .unwrap_or_else(|| "?".into());

            let mut sq_children = Vec::new();

            // Squad state subtree (vertical items).
            sq_children.push(SummaryContent::Tree {
                label: "state".to_string(),
                children: vec![
                    SummaryContent::Text(format!("{:?}", squad.state)),
                    SummaryContent::Text(format!("spawn_done: {}", squad.spawn_complete)),
                    SummaryContent::Text(format!("expected: {}", squad.expected_count)),
                    SummaryContent::Text(format!("target: {}", target_label)),
                ],
            });

            // Squad orders subtree (focus, formation, retreat).
            let orders_children = {
                let mut lines = vec![
                    SummaryContent::Text(format!("formation: {:?}", squad_ctx.formation_mode)),
                    SummaryContent::Text(format!("retreat_threshold: {}", squad_ctx.retreat_threshold)),
                ];
                if let Some(focus) = squad_ctx.focus_target {
                    lines.insert(0, SummaryContent::Text(format!("focus: {}", focus)));
                }
                if squad_ctx.strict_hold_ticks > 0 {
                    lines.push(SummaryContent::Text(format!("strict_hold_ticks: {}", squad_ctx.strict_hold_ticks)));
                }
                lines
            };
            sq_children.push(SummaryContent::Tree {
                label: "orders".to_string(),
                children: orders_children,
            });

            // Per-member tree: label "name (RoomName)" so users can find creeps in the world.
            for member in squad_ctx.members.iter() {
                let name = creep_owner
                    .get(member.entity)
                    .and_then(|co| co.owner.resolve())
                    .map(|c| c.name())
                    .unwrap_or_else(|| format!("entity_{:?}", member.entity.id()));
                let room_str = member
                    .position
                    .map(|p| format!("{}", p.room_name()))
                    .unwrap_or_else(|| "?".to_string());
                let member_label = format!("{} ({})", name, room_str);

                let mut member_children = vec![
                    SummaryContent::Text(format!("role: {:?}", member.role)),
                    SummaryContent::Text(format!("HP: {}/{}", member.current_hits, member.max_hits)),
                    SummaryContent::Text(format!("room: {}", room_str)),
                ];
                if let Some(ref orders) = member.tick_orders {
                    member_children.push(SummaryContent::Text(format!("movement: {:?}", orders.movement)));
                    if orders.attack_target.is_some() {
                        member_children.push(SummaryContent::Text("attack: focus".to_string()));
                    }
                    if orders.heal_target.is_some() {
                        member_children.push(SummaryContent::Text("heal: assigned".to_string()));
                    }
                }
                sq_children.push(SummaryContent::Tree {
                    label: member_label,
                    children: member_children,
                });
            }

            children.push(SummaryContent::Tree {
                label: format!("sq[{}] {}", i, comp_label),
                children: sq_children,
            });
        }

        SummaryContent::Tree {
            label: format!("AttackMission -> {} ({})", ctx.target_room, self.state.status_description()),
            children,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for AttackMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        // Return the first home room for cleanup purposes.
        // The operation also attaches the mission to the target room;
        // the integrity pass in repair_entity_integrity handles removing
        // stale mission references from all rooms.
        //
        // If home rooms have been cleaned up (e.g. integrity repair removed
        // dead entities), fall back to the owner entity. The cleanup system
        // uses this to remove the mission from the room's mission list; if
        // the entity doesn't correspond to a room, the removal is a no-op.
        if let Some(entity) = self.context.home_room_datas.first() {
            *entity
        } else if let Some(owner) = *self.owner {
            error!(
                "[AttackMission] get_room: no home rooms for {}, using owner",
                self.context.target_room
            );
            owner
        } else {
            error!(
                "[AttackMission] get_room: no home rooms and no owner for {}",
                self.context.target_room
            );
            // Return the first squad entity as a last resort. The cleanup
            // system will fail to find a room for it, which is acceptable.
            self.context
                .squad_entities
                .first()
                .copied()
                .expect("AttackMission must have at least one entity reference")
        }
    }

    fn get_children(&self) -> Vec<Entity> {
        // Squad context entities are auxiliary children that should be cleaned up
        // when the mission is deleted. The cleanup system handles non-mission
        // children by deleting them directly.
        self.context.squad_entities.iter().copied().collect()
    }

    fn remove_creep(&mut self, _entity: Entity) {
        // Member tracking is delegated to SquadContext; PreRunSquadUpdateSystem
        // marks dead members. Nothing to do here.
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        self.context.home_room_datas.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead home room entity {:?} removed from AttackMission", e);
            }
            ok
        });
        self.context.squad_entities.retain(|e| {
            let ok = is_valid(*e);
            if !ok {
                error!("INTEGRITY: dead squad entity {:?} removed from AttackMission", e);
            }
            ok
        });
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String {
        self.state.describe_state(system_data, mission_entity, &self.context)
    }

    fn summarize(&self) -> SummaryContent {
        let ctx = &self.context;
        let mut children = Vec::new();

        // Mission-level state.
        let age = ctx.start_tick.map(|s| game::time().saturating_sub(s));
        let exploit_age = ctx.exploit_start_tick.map(|s| game::time().saturating_sub(s));

        children.push(SummaryContent::Text(format!(
            "wave {}/{} | succeeded: {}",
            ctx.current_wave, ctx.max_waves, ctx.mission_succeeded,
        )));
        children.push(SummaryContent::Text(format!(
            "age: {} | exploit: {} | haulers_spawned: {}",
            age.map(|a| a.to_string()).unwrap_or_else(|| "-".into()),
            exploit_age.map(|a| a.to_string()).unwrap_or_else(|| "-".into()),
            ctx.exploit_haulers_spawned,
        )));
        children.push(SummaryContent::Text(format!(
            "homes: {} | entities: {} | plans: {}",
            ctx.home_room_datas.len(),
            ctx.squad_entities.len(),
            ctx.force_plan.len(),
        )));

        // Per-squad tree items.
        for (i, squad) in ctx.squads.iter().enumerate() {
            let has_entity = ctx.squad_entities.get(i).is_some();
            let plan = ctx.force_plan.get(squad.plan_index);
            let comp_label = plan.map(|p| p.composition.label.as_str()).unwrap_or("?");
            let target_label = plan
                .map(|p| match &p.target {
                    SquadTarget::DefendRoom { room } => format!("defend {}", room),
                    SquadTarget::AttackRoom { room } => format!("attack {}", room),
                    SquadTarget::HarassRoom { room } => format!("harass {}", room),
                    SquadTarget::CollectResources { room } => format!("loot {}", room),
                    SquadTarget::MoveToPosition { position } => format!("move {}", position),
                    SquadTarget::AttackStructure { position } => format!("struct {}", position),
                    SquadTarget::EscortPosition { position } => format!("escort {}", position),
                })
                .unwrap_or_else(|| "?".into());

            let mut sq_children = Vec::new();
            sq_children.push(SummaryContent::Text(format!(
                "state: {:?} | spawn_done: {}",
                squad.state, squad.spawn_complete,
            )));
            sq_children.push(SummaryContent::Text(format!(
                "expect: {} | target: {} | entity: {}",
                squad.expected_count,
                target_label,
                if has_entity { "ok" } else { "MISSING" },
            )));

            children.push(SummaryContent::Tree {
                label: format!("sq[{}] {}", i, comp_label),
                children: sq_children,
            });
        }

        SummaryContent::Tree {
            label: format!("AttackMission -> {} ({})", ctx.target_room, self.state.status_description(),),
            children,
        }
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        self.state.gather_data(system_data, mission_entity);
        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        crate::machine_tick::run_state_machine_result(&mut self.state, "AttackMission", |state| {
            state.tick(system_data, mission_entity, &mut self.context)
        })?;

        self.state.visualize(system_data, mission_entity, &self.context);

        if matches!(self.state, AttackMissionState::MissionComplete(_)) {
            // Squad entities are cleaned up by the EntityCleanupSystem via
            // get_children() when this mission entity is deleted.
            return Ok(MissionResult::Success);
        }

        Ok(MissionResult::Running)
    }
}
