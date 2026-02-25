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

// ─── Body part detection helpers ────────────────────────────────────────────

fn has_active_part(creep: &Creep, part: Part) -> bool {
    creep.body().iter().any(|p| p.part() == part && p.hits() > 0)
}

// ─── MoveToRoom ─────────────────────────────────────────────────────────────

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut SquadCombatJobContext, tick_context: &mut JobTickContext) -> Option<SquadCombatState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();
        let creep_entity = tick_context.runtime_data.creep_entity;

        // Check for hostiles in the current room -- respond to ambush.
        if creep_pos.room_name() != state_context.target_room {
            let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);
            let nearby_threats = hostiles.iter().any(|c| creep_pos.get_range_to(c.pos()) <= 5);

            if nearby_threats {
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

        // Check for squad formation movement orders (keeps squad grouped during travel).
        let tick_orders = get_tick_orders(state_context.squad_entity, creep_entity, tick_context);

        if let Some(ref orders) = tick_orders {
            if matches!(orders.movement, TickMovement::Formation) {
                if let Some(target_tile) = get_formation_target(
                    state_context.squad_entity,
                    creep_entity,
                    tick_context,
                    creep_pos,
                ) {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, target_tile)
                        .range(0)
                        .priority(MovementPriority::High);

                    // Only transition to Engaged when the squad itself has
                    // progressed past Rallying. This prevents individual
                    // creeps from engaging while the rest of the squad is
                    // still gathering at a room boundary.
                    if creep_pos.room_name() == state_context.target_room {
                        let squad_ready = get_squad_state(state_context.squad_entity, tick_context)
                            .map(|s| s >= SquadState::Moving)
                            .unwrap_or(true);
                        if squad_ready {
                            return Some(SquadCombatState::engaged());
                        }
                    }
                    return None;
                }
            }
        }

        // No squad formation orders -- use standard room navigation.
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
        let threats_nearby = hostiles.iter().any(|c| creep_pos.get_range_to(c.pos()) <= 6);

        let timed_out = state_context
            .combat_response_start
            .map(|start| game::time() - start > COMBAT_RESPONSE_TIMEOUT)
            .unwrap_or(false);

        if !threats_nearby || timed_out {
            state_context.combat_response_start = None;
            return Some(SquadCombatState::move_to_room());
        }

        // Fight back with all applicable body parts.
        let tick_orders = get_tick_orders(state_context.squad_entity, creep_entity, tick_context);
        let focus_creep: Option<Creep> = tick_orders
            .as_ref()
            .and_then(|o| o.attack_target.as_ref())
            .and_then(|t| t.resolve_creep());

        // Pipeline A: Melee attack adjacent hostile (prefer focus target).
        if has_active_part(creep, Part::Attack) {
            let target = if let Some(ref focus) = focus_creep {
                if creep_pos.get_range_to(focus.pos()) <= 1 {
                    Some(focus)
                } else {
                    hostiles
                        .iter()
                        .filter(|c| creep_pos.get_range_to(c.pos()) <= 1)
                        .min_by_key(|c| c.hits())
                }
            } else {
                hostiles
                    .iter()
                    .filter(|c| creep_pos.get_range_to(c.pos()) <= 1)
                    .min_by_key(|c| c.hits())
            };
            if let Some(target) = target {
                let _ = creep.attack(target);
            }
        }

        // Pipeline B: Ranged attack (prefer focus target).
        if has_active_part(creep, Part::RangedAttack) {
            let in_range_3_count = hostiles
                .iter()
                .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                .count();
            let in_range_1_count = hostiles
                .iter()
                .filter(|c| creep_pos.get_range_to(c.pos()) <= 1)
                .count();

            if in_range_1_count >= 3 || (in_range_3_count >= 3 && in_range_1_count >= 1) {
                let _ = creep.ranged_mass_attack();
            } else {
                let target = if let Some(ref focus) = focus_creep {
                    if creep_pos.get_range_to(focus.pos()) <= 3 {
                        Some(focus)
                    } else {
                        hostiles
                            .iter()
                            .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                            .min_by_key(|c| c.hits())
                    }
                } else {
                    hostiles
                        .iter()
                        .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                        .min_by_key(|c| c.hits())
                };
                if let Some(target) = target {
                    let _ = creep.ranged_attack(target);
                }
            }
        }

        // Pipeline C: Heal -- resolve assigned target by ID, else best nearby.
        if has_active_part(creep, Part::Heal) {
            let heal_target = tick_orders
                .as_ref()
                .and_then(|o| o.heal_target)
                .and_then(|id| id.resolve());
            if let Some(target) = heal_target {
                let range = creep_pos.get_range_to(target.pos());
                if range <= 1 {
                    let _ = creep.heal(&target);
                } else if range <= 3 {
                    let _ = creep.ranged_heal(&target);
                } else {
                    heal_best_nearby(creep, tick_context);
                }
            } else {
                heal_best_nearby(creep, tick_context);
            }
        }

        // Movement: follow tick orders or kite toward objective.
        if let Some(ref orders) = tick_orders {
            match &orders.movement {
                TickMovement::Flee => {
                    flee_from_hostiles(tick_context);
                }
                TickMovement::Formation | TickMovement::Hold => {
                    Self::kite_toward_objective(tick_context, state_context);
                }
                TickMovement::MoveTo(pos) => {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, *pos)
                        .range(1)
                        .priority(MovementPriority::High);
                }
            }
        } else {
            Self::kite_toward_objective(tick_context, state_context);
        }

        None
    }

    fn kite_toward_objective(tick_context: &mut JobTickContext, state_context: &SquadCombatJobContext) {
        let creep_entity = tick_context.runtime_data.creep_entity;
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

        // Retreat if HP drops below 50%.
        if creep.hits() < creep.hits_max() / 2 {
            return Some(SquadCombatState::retreating());
        }

        // If we've left the target room, move back.
        if creep_pos.room_name() != state_context.target_room {
            return Some(SquadCombatState::move_to_room());
        }

        // ── Execute actions (all pipelines fire independently) ──

        if let Some(ref orders) = tick_orders {
            // With tick orders: use ordered targets.
            Self::execute_attack_with_orders(creep, creep_pos, orders, tick_context);
            Self::execute_heal_with_orders(creep, creep_pos, orders, tick_context);
        } else {
            // No tick orders: body-part-aware fallback.
            Self::fallback_attack(creep, creep_pos, tick_context);
            Self::fallback_heal(creep, tick_context);
        }

        // ── Movement ──

        if let Some(ref orders) = tick_orders {
            match &orders.movement {
                TickMovement::Formation => {
                    execute_formation_movement(state_context, creep_entity, orders, tick_context);
                }
                TickMovement::MoveTo(pos) => {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, *pos)
                        .range(1)
                        .priority(MovementPriority::High);
                }
                TickMovement::Flee => {
                    flee_from_hostiles(tick_context);
                }
                TickMovement::Hold => {}
            }
        } else {
            Self::fallback_movement(creep, creep_pos, creep_entity, tick_context, state_context);
        }

        None
    }

    // ── Ordered attack ──

    fn execute_attack_with_orders(
        creep: &Creep,
        creep_pos: Position,
        orders: &TickOrders,
        tick_context: &mut JobTickContext,
    ) {
        let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);

        // Resolve the focus target from the AttackTarget enum.
        let focus_creep: Option<Creep> = orders
            .attack_target
            .as_ref()
            .and_then(|t| t.resolve_creep());

        // Pipeline A: Melee attack adjacent hostile -- prefer focus target.
        if has_active_part(creep, Part::Attack) {
            let target = if let Some(ref focus) = focus_creep {
                if creep_pos.get_range_to(focus.pos()) <= 1 {
                    Some(focus)
                } else {
                    hostiles
                        .iter()
                        .filter(|c| creep_pos.get_range_to(c.pos()) <= 1)
                        .min_by_key(|c| c.hits())
                }
            } else {
                hostiles
                    .iter()
                    .filter(|c| creep_pos.get_range_to(c.pos()) <= 1)
                    .min_by_key(|c| c.hits())
            };
            if let Some(target) = target {
                let _ = creep.attack(target);
            }
        }

        // Pipeline B: Ranged attack -- focus fire the squad's designated target.
        if has_active_part(creep, Part::RangedAttack) {
            let in_range_3_count = hostiles
                .iter()
                .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                .count();
            let in_range_1_count = hostiles
                .iter()
                .filter(|c| creep_pos.get_range_to(c.pos()) <= 1)
                .count();

            if in_range_3_count > 0 {
                // Mass attack when multiple hostiles are stacked on us.
                if in_range_1_count >= 3 || (in_range_3_count >= 3 && in_range_1_count >= 1) {
                    let _ = creep.ranged_mass_attack();
                } else {
                    // Focus fire: prefer the exact focus target by ID.
                    let target = if let Some(ref focus) = focus_creep {
                        if creep_pos.get_range_to(focus.pos()) <= 3 {
                            Some(focus)
                        } else {
                            hostiles
                                .iter()
                                .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                                .min_by_key(|c| c.hits())
                        }
                    } else {
                        hostiles
                            .iter()
                            .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                            .min_by_key(|c| c.hits())
                    };
                    if let Some(target) = target {
                        let _ = creep.ranged_attack(target);
                    }
                }
            } else {
                // No hostiles in range -- try structures.
                let structures = get_hostile_structures(creep_pos.room_name(), tick_context);
                let target = structures
                    .iter()
                    .filter(|s| creep_pos.get_range_to(s.pos()) <= 3)
                    .min_by_key(|s| match s.structure_type() {
                        StructureType::InvaderCore => 0u32,
                        StructureType::Spawn => 1,
                        StructureType::Tower => 2,
                        _ => 10,
                    });
                if let Some(target) = target {
                    if let Some(attackable) = target.as_attackable() {
                        let _ = creep.ranged_attack(attackable);
                    }
                }
            }
        }
    }

    // ── Ordered heal ──

    fn execute_heal_with_orders(
        creep: &Creep,
        creep_pos: Position,
        orders: &TickOrders,
        tick_context: &mut JobTickContext,
    ) {
        if !has_active_part(creep, Part::Heal) {
            return;
        }

        // Resolve the heal target directly by ObjectId.
        if let Some(target) = orders.heal_target.and_then(|id| id.resolve()) {
            let range = creep_pos.get_range_to(target.pos());
            if range <= 1 {
                let _ = creep.heal(&target);
            } else if range <= 3 {
                let _ = creep.ranged_heal(&target);
            } else {
                // Assigned target out of range -- heal best nearby instead.
                heal_best_nearby(creep, tick_context);
            }
        } else {
            // No target or target died -- heal best nearby.
            heal_best_nearby(creep, tick_context);
        }
    }

    // ── Fallback attack (no tick orders, body-part-aware) ──

    fn fallback_attack(creep: &Creep, creep_pos: Position, tick_context: &mut JobTickContext) {
        let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);

        if hostiles.is_empty() {
            // Attack structures: prioritize invader cores > spawns > towers.
            if has_active_part(creep, Part::RangedAttack) {
                let structures = get_hostile_structures(creep_pos.room_name(), tick_context);
                let target = structures
                    .iter()
                    .filter(|s| creep_pos.get_range_to(s.pos()) <= 3)
                    .min_by_key(|s| match s.structure_type() {
                        StructureType::InvaderCore => 0u32,
                        StructureType::Spawn => 1,
                        StructureType::Tower => 2,
                        _ => 10,
                    });
                if let Some(target) = target {
                    if let Some(attackable) = target.as_attackable() {
                        let _ = creep.ranged_attack(attackable);
                    }
                }
            }
            if has_active_part(creep, Part::Attack) {
                let structures = get_hostile_structures(creep_pos.room_name(), tick_context);
                let target = structures
                    .iter()
                    .filter(|s| creep_pos.get_range_to(s.pos()) <= 1)
                    .min_by_key(|s| match s.structure_type() {
                        StructureType::InvaderCore => 0u32,
                        StructureType::Spawn => 1,
                        StructureType::Tower => 2,
                        _ => 10,
                    });
                if let Some(target) = target {
                    if let Some(attackable) = target.as_attackable() {
                        let _ = creep.attack(attackable);
                    }
                }
            }
            return;
        }

        // Pipeline A: Melee attack adjacent hostile.
        if has_active_part(creep, Part::Attack) {
            if let Some(target) = hostiles
                .iter()
                .filter(|c| creep_pos.get_range_to(c.pos()) <= 1)
                .min_by_key(|c| c.hits())
            {
                let _ = creep.attack(target);
            }
        }

        // Pipeline B: Ranged attack.
        if has_active_part(creep, Part::RangedAttack) {
            let in_range_1: usize = hostiles.iter().filter(|c| creep_pos.get_range_to(c.pos()) <= 1).count();

            if in_range_1 >= 3 {
                let _ = creep.ranged_mass_attack();
            } else {
                let target = hostiles
                    .iter()
                    .filter(|c| creep_pos.get_range_to(c.pos()) <= 3)
                    .min_by_key(|c| c.hits());

                if let Some(target) = target {
                    let _ = creep.ranged_attack(target);
                }
            }
        }
    }

    // ── Fallback heal (no tick orders) ──

    fn fallback_heal(creep: &Creep, tick_context: &mut JobTickContext) {
        if !has_active_part(creep, Part::Heal) {
            return;
        }
        heal_best_nearby(creep, tick_context);
    }

    // ── Fallback movement (no tick orders, body-part-aware) ──

    fn fallback_movement(
        creep: &Creep,
        creep_pos: Position,
        creep_entity: Entity,
        tick_context: &mut JobTickContext,
        state_context: &SquadCombatJobContext,
    ) {
        let has_attack = has_active_part(creep, Part::Attack);
        let has_ranged = has_active_part(creep, Part::RangedAttack);
        let has_heal = has_active_part(creep, Part::Heal);

        let hostiles = get_hostile_creeps(state_context.target_room, tick_context);

        if has_attack && !has_ranged {
            // Pure melee: close to range 1 aggressively.
            if let Some(target) = hostiles.iter().min_by_key(|c| creep_pos.get_range_to(c.pos())) {
                let range = creep_pos.get_range_to(target.pos());
                if range > 1 {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, target.pos())
                        .range(1)
                        .priority(MovementPriority::High);
                } else {
                    mark_working(tick_context, target.pos(), 1);
                }
            }
        } else if has_ranged {
            // Ranged (with or without melee): kite at range 3.
            if let Some(target) = hostiles.iter().min_by_key(|c| creep_pos.get_range_to(c.pos())) {
                let range = creep_pos.get_range_to(target.pos());

                let is_melee_only = target.body().iter().any(|p| p.part() == Part::Attack && p.hits() > 0)
                    && !target.body().iter().any(|p| p.part() == Part::RangedAttack && p.hits() > 0);

                if is_melee_only && range <= 2 {
                    flee_from_hostiles(tick_context);
                } else if range > 3 {
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
        } else if has_heal && !hostiles.is_empty() {
            // Pure healer: follow nearest damaged friendly.
            let my_creeps = get_friendly_creeps(creep_pos.room_name(), tick_context);
            let damaged = my_creeps
                .iter()
                .filter(|c| c.hits() < c.hits_max() && c.pos() != creep_pos)
                .min_by_key(|c| creep_pos.get_range_to(c.pos()));

            if let Some(target) = damaged {
                let range = creep_pos.get_range_to(target.pos());
                if range > 1 {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, target.pos())
                        .range(1)
                        .priority(MovementPriority::High);
                }
            }
        }
        // Hauler / no combat parts: idle.
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
        if creep.hits() > creep.hits_max() * 4 / 5 || (squad_wants_engage && creep.hits() > creep.hits_max() * 3 / 5) {
            return Some(SquadCombatState::engaged());
        }

        // Read tick orders for coordinated retreat.
        let tick_orders = get_tick_orders(state_context.squad_entity, creep_entity, tick_context);

        // Pipeline B: Ranged mass attack while retreating.
        if has_active_part(creep, Part::RangedAttack) {
            let _ = creep.ranged_mass_attack();
        }

        // Pipeline A: Melee attack if adjacent.
        if has_active_part(creep, Part::Attack) {
            let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);
            if let Some(target) = hostiles.iter().find(|c| creep_pos.get_range_to(c.pos()) <= 1) {
                let _ = creep.attack(target);
            }
        }

        // Pipeline C: Heal -- resolve assigned target by ID, else best nearby.
        if has_active_part(creep, Part::Heal) {
            let heal_target = tick_orders
                .as_ref()
                .and_then(|o| o.heal_target)
                .and_then(|id| id.resolve());
            if let Some(target) = heal_target {
                let range = creep_pos.get_range_to(target.pos());
                if range <= 1 {
                    let _ = creep.heal(&target);
                } else if range <= 3 {
                    let _ = creep.ranged_heal(&target);
                } else {
                    heal_best_nearby(creep, tick_context);
                }
            } else {
                heal_best_nearby(creep, tick_context);
            }
        }

        // Movement: use tick orders for coordinated retreat, fall back to flee.
        if let Some(ref orders) = tick_orders {
            match &orders.movement {
                TickMovement::MoveTo(pos) => {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, *pos)
                        .range(1)
                        .priority(MovementPriority::High);
                }
                TickMovement::Flee => {
                    flee_from_hostiles(tick_context);
                }
                _ => {
                    flee_from_hostiles(tick_context);
                }
            }
        } else {
            flee_from_hostiles(tick_context);
        }

        None
    }
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Heal the best nearby target: prefer adjacent damaged squad member, then
/// adjacent damaged friendly, then self-heal, then ranged heal.
fn heal_best_nearby(creep: &Creep, tick_context: &mut JobTickContext) {
    let creep_pos = creep.pos();
    let my_creeps = get_friendly_creeps(creep_pos.room_name(), tick_context);

    // Prefer adjacent damaged friendlies (12 HP/part).
    let adjacent_damaged = my_creeps
        .iter()
        .filter(|c| creep_pos.get_range_to(c.pos()) <= 1 && c.hits() < c.hits_max())
        .min_by_key(|c| c.hits());

    if let Some(target) = adjacent_damaged {
        let _ = creep.heal(target);
        return;
    }

    // Self-heal if damaged.
    if creep.hits() < creep.hits_max() {
        let _ = creep.heal(creep);
        return;
    }

    // Ranged heal damaged friendlies (4 HP/part).
    let ranged_damaged = my_creeps
        .iter()
        .filter(|c| {
            let range = creep_pos.get_range_to(c.pos());
            range > 1 && range <= 3 && c.hits() < c.hits_max()
        })
        .min_by_key(|c| c.hits());

    if let Some(target) = ranged_damaged {
        let _ = creep.ranged_heal(target);
    }
}

/// Flee from nearby hostiles.
fn flee_from_hostiles(tick_context: &mut JobTickContext) {
    let creep = tick_context.runtime_data.owner;
    let creep_entity = tick_context.runtime_data.creep_entity;

    let hostiles = get_hostile_creeps(creep.pos().room_name(), tick_context);
    let flee_targets: Vec<FleeTarget> = hostiles.iter().map(|c| FleeTarget { pos: c.pos(), range: 8 }).collect();

    if !flee_targets.is_empty() {
        tick_context.runtime_data.movement.flee(creep_entity, flee_targets).range(8);
    }
}

/// Execute formation movement: move toward the virtual anchor offset tile.
fn execute_formation_movement(
    state_context: &SquadCombatJobContext,
    creep_entity: Entity,
    orders: &TickOrders,
    tick_context: &mut JobTickContext,
) {
    let creep_pos = tick_context.runtime_data.owner.pos();
    let moved = (|| {
        let target_tile = get_formation_target(
            state_context.squad_entity,
            creep_entity,
            tick_context,
            creep_pos,
        )?;
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
        if let Some(target_pos) = orders.attack_target.as_ref().and_then(|t| t.pos()) {
            tick_context
                .runtime_data
                .movement
                .move_to(creep_entity, target_pos)
                .range(1)
                .priority(MovementPriority::High);
        }
    }
}

/// Get the formation target tile for a specific creep from the squad context.
///
/// If the virtual position is in a different room from the creep, returns
/// a position on the nearest room edge toward the virtual position so the
/// creep moves to rejoin the formation rather than wandering independently.
///
/// Uses `creep_pos_fallback` when `member.position` is None (e.g. first tick
/// after spawn or before PreRunSquadUpdate has synced this member's position)
/// so that all squad members get a valid formation target and move together.
fn get_formation_target(
    squad_entity_id: Option<u32>,
    creep_entity: Entity,
    tick_context: &JobTickContext,
    creep_pos_fallback: Position,
) -> Option<Position> {
    let id = squad_entity_id?;
    let entity = tick_context.system_data.entities.entity(id);
    let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
    let member = squad_ctx.get_member(creep_entity)?;
    let virtual_pos = squad_ctx.squad_path.as_ref().map(|p| p.virtual_pos)?;
    let layout = squad_ctx.layout.as_ref()?;
    let target = virtual_anchor_target(virtual_pos, layout, member.formation_slot)?;

    // Prefer cached position; use live creep position when not yet synced (e.g. second of duo).
    let creep_pos = member.position.unwrap_or(creep_pos_fallback);
    if creep_pos.room_name() == target.room_name() {
        return Some(target);
    }

    // The target is in a different room. Guide the creep toward the room
    // exit that leads to the target's room. Using world coordinates gives
    // the correct direction even across room boundaries.
    let (cur_wx, cur_wy) = creep_pos.world_coords();
    let (tgt_wx, tgt_wy) = target.world_coords();
    let dx = (tgt_wx - cur_wx).signum();
    let dy = (tgt_wy - cur_wy).signum();

    // Compute a position on the current room's edge in the right direction.
    let edge_x = if dx > 0 {
        49
    } else if dx < 0 {
        0
    } else {
        creep_pos.x().u8()
    };
    let edge_y = if dy > 0 {
        49
    } else if dy < 0 {
        0
    } else {
        creep_pos.y().u8()
    };

    Some(Position::new(
        RoomCoordinate::new(edge_x).ok()?,
        RoomCoordinate::new(edge_y).ok()?,
        creep_pos.room_name(),
    ))
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
                    .filter(|s| s.as_owned().map(|o| !o.my()).unwrap_or(false))
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
fn get_tick_orders(squad_entity_id: Option<u32>, creep_entity: Entity, tick_context: &JobTickContext) -> Option<TickOrders> {
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
            action_flags: super::actions::SimultaneousActionFlags::UNSET,
        };

        crate::machine_tick::run_state_machine(&mut self.state, "SquadCombatJob", |state| {
            state.tick(&mut self.context, &mut tick_context)
        });
    }
}
