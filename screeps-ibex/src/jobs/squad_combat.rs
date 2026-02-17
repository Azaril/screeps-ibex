use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::movebehavior::*;
use crate::military::formation::virtual_anchor_target;
use crate::military::squad::*;
use crate::visualization::SummaryContent;
use screeps::*;
use screeps_machine::*;
use screeps_rover::*;
use serde::*;
use specs::Entity;

#[derive(Clone, Serialize, Deserialize)]
pub struct SquadCombatJobContext {
    target_room: RoomName,
    /// Squad entity ID for coordinated behavior.
    #[serde(default)]
    pub(crate) squad_entity: Option<u32>,
    /// Tick when we entered the combat response state (for timeout).
    #[serde(default)]
    combat_response_start: Option<u32>,
}

/// Maximum ticks to spend in combat response before resuming objective.
const COMBAT_RESPONSE_TIMEOUT: u32 = 50;

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum SquadCombatState {
        /// Traveling to the target room. Will transition to CombatResponse
        /// if attacked en route.
        MoveToRoom,
        /// Temporarily engaged in combat while not at objective.
        /// Fights back and attempts to disengage to resume travel.
        CombatResponse,
        /// At the objective room, actively fighting.
        Engaged,
        /// Withdrawing from combat due to low HP or squad retreat signal.
        Retreating
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut SquadCombatJobContext, tick_context: &mut JobTickContext) -> Option<SquadCombatState>;
    }
);

// ─── MoveToRoom ─────────────────────────────────────────────────────────────

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut SquadCombatJobContext, tick_context: &mut JobTickContext) -> Option<SquadCombatState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();

        // Check for hostiles in the current room -- respond to ambush.
        if creep_pos.room_name() != state_context.target_room {
            let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);
            let nearby_threats = hostiles
                .iter()
                .any(|c| creep_pos.get_range_to(c.pos()) <= 5);

            if nearby_threats {
                // Under attack while traveling -- enter combat response.
                state_context.combat_response_start = Some(game::time());
                return Some(SquadCombatState::combat_response());
            }
        }

        // Check squad retreat signal.
        if let Some(squad_state) = get_squad_state(state_context.squad_entity, tick_context) {
            if squad_state == SquadState::Retreating {
                return Some(SquadCombatState::retreating());
            }
        }

        let room_options = RoomOptions::new(HostileBehavior::HighCost);

        tick_move_to_room(
            tick_context,
            state_context.target_room,
            Some(room_options),
            SquadCombatState::engaged,
        )
    }
}

// ─── CombatResponse ─────────────────────────────────────────────────────────

impl CombatResponse {
    pub fn tick(&mut self, state_context: &mut SquadCombatJobContext, tick_context: &mut JobTickContext) -> Option<SquadCombatState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();
        let creep_entity = tick_context.runtime_data.creep_entity;

        // Check squad retreat signal.
        if let Some(squad_state) = get_squad_state(state_context.squad_entity, tick_context) {
            if squad_state == SquadState::Retreating {
                return Some(SquadCombatState::retreating());
            }
        }

        // Retreat if HP drops below 40%.
        if creep.hits() < creep.hits_max() * 2 / 5 {
            return Some(SquadCombatState::retreating());
        }

        // If we've reached the target room, switch to full engagement.
        if creep_pos.room_name() == state_context.target_room {
            return Some(SquadCombatState::engaged());
        }

        // Check if threats have cleared or timeout reached.
        let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);
        let threats_nearby = hostiles
            .iter()
            .any(|c| creep_pos.get_range_to(c.pos()) <= 6);

        let timed_out = state_context
            .combat_response_start
            .map(|start| game::time() - start > COMBAT_RESPONSE_TIMEOUT)
            .unwrap_or(false);

        if !threats_nearby || timed_out {
            // Threats cleared or timeout -- resume travel.
            state_context.combat_response_start = None;
            return Some(SquadCombatState::move_to_room());
        }

        // Fight back: ranged attack + heal while kiting.
        let tick_orders = get_tick_orders(state_context.squad_entity, creep_entity, tick_context);

        // Ranged attack nearest hostile (Pipeline B).
        {
            // Mass attack if multiple hostiles nearby.
            let in_range_3: usize = hostiles.iter().filter(|c| creep_pos.get_range_to(c.pos()) <= 3).count();
            let in_range_1: usize = hostiles.iter().filter(|c| creep_pos.get_range_to(c.pos()) <= 1).count();

            if (in_range_3 >= 2 || in_range_1 >= 1)
                && tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_MASS_ATTACK)
            {
                let _ = creep.ranged_mass_attack();
            } else if let Some(target) = hostiles.iter().min_by_key(|c| creep_pos.get_range_to(c.pos())) {
                if creep_pos.get_range_to(target.pos()) <= 3
                    && tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_ATTACK)
                {
                    let _ = creep.ranged_attack(target);
                }
            }

            // Melee attack if adjacent (Pipeline A -- independent of ranged).
            if let Some(target) = hostiles.iter().find(|c| creep_pos.get_range_to(c.pos()) <= 1) {
                if tick_context.action_flags.consume(SimultaneousActionFlags::ATTACK) {
                    let _ = creep.attack(target);
                }
            }
        }

        // Self-heal (Pipeline C -- independent of attack).
        if creep.hits() < creep.hits_max() && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
            let _ = creep.heal(creep);
        }

        // Movement: kite if tick orders say Formation, otherwise follow orders.
        if let Some(ref orders) = tick_orders {
            match &orders.movement {
                TickMovement::Flee => {
                    Self::flee_from_hostiles(tick_context);
                }
                TickMovement::Formation | TickMovement::Hold => {
                    // During combat response, default to kiting toward objective.
                    Self::kite_toward_objective(tick_context, state_context);
                }
                TickMovement::MoveTo(pos) => {
                    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                        tick_context
                            .runtime_data
                            .movement
                            .move_to(creep_entity, *pos)
                            .range(1)
                            .priority(MovementPriority::High);
                    }
                }
            }
        } else {
            // No tick orders -- kite toward objective by default.
            Self::kite_toward_objective(tick_context, state_context);
        }

        None
    }

    /// Kite toward the objective room while maintaining range from hostiles.
    fn kite_toward_objective(tick_context: &mut JobTickContext, state_context: &SquadCombatJobContext) {
        let creep_entity = tick_context.runtime_data.creep_entity;

        // Move toward the target room center.
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            let target_pos = Position::new(
                RoomCoordinate::new(25).unwrap(),
                RoomCoordinate::new(25).unwrap(),
                state_context.target_room,
            );
            tick_context
                .runtime_data
                .movement
                .move_to(creep_entity, target_pos)
                .range(20)
                .priority(MovementPriority::High);
        }
    }

    /// Flee from nearby hostiles.
    fn flee_from_hostiles(tick_context: &mut JobTickContext) {
        let creep = tick_context.runtime_data.owner;
        let creep_entity = tick_context.runtime_data.creep_entity;

        let hostiles = get_hostile_creeps(creep.pos().room_name(), tick_context);
        let flee_targets: Vec<FleeTarget> = hostiles
            .iter()
            .map(|c| FleeTarget {
                pos: c.pos(),
                range: 8,
            })
            .collect();

        if !flee_targets.is_empty() && tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .flee(creep_entity, flee_targets)
                .range(8);
        }
    }
}

// ─── Engaged ────────────────────────────────────────────────────────────────

impl Engaged {
    pub fn tick(&mut self, state_context: &mut SquadCombatJobContext, tick_context: &mut JobTickContext) -> Option<SquadCombatState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();
        let creep_entity = tick_context.runtime_data.creep_entity;

        // Read tick orders from squad context if available.
        let tick_orders = get_tick_orders(state_context.squad_entity, creep_entity, tick_context);

        // Check squad retreat signal.
        if let Some(squad_state) = get_squad_state(state_context.squad_entity, tick_context) {
            if squad_state == SquadState::Retreating {
                return Some(SquadCombatState::retreating());
            }
        }

        // Retreat if HP drops below 50% (unless squad overrides).
        if creep.hits() < creep.hits_max() / 2 {
            return Some(SquadCombatState::retreating());
        }

        // If we've left the target room, move back.
        if creep_pos.room_name() != state_context.target_room {
            return Some(SquadCombatState::move_to_room());
        }

        // Check if renewing.
        if let Some(ref orders) = tick_orders {
            if orders.renewing {
                return None;
            }
        }

        // Execute attack orders (Pipeline A or B).
        if let Some(ref orders) = tick_orders {
            if let Some(attack_pos) = orders.attack_target {
                let range = creep_pos.get_range_to(attack_pos);
                let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);

                // Try ranged attack (Pipeline B).
                if range <= 3 && tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_ATTACK) {
                    if let Some(target) = hostiles.iter().min_by_key(|c| creep_pos.get_range_to(c.pos())) {
                        if creep_pos.get_range_to(target.pos()) <= 3 {
                            let _ = creep.ranged_attack(target);
                        }
                    }
                }

                // Try melee attack (Pipeline A) -- independent of ranged.
                if range <= 1 && tick_context.action_flags.consume(SimultaneousActionFlags::ATTACK) {
                    if let Some(target) = hostiles.iter().find(|c| creep_pos.get_range_to(c.pos()) <= 1) {
                        let _ = creep.attack(target);
                    }
                }
            }
        } else {
            // No tick orders -- fallback: attack nearest hostile.
            Self::fallback_attack(tick_context);
        }

        // Execute heal orders (Pipeline C -- independent of attack pipelines).
        if let Some(ref orders) = tick_orders {
            if let Some(heal_pos) = orders.heal_target_pos {
                let range = creep_pos.get_range_to(heal_pos);
                let my_creeps = get_friendly_creeps(creep_pos.room_name(), tick_context);

                // Find the actual creep at the heal target position.
                let heal_target = my_creeps
                    .iter()
                    .filter(|c| c.pos() == heal_pos || creep_pos.get_range_to(c.pos()) <= 3)
                    .min_by_key(|c| c.pos().get_range_to(heal_pos));

                if let Some(target) = heal_target {
                    let actual_range = creep_pos.get_range_to(target.pos());
                    if actual_range <= 1 && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                        // Adjacent heal is always preferred (12 HP/part vs 4 HP/part).
                        let _ = creep.heal(target);
                    } else if actual_range <= 3 && tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_HEAL) {
                        let _ = creep.ranged_heal(target);
                    }
                } else if range <= 1 && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                    // Target not found at position -- self-heal as fallback.
                    let _ = creep.heal(creep);
                }
            } else {
                // No heal target assigned but we have orders -- self-heal if damaged.
                if creep.hits() < creep.hits_max() && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                    let _ = creep.heal(creep);
                }
            }
        } else {
            // No tick orders -- fallback: self-heal if damaged.
            if creep.hits() < creep.hits_max() && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                let _ = creep.heal(creep);
            }
        }

        // Issue movement based on tick orders.
        if let Some(ref orders) = tick_orders {
            match &orders.movement {
                TickMovement::Formation => {
                    // Read the squad's virtual position and this member's formation
                    // slot, then move toward the computed formation offset tile.
                    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                        let moved = (|| {
                            let id = state_context.squad_entity?;
                            let entity = tick_context.system_data.entities.entity(id);
                            let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
                            let member = squad_ctx.get_member(creep_entity)?;
                            let virtual_pos = squad_ctx.squad_path.as_ref().map(|p| p.virtual_pos)?;
                            let layout = squad_ctx.layout.as_ref()?;
                            let target_tile = virtual_anchor_target(virtual_pos, layout, member.formation_slot)?;
                            tick_context
                                .runtime_data
                                .movement
                                .move_to(creep_entity, target_tile)
                                .range(0)
                                .priority(MovementPriority::High);
                            Some(())
                        })();

                        if moved.is_none() {
                            // Fallback: no squad path or layout -- move toward focus target.
                            if let Some(ref orders) = tick_orders {
                                if let Some(target) = orders.attack_target {
                                    tick_context
                                        .runtime_data
                                        .movement
                                        .move_to(creep_entity, target)
                                        .range(1)
                                        .priority(MovementPriority::High);
                                }
                            }
                        }
                    }
                }
                TickMovement::MoveTo(pos) => {
                    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                        tick_context
                            .runtime_data
                            .movement
                            .move_to(creep_entity, *pos)
                            .range(1)
                            .priority(MovementPriority::High);
                    }
                }
                TickMovement::Flee => {
                    CombatResponse::flee_from_hostiles(tick_context);
                }
                TickMovement::Hold => {
                    // Stay put.
                }
            }
        } else {
            // No tick orders -- fallback: move toward nearest hostile.
            Self::fallback_movement(tick_context, state_context);
        }

        None
    }

    /// Fallback attack behavior when no tick orders are available.
    /// Focus fire: prefer lowest HP hostile to get kills faster.
    fn fallback_attack(tick_context: &mut JobTickContext) {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();

        let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);

        if hostiles.is_empty() {
            // Attack structures: prioritize invader cores > spawns > towers.
            let structures = get_hostile_structures(creep_pos.room_name(), tick_context);
            let target = structures.iter()
                .filter(|s| creep_pos.get_range_to(s.pos()) <= 3)
                .min_by_key(|s| match s.structure_type() {
                    StructureType::InvaderCore => 0u32,
                    StructureType::Spawn => 1,
                    StructureType::Tower => 2,
                    _ => 10,
                });
            if let Some(target) = target {
                if tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_ATTACK) {
                    if let Some(attackable) = target.as_attackable() {
                        let _ = creep.ranged_attack(attackable);
                    }
                }
            }
            return;
        }

        // Use mass attack only when 3+ hostiles are at range 1 (it does less
        // single-target damage than ranged_attack at range 3).
        let in_range_1: usize = hostiles.iter().filter(|c| creep_pos.get_range_to(c.pos()) <= 1).count();

        if in_range_1 >= 3
            && tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_MASS_ATTACK)
        {
            let _ = creep.ranged_mass_attack();
        } else {
            // Focus fire: attack the hostile with lowest HP in range 3.
            let target = hostiles
                .iter()
                .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                .min_by_key(|c| c.hits());

            if let Some(target) = target {
                if tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_ATTACK) {
                    let _ = creep.ranged_attack(target);
                }
            }
        }

        // Self-heal (Pipeline C -- independent of ranged attack).
        if creep.hits() < creep.hits_max() && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
            let _ = creep.heal(creep);
        }
    }

    /// Fallback movement when no tick orders are available.
    /// Kite melee hostiles at range 3; flee if they close to range 1-2.
    fn fallback_movement(tick_context: &mut JobTickContext, state_context: &SquadCombatJobContext) {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();
        let creep_entity = tick_context.runtime_data.creep_entity;

        let hostiles = get_hostile_creeps(state_context.target_room, tick_context);
        if let Some(target) = hostiles.iter().min_by_key(|c| creep_pos.get_range_to(c.pos())) {
            let range = creep_pos.get_range_to(target.pos());

            // Check if the nearest hostile is melee-only (has ATTACK but no RANGED_ATTACK).
            let is_melee = target.body().iter().any(|p| p.part() == Part::Attack && p.hits() > 0)
                && !target.body().iter().any(|p| p.part() == Part::RangedAttack && p.hits() > 0);

            if is_melee && range <= 2 {
                // Melee hostile too close -- flee to maintain kiting distance.
                if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                    CombatResponse::flee_from_hostiles(tick_context);
                }
            } else if range > 3 && tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                // Too far -- close to range 3.
                tick_context
                    .runtime_data
                    .movement
                    .move_to(creep_entity, target.pos())
                    .range(3)
                    .priority(MovementPriority::High);
            } else {
                mark_working(tick_context, target.pos(), 3);
            }
        }
    }
}

// ─── Retreating ─────────────────────────────────────────────────────────────

impl Retreating {
    pub fn tick(&mut self, state_context: &mut SquadCombatJobContext, tick_context: &mut JobTickContext) -> Option<SquadCombatState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();
        let creep_entity = tick_context.runtime_data.creep_entity;

        // Check squad state -- re-engage if squad says so.
        let squad_state = get_squad_state(state_context.squad_entity, tick_context);
        let squad_wants_engage = squad_state
            .map(|s| s == SquadState::Engaged || s == SquadState::Moving)
            .unwrap_or(false);

        // Re-engage once HP recovers above 80%, or if squad signals engage.
        if creep.hits() > creep.hits_max() * 4 / 5
            || (squad_wants_engage && creep.hits() > creep.hits_max() * 3 / 5)
        {
            return Some(SquadCombatState::engaged());
        }

        // Read tick orders for coordinated retreat.
        let tick_orders = get_tick_orders(state_context.squad_entity, creep_entity, tick_context);

        // Ranged mass attack while retreating (Pipeline B).
        if tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_MASS_ATTACK) {
            let _ = creep.ranged_mass_attack();
        }

        // Heal: prefer healing squad members over self-heal during retreat.
        if let Some(ref orders) = tick_orders {
            if let Some(heal_pos) = orders.heal_target_pos {
                let my_creeps = get_friendly_creeps(creep_pos.room_name(), tick_context);
                let heal_target = my_creeps
                    .iter()
                    .filter(|c| c.pos() == heal_pos || creep_pos.get_range_to(c.pos()) <= 3)
                    .min_by_key(|c| c.pos().get_range_to(heal_pos));

                if let Some(target) = heal_target {
                    let range = creep_pos.get_range_to(target.pos());
                    if range <= 1 && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                        let _ = creep.heal(target);
                    } else if range <= 3 && tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_HEAL) {
                        let _ = creep.ranged_heal(target);
                    }
                } else if creep.hits() < creep.hits_max()
                    && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL)
                {
                    let _ = creep.heal(creep);
                }
            } else if creep.hits() < creep.hits_max()
                && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL)
            {
                let _ = creep.heal(creep);
            }
        } else if creep.hits() < creep.hits_max()
            && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL)
        {
            let _ = creep.heal(creep);
        }

        // Movement: use tick orders for coordinated retreat, fall back to flee.
        if let Some(ref orders) = tick_orders {
            match &orders.movement {
                TickMovement::MoveTo(pos) => {
                    // Squad-coordinated retreat toward rally point.
                    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                        tick_context
                            .runtime_data
                            .movement
                            .move_to(creep_entity, *pos)
                            .range(1)
                            .priority(MovementPriority::High);
                    }
                }
                TickMovement::Flee => {
                    CombatResponse::flee_from_hostiles(tick_context);
                }
                _ => {
                    // Default: flee from hostiles.
                    CombatResponse::flee_from_hostiles(tick_context);
                }
            }
        } else {
            CombatResponse::flee_from_hostiles(tick_context);
        }

        None
    }
}

// ─── SquadCombatJob ─────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Deserialize)]
pub struct SquadCombatJob {
    pub context: SquadCombatJobContext,
    pub state: SquadCombatState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SquadCombatJob {
    pub fn new(target_room: RoomName) -> SquadCombatJob {
        SquadCombatJob {
            context: SquadCombatJobContext {
                target_room,
                squad_entity: None,
                combat_response_start: None,
            },
            state: SquadCombatState::move_to_room(),
        }
    }

    pub fn new_with_squad(target_room: RoomName, squad_entity: Entity) -> SquadCombatJob {
        SquadCombatJob {
            context: SquadCombatJobContext {
                target_room,
                squad_entity: Some(squad_entity.id()),
                combat_response_start: None,
            },
            state: SquadCombatState::move_to_room(),
        }
    }
}

/// Look up the squad state for a job that may or may not be in a squad.
fn get_squad_state(squad_entity_id: Option<u32>, tick_context: &JobTickContext) -> Option<SquadState> {
    let id = squad_entity_id?;
    let entity = tick_context.system_data.entities.entity(id);
    let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
    Some(squad_ctx.state)
}

/// Get cached hostile creeps in the given room from dynamic visibility data.
/// Falls back to game API if room data is not available (e.g. room not in ECS).
fn get_hostile_creeps(room_name: RoomName, tick_context: &JobTickContext) -> Vec<Creep> {
    if let Some(room_entity) = tick_context.runtime_data.mapping.get_room(&room_name) {
        if let Some(room_data) = tick_context.system_data.room_data.get(room_entity) {
            if let Some(creeps) = room_data.get_creeps() {
                return creeps.hostile().to_vec();
            }
        }
    }
    // Fallback: direct game API (room may not be in ECS yet).
    game::rooms()
        .get(room_name)
        .map(|room| room.find(find::HOSTILE_CREEPS, None))
        .unwrap_or_default()
}

/// Get cached friendly creeps in the given room from dynamic visibility data.
fn get_friendly_creeps(room_name: RoomName, tick_context: &JobTickContext) -> Vec<Creep> {
    if let Some(room_entity) = tick_context.runtime_data.mapping.get_room(&room_name) {
        if let Some(room_data) = tick_context.system_data.room_data.get(room_entity) {
            if let Some(creeps) = room_data.get_creeps() {
                return creeps.friendly().to_vec();
            }
        }
    }
    game::rooms()
        .get(room_name)
        .map(|room| room.find(find::MY_CREEPS, None))
        .unwrap_or_default()
}

/// Get cached hostile structures in the given room from dynamic visibility data.
fn get_hostile_structures(room_name: RoomName, tick_context: &JobTickContext) -> Vec<StructureObject> {
    if let Some(room_entity) = tick_context.runtime_data.mapping.get_room(&room_name) {
        if let Some(room_data) = tick_context.system_data.room_data.get(room_entity) {
            if let Some(structures) = room_data.get_structures() {
                return structures
                    .all()
                    .iter()
                    .filter(|s| {
                        s.as_owned()
                            .map(|o| !o.my())
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
            }
        }
    }
    game::rooms()
        .get(room_name)
        .map(|room| room.find(find::HOSTILE_STRUCTURES, None))
        .unwrap_or_default()
}

/// Look up tick orders for a specific creep from the squad context.
fn get_tick_orders(
    squad_entity_id: Option<u32>,
    creep_entity: Entity,
    tick_context: &JobTickContext,
) -> Option<TickOrders> {
    let id = squad_entity_id?;
    let entity = tick_context.system_data.entities.entity(id);
    let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
    let member = squad_ctx.get_member(creep_entity)?;
    member.tick_orders.clone()
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for SquadCombatJob {
    fn summarize(&self) -> SummaryContent {
        SummaryContent::Text(format!("SquadCombat - {}", self.state.status_description()))
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

        crate::machine_tick::run_state_machine(&mut self.state, "SquadCombatJob", |state| {
            state.tick(&mut self.context, &mut tick_context)
        });
    }
}
