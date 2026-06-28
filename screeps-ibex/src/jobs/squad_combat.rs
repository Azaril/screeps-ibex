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
    pub(crate) squad_entity: Option<SquadRef>,
    /// Tick when we entered the combat response state (for timeout).
    #[serde(default)]
    combat_response_start: Option<u32>,
}

/// Maximum ticks to spend in combat response before resuming objective.
const COMBAT_RESPONSE_TIMEOUT: u32 = 50;

// The pure transition table of this state machine is mirrored in `screeps_combat_decision::squad_fsm`
// (`next_state`, K2 / ADR 0028) — the canonical, unit-tested spec the offline lifecycle harness drives.
// The `return Some(state)` decisions in each `*::tick` below MUST stay in step with that kernel; the ECS
// actions (movement, combat, the orphan recall-to-recycle) stay here. (Full bot adoption of `next_state`
// is deferred: each tick interleaves its transition checks with movement, so calling `next_state`
// up-front would move the post-movement arrival-engage — see ADR 0028 §K2.)
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

// ─── Tactical-seam adapters (game::* → JS-free DTOs) ─────────────────────────
//
// The single place the live combat path reads a `Creep` / `StructureObject` into the seam DTOs
// (`combat::*`); shared by the `squad_combat` per-creep decision and the `attack_mission` focus
// selection so both build the view identically. No `game::*` lives below this seam (ADR 0006 §B.2).

pub(crate) fn creep_to_dto(c: &Creep) -> crate::combat::CombatCreepDto {
    crate::combat::CombatCreepDto {
        id: c.try_raw_id(),
        pos: c.pos(),
        hits: c.hits(),
        hits_max: c.hits_max(),
        body: c
            .body()
            .iter()
            .map(|p| crate::combat::CombatBodyPart { part: p.part(), hits: p.hits() })
            .collect(),
    }
}

pub(crate) fn structure_to_dto(s: &StructureObject) -> crate::combat::CombatStructureDto {
    use crate::combat::Ownership;
    let ownership = match s.as_owned() {
        Some(o) if o.my() => Ownership::Mine,
        Some(_) => Ownership::Hostile,
        None => Ownership::Neutral,
    };
    let (hits, hits_max) = s.as_attackable().map(|a| (a.hits(), a.hits_max())).unwrap_or((0, 0));
    // Tower stored energy: a tower with < TOWER_ENERGY_COST can't fire or heal, so the decision must
    // exclude a drained tower from the threat field + the max-heal estimate (ADR 0020).
    let energy = match s {
        StructureObject::StructureTower(t) => t.store().get_used_capacity(Some(screeps::constants::ResourceType::Energy)),
        _ => 0,
    };
    crate::combat::CombatStructureDto {
        pos: s.pos(),
        structure_type: s.structure_type(),
        hits,
        hits_max,
        ownership,
        energy,
    }
}

// ─── MoveToRoom ─────────────────────────────────────────────────────────────

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut SquadCombatJobContext, tick_context: &mut JobTickContext) -> Option<SquadCombatState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();
        let creep_entity = tick_context.runtime_data.creep_entity;

        // P-OBJ #23 zero-orphan recall (travel path): if this squad was retired while we were en route
        // (its objective resolved/given-up), DON'T trek on to the now-abandoned objective room — recall to
        // the nearest home spawn and recycle, so an orphan is never stranded mid-travel on a room edge.
        if state_context.squad_entity.is_some() && get_squad_state(state_context.squad_entity, tick_context).is_none() {
            let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);
            if hostiles.is_empty() {
                Engaged::recall_to_recycle(creep, creep_pos, creep_entity, tick_context);
                return None;
            }
        }

        // ADR 0027 v1.1 P2: a DECLAIM squad member that has reached the target room transitions to Engaged
        // (which runs the declaim drive — move-to-controller + strike). A declaimer carries no combat parts,
        // so it does not need the formation-assault path; reaching the room is enough to start striking.
        if squad_attack_controller_pos(state_context.squad_entity, tick_context).is_some()
            && creep_pos.room_name() == state_context.target_room
        {
            return Some(SquadCombatState::engaged());
        }

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

        // MOVEMENT-STALL FIX (ADR 0028 K0): SOLO travel to the shared rally. The manager stamps a
        // `MoveTo(rally)` order during the travel-to-rally phase (no formation anchor) — each member paths
        // INDIVIDUALLY to the shared staging point (sidestepping the frozen cross-room formation anchor).
        // Once the gather quorum fires the manager switches to a formation anchor + `Formation` orders
        // (handled below). A member that has reached the rally still holds here (range 1) until then.
        if let Some(ref orders) = tick_orders {
            match orders.movement {
                TickMovement::MoveTo(rally) => {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, rally)
                        .range(1)
                        .priority(MovementPriority::High);
                    return None;
                }
                // HOLD (rally/forming phase): the rally gate has not released — hold at home next to the
                // spawn (renewable) instead of marching solo to the target room. No movement this tick.
                TickMovement::Hold => {
                    return None;
                }
                _ => {}
            }
        }

        if let Some(ref orders) = tick_orders {
            if matches!(orders.movement, TickMovement::Formation) {
                if let Some(target_tile) = get_formation_target(state_context.squad_entity, creep_entity, tick_context, creep_pos) {
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
            .map(|start| game::time().saturating_sub(start) > COMBAT_RESPONSE_TIMEOUT)
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
                crate::intents::attack(
                    creep,
                    &mut tick_context.action_flags,
                    tick_context.runtime_data.intent_recorder,
                    target,
                    target.pos(),
                );
            }
        }

        // Pipeline B: Ranged attack (prefer focus target).
        if has_active_part(creep, Part::RangedAttack) {
            let in_range_3_count = hostiles.iter().filter(|c| creep_pos.get_range_to(c.pos()) <= 3).count();
            let in_range_1_count = hostiles.iter().filter(|c| creep_pos.get_range_to(c.pos()) <= 1).count();

            if in_range_1_count >= 3 || (in_range_3_count >= 3 && in_range_1_count >= 1) {
                crate::intents::ranged_mass_attack(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder);
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
                    crate::intents::ranged_attack(
                        creep,
                        &mut tick_context.action_flags,
                        tick_context.runtime_data.intent_recorder,
                        target,
                        target.pos(),
                    );
                }
            }
        }

        // Pipeline C: Heal -- resolve assigned target by ID, else best nearby.
        if has_active_part(creep, Part::Heal) {
            let heal_target = tick_orders.as_ref().and_then(|o| o.heal_target).and_then(|id| id.resolve());
            if let Some(target) = heal_target {
                let range = creep_pos.get_range_to(target.pos());
                if range <= 1 {
                    crate::intents::heal(
                        creep,
                        &mut tick_context.action_flags,
                        tick_context.runtime_data.intent_recorder,
                        &target,
                        target.pos(),
                    );
                } else if range <= 3 {
                    crate::intents::ranged_heal(
                        creep,
                        &mut tick_context.action_flags,
                        tick_context.runtime_data.intent_recorder,
                        &target,
                        target.pos(),
                    );
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

        // ── ADR 0027 v1.1 P2: DECLAIM drive ──
        // A declaim squad's in-room job is NOT to fight (the room is derelict/quiet by construction) but to
        // `attackController` the controller on the 1000-tick upgrade-block cadence. A CLAIM declaimer carries
        // no combat parts, so it skips the combat pipeline entirely; it moves adjacent to the controller and
        // strikes when the block clears (else HOLDS adjacent — the manager's lease keeps it committed across
        // the cadence; see `declaiming` in squad_manager). Inert for every combat squad (returns early only
        // when the squad target is `AttackController`).
        if let Some(controller_pos) = squad_attack_controller_pos(state_context.squad_entity, tick_context) {
            drive_declaim(controller_pos, tick_context);
            return None;
        }

        // ── Execute actions (all pipelines fire independently) ──

        // Attack + heal through the tactical seam (`combat::decide_combat`) — the single shared
        // implementation that also drives the sim, so live and sim cannot diverge (ADR 0006 §B.2,
        // P2.H2). Movement stays below (it rides P2.M2).
        Self::execute_combat_via_seam(creep, creep_pos, tick_orders.as_ref(), tick_context);

        // ── Movement ──

        if let Some(ref orders) = tick_orders {
            match &orders.movement {
                TickMovement::Formation => {
                    // A squad with a cached anchor path (manager-fielded siege/Formation, O1)
                    // follows it. An anchorless manager squad (P2.G3-tail) routes movement through
                    // the pure `decide_movement` using the squad's shared directive (the cohesive,
                    // pathfinding-scored kite/advance goal the manager stamped on the orders) — the job
                    // issues the request (§5 ⚑ job-owns-movement). decide_movement's own precedence
                    // (critical-HP flee, immediate melee-evade, cohesion rejoin) keeps the block together.
                    if squad_has_anchor(state_context.squad_entity, tick_context) {
                        execute_formation_movement(state_context, creep_entity, orders, tick_context);
                    } else {
                        Self::execute_decide_movement(creep, creep_pos, orders, tick_context);
                    }
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

    // ── Combat via the tactical seam (P2.H2, ADR 0006 §B.2) ──
    //
    // Build the per-creep `CombatView` from `game::*` (the live adapter leaf — the only place this
    // path touches the game), run the shared decision `combat::decide_combat` (the SAME code the
    // sim runs — no fork), then translate the returned intents back through the guarded sink. This
    // replaces the old inline `execute_*_with_orders` / `fallback_*` (attack + heal). Movement is
    // handled separately below and rides P2.M2.
    fn execute_combat_via_seam(creep: &Creep, creep_pos: Position, tick_orders: Option<&TickOrders>, tick_context: &mut JobTickContext) {
        use crate::combat::{decide_combat, CombatView, CreepOrders, FocusTarget, SquadMovement, SquadStateDto};

        let room = creep_pos.room_name();
        let hostiles_raw = get_hostile_creeps(room, tick_context);
        let friends_raw = get_friendly_creeps(room, tick_context);
        let structures_raw = get_hostile_structures(room, tick_context);

        let me_dto = creep_to_dto(creep);
        let hostiles: Vec<_> = hostiles_raw.iter().map(creep_to_dto).collect();
        let friends: Vec<_> = friends_raw.iter().map(creep_to_dto).collect();
        let structures: Vec<_> = structures_raw.iter().map(structure_to_dto).collect();

        let orders = tick_orders.map(|o| CreepOrders {
            // The resolved focus *creep* (`resolve_creep()` is `None` for structure targets, which
            // are scanned per-creep) and the resolved heal target — mirroring the prior logic.
            focus: o.attack_target.and_then(|t| t.resolve_creep()).map(|c| FocusTarget { pos: c.pos(), id: c.try_raw_id() }),
            heal_target: o.heal_target.and_then(|id| id.resolve()).map(|c| FocusTarget { pos: c.pos(), id: c.try_raw_id() }),
        });

        // `decide_combat` (attack/heal) reads only `center`/`room`; the movement directive +
        // cohesion radius are for `decide_movement` (wired live in P2.G3-tail Step 6).
        let squad = SquadStateDto { center: creep_pos, room, movement: SquadMovement::Hold, cohesion_radius: 0 };
        let intents = {
            let view = CombatView {
                tick: game::time(),
                me: &me_dto,
                squad: &squad,
                orders,
                friends: &friends,
                hostiles: &hostiles,
                structures: &structures,
            };
            decide_combat(&view)
        };

        Self::translate_intents(creep, &intents, &structures_raw, tick_context);
    }

    /// Re-emit the seam's combat intents through the guarded sink, in their emitted (pipeline)
    /// order, so the `IntentRecorder` digest is identical to the prior inline logic. Creep targets
    /// resolve by id (the live `resolve()`); structure targets resolve by position within the
    /// hostile-structure list. Movement / `Idle` / `Dismantle` intents are no-ops here.
    fn translate_intents(
        creep: &Creep,
        intents: &[crate::combat::CombatIntent],
        structures: &[StructureObject],
        tick_context: &mut JobTickContext,
    ) {
        use crate::combat::CombatIntent;
        let struct_at = |pos: Position| structures.iter().find(|s| s.pos() == pos);
        for intent in intents {
            match intent {
                CombatIntent::Attack { target, id } => {
                    if let Some(raw) = id {
                        if let Some(c) = ObjectId::<Creep>::from(*raw).resolve() {
                            crate::intents::attack(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder, &c, *target);
                        }
                    } else if let Some(a) = struct_at(*target).and_then(|s| s.as_attackable()) {
                        crate::intents::attack(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder, a, *target);
                    }
                }
                CombatIntent::RangedAttack { target, id } => {
                    if let Some(raw) = id {
                        if let Some(c) = ObjectId::<Creep>::from(*raw).resolve() {
                            crate::intents::ranged_attack(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder, &c, *target);
                        }
                    } else if let Some(a) = struct_at(*target).and_then(|s| s.as_attackable()) {
                        crate::intents::ranged_attack(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder, a, *target);
                    }
                }
                CombatIntent::RangedMassAttack => {
                    crate::intents::ranged_mass_attack(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder);
                }
                CombatIntent::Heal { target, id } => {
                    if let Some(raw) = id {
                        if let Some(c) = ObjectId::<Creep>::from(*raw).resolve() {
                            crate::intents::heal(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder, &c, *target);
                        }
                    }
                }
                CombatIntent::RangedHeal { target, id } => {
                    if let Some(raw) = id {
                        if let Some(c) = ObjectId::<Creep>::from(*raw).resolve() {
                            crate::intents::ranged_heal(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder, &c, *target);
                        }
                    }
                }
                CombatIntent::Dismantle { .. } | CombatIntent::MoveTo { .. } | CombatIntent::Flee { .. } | CombatIntent::Idle => {}
            }
        }
    }

    // ── Squad-cohesive movement via the tactical seam (P2.G3-tail) ──
    //
    // The live adapter for the pure `decide_movement`: build the per-creep `CombatView` from
    // `game::*` + the squad's shared directive (the manager stamped `squad_movement`/`squad_center`/
    // `squad_cohesion_radius` on the orders), run `decide_movement` (the SAME code the sim runs —
    // cohesive kiting/advance with the critical-HP + melee-evade + rejoin precedence), and translate
    // its single movement goal into a rover request. Used for anchorless manager squads (the SK duo,
    // defense); squads with a cached anchor path (siege/Formation) keep the anchor mover.
    fn execute_decide_movement(creep: &Creep, creep_pos: Position, orders: &TickOrders, tick_context: &mut JobTickContext) {
        use crate::combat::{decide_movement, CombatIntent, CombatView, CreepOrders, FocusTarget, SquadStateDto};

        let room = creep_pos.room_name();
        let hostiles_raw = get_hostile_creeps(room, tick_context);
        let friends_raw = get_friendly_creeps(room, tick_context);
        let structures_raw = get_hostile_structures(room, tick_context);

        let me_dto = creep_to_dto(creep);
        let hostiles: Vec<_> = hostiles_raw.iter().map(creep_to_dto).collect();
        let friends: Vec<_> = friends_raw.iter().map(creep_to_dto).collect();
        let structures: Vec<_> = structures_raw.iter().map(structure_to_dto).collect();

        let creep_orders = CreepOrders {
            focus: orders.attack_target.and_then(|t| t.resolve_creep()).map(|c| FocusTarget { pos: c.pos(), id: c.try_raw_id() }),
            heal_target: orders.heal_target.and_then(|id| id.resolve()).map(|c| FocusTarget { pos: c.pos(), id: c.try_raw_id() }),
        };
        let squad = SquadStateDto {
            center: orders.squad_center.unwrap_or(creep_pos),
            room,
            movement: orders.squad_movement,
            cohesion_radius: orders.squad_cohesion_radius,
        };
        let intents = {
            let view = CombatView {
                tick: game::time(),
                me: &me_dto,
                squad: &squad,
                orders: Some(creep_orders),
                friends: &friends,
                hostiles: &hostiles,
                structures: &structures,
            };
            decide_movement(&view)
        };

        let creep_entity = tick_context.runtime_data.creep_entity;
        for intent in intents {
            match intent {
                CombatIntent::MoveTo { target, range } => {
                    tick_context
                        .runtime_data
                        .movement
                        .move_to(creep_entity, target)
                        .range(range as u32)
                        .priority(MovementPriority::High);
                }
                CombatIntent::Flee { from, range } => {
                    let targets: Vec<FleeTarget> = from.iter().map(|p| FleeTarget { pos: *p, range: range as u32 }).collect();
                    if !targets.is_empty() {
                        tick_context.runtime_data.movement.flee(creep_entity, targets).range(range as u32);
                    }
                }
                // decide_movement emits only MoveTo/Flee (empty = hold this tick).
                _ => {}
            }
        }
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

        // P-OBJ #23 zero-orphan recall: this creep's squad is GONE (the manager retired it — resolved a
        // clear, gave up, or it was wiped) and there is nothing to fight here. Rather than idling in place
        // (the observed "stuck on a room edge" scatter), recall to the nearest home spawn and recycle,
        // reclaiming part of the body energy. A squad that still exists but simply has no orders this tick
        // is NOT orphaned — `get_squad_state` resolves for it — so a live squad is never recalled.
        let orphaned =
            state_context.squad_entity.is_some() && get_squad_state(state_context.squad_entity, tick_context).is_none();
        if orphaned && hostiles.is_empty() {
            Self::recall_to_recycle(creep, creep_pos, creep_entity, tick_context);
            return;
        }

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
            // Pure healer: escort the squad PROACTIVELY. Move to range 1 of the
            // nearest damaged friendly if any are hurt, otherwise the nearest
            // friendly COMBATANT (our attacker). Escorting the attacker even when
            // nobody is damaged keeps the healer travelling WITH it and already
            // adjacent when it takes fire — instead of lagging behind and only
            // closing the gap after the attacker is already hurt.
            let my_creeps = get_friendly_creeps(creep_pos.room_name(), tick_context);
            let escort_target = my_creeps
                .iter()
                .filter(|c| c.pos() != creep_pos && c.hits() < c.hits_max())
                .min_by_key(|c| creep_pos.get_range_to(c.pos()))
                .or_else(|| {
                    my_creeps
                        .iter()
                        .filter(|c| {
                            c.pos() != creep_pos
                                && c.body().iter().any(|p| p.hits() > 0 && matches!(p.part(), Part::Attack | Part::RangedAttack))
                        })
                        .min_by_key(|c| creep_pos.get_range_to(c.pos()))
                });

            if let Some(target) = escort_target {
                if creep_pos.get_range_to(target.pos()) > 1 {
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

    /// P-OBJ #23: send an orphaned squad creep home to recycle rather than letting it idle/scatter where
    /// its squad was retired. Moves to the nearest of our spawns and, once adjacent, recycles (reclaiming
    /// part of the body energy); if we somehow have no spawn at all, suicides rather than leaving a
    /// permanently idle creep. Called only for a creep whose squad has vanished (see `fallback_movement`).
    fn recall_to_recycle(creep: &Creep, creep_pos: Position, creep_entity: Entity, tick_context: &mut JobTickContext) {
        match game::spawns().values().min_by_key(|s| creep_pos.get_range_to(s.pos())) {
            Some(spawn) if creep_pos.get_range_to(spawn.pos()) > 1 => {
                tick_context
                    .runtime_data
                    .movement
                    .move_to(creep_entity, spawn.pos())
                    .range(1)
                    .priority(MovementPriority::Normal);
            }
            Some(spawn) => {
                let _ = spawn.recycle_creep(creep);
            }
            None => {
                let _ = creep.suicide();
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

        // Re-engage once HP recovers above 80%, or if squad signals engage --
        // but NEVER while the squad itself is signalling Retreat. Otherwise a
        // healthy creep ping-pongs Engaged<->Retreating every tick (Engaged sees
        // the squad retreat signal -> Retreating; Retreating sees HP>80% ->
        // Engaged), hitting the 20-transition guard and never actually
        // retreating. Stay retreating until the squad clears the signal (e.g.
        // the Lanchester re-engage band against an unwinnable target).
        let squad_retreating = squad_state.map(|s| s == SquadState::Retreating).unwrap_or(false);
        if !squad_retreating
            && (creep.hits() > creep.hits_max() * 4 / 5 || (squad_wants_engage && creep.hits() > creep.hits_max() * 3 / 5))
        {
            return Some(SquadCombatState::engaged());
        }

        // Read tick orders for coordinated retreat.
        let tick_orders = get_tick_orders(state_context.squad_entity, creep_entity, tick_context);

        // Pipeline B: Ranged mass attack while retreating.
        if has_active_part(creep, Part::RangedAttack) {
            crate::intents::ranged_mass_attack(creep, &mut tick_context.action_flags, tick_context.runtime_data.intent_recorder);
        }

        // Pipeline A: Melee attack if adjacent.
        if has_active_part(creep, Part::Attack) {
            let hostiles = get_hostile_creeps(creep_pos.room_name(), tick_context);
            if let Some(target) = hostiles.iter().find(|c| creep_pos.get_range_to(c.pos()) <= 1) {
                crate::intents::attack(
                    creep,
                    &mut tick_context.action_flags,
                    tick_context.runtime_data.intent_recorder,
                    target,
                    target.pos(),
                );
            }
        }

        // Pipeline C: Heal -- resolve assigned target by ID, else best nearby.
        if has_active_part(creep, Part::Heal) {
            let heal_target = tick_orders.as_ref().and_then(|o| o.heal_target).and_then(|id| id.resolve());
            if let Some(target) = heal_target {
                let range = creep_pos.get_range_to(target.pos());
                if range <= 1 {
                    crate::intents::heal(
                        creep,
                        &mut tick_context.action_flags,
                        tick_context.runtime_data.intent_recorder,
                        &target,
                        target.pos(),
                    );
                } else if range <= 3 {
                    crate::intents::ranged_heal(
                        creep,
                        &mut tick_context.action_flags,
                        tick_context.runtime_data.intent_recorder,
                        &target,
                        target.pos(),
                    );
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
        crate::intents::heal(
            creep,
            &mut tick_context.action_flags,
            tick_context.runtime_data.intent_recorder,
            target,
            target.pos(),
        );
        return;
    }

    // Self-heal if damaged.
    if creep.hits() < creep.hits_max() {
        let creep_pos = creep.pos();
        crate::intents::heal(
            creep,
            &mut tick_context.action_flags,
            tick_context.runtime_data.intent_recorder,
            creep,
            creep_pos,
        );
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
        crate::intents::ranged_heal(
            creep,
            &mut tick_context.action_flags,
            tick_context.runtime_data.intent_recorder,
            target,
            target.pos(),
        );
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
        let target_tile = get_formation_target(state_context.squad_entity, creep_entity, tick_context, creep_pos)?;
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
    squad: Option<SquadRef>,
    creep_entity: Entity,
    tick_context: &JobTickContext,
    creep_pos_fallback: Position,
) -> Option<Position> {
    let entity = squad?.resolve(tick_context.system_data.entities)?;
    let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
    let member = squad_ctx.get_member(creep_entity)?;
    let virtual_pos = squad_ctx.squad_path.as_ref().map(|p| p.anchor.virtual_pos)?;
    let dest_room = squad_ctx.squad_path.as_ref().map(|p| p.anchor.destination.room_name());
    let layout = squad_ctx.layout.as_ref()?;
    let target = virtual_anchor_target(virtual_pos, layout, member.formation_slot)?;

    // Prefer cached position; use live creep position when not yet synced (e.g. second of duo).
    let creep_pos = member.position.unwrap_or(creep_pos_fallback);
    cross_room_formation_target(creep_pos, target, dest_room)
}

/// Resolve a member's per-tick formation move target given its slot `target` (derived from the
/// anchor's `virtual_pos`) and the squad's `dest_room` (the anchor's destination room).
///
/// - **Same room as the slot** → move to the slot.
/// - **Already crossed into `dest_room` while the anchor still lags in the rear room** → HOLD in
///   place (`creep_pos`). This is the W7N3 border-ping-pong fix: while the boundary-hold quorum gate
///   freezes `virtual_pos` in the rear room, every slot resolves to the rear room, so a member that
///   has already entered the destination room would otherwise be sent back to its own room's exit
///   ring — where the engine bounces it across the boundary, in and out, forever. Holding lets the
///   laggards/anchor close up; normal slot-following resumes the moment the anchor advances into the
///   destination room (then the same-room branch above fires).
/// - **Otherwise** → head to the current room's edge toward the slot's room (world-coord direction).
fn cross_room_formation_target(creep_pos: Position, target: Position, dest_room: Option<RoomName>) -> Option<Position> {
    if creep_pos.room_name() == target.room_name() {
        return Some(target);
    }
    if Some(creep_pos.room_name()) == dest_room {
        // Crossed into the destination room ahead of the anchor — wait here, don't get expelled.
        return Some(creep_pos);
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
                squad_entity: Some(SquadRef::from_entity(squad_entity)),
                combat_response_start: None,
            },
            state: SquadCombatState::move_to_room(),
        }
    }
}

/// Look up the squad state for a job that may or may not be in a squad.
fn get_squad_state(squad: Option<SquadRef>, tick_context: &JobTickContext) -> Option<SquadState> {
    let entity = squad?.resolve(tick_context.system_data.entities)?;
    let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
    Some(squad_ctx.state)
}

/// ADR 0027 v1.1 P2: the controller TILE this squad must `attackController` (de-claim), if its objective is a
/// `SquadTarget::AttackController`. `None` for every combat squad — so the declaim drive below is inert for
/// all existing objectives. The position is read off the squad's shared `SquadContext.target` (set by the
/// manager from the `Declaim` objective), so every member of the declaim squad sees the same tile.
fn squad_attack_controller_pos(squad: Option<SquadRef>, tick_context: &JobTickContext) -> Option<Position> {
    let entity = squad?.resolve(tick_context.system_data.entities)?;
    let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
    match squad_ctx.target {
        Some(SquadTarget::AttackController { position }) => Some(position),
        _ => None,
    }
}

/// ADR 0027 v1.1 P2 — drive a DECLAIM member: move adjacent to the controller tile and `attackController`
/// (the EXISTING `DeclaimJob` behavior — strike + the 1000-tick upgrade-block cadence). A declaimer carries
/// CLAIM + MOVE only (no combat parts), so it never fights; it just reaches the controller and strikes when
/// the upgrade-block clears. Returns `true` when it acted as a declaimer (the caller then skips the combat
/// pipeline this tick). `controller_pos` is the controller TILE from the squad target. Mirrors
/// `controllerbehavior::tick_attack_controller` (resolved from the tile, since the squad target carries a
/// `Position`, not a `RemoteObjectId`).
fn drive_declaim(controller_pos: Position, tick_context: &mut JobTickContext) {
    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();
    let creep_entity = tick_context.runtime_data.creep_entity;

    if !creep_pos.is_near_to(controller_pos) {
        // Not yet adjacent — close to range 1 of the controller (routes through the confirmed-derelict room
        // with HighCost, like the dismantler / the former DeclaimJob).
        tick_context
            .runtime_data
            .movement
            .move_to(creep_entity, controller_pos)
            .range(1)
            .room_options(RoomOptions::new(HostileBehavior::HighCost))
            .priority(MovementPriority::High);
        return;
    }

    // Adjacent — strike the controller. Resolve it from the structures at the tile (the squad target carries
    // the tile, not an id). Already-neutral or upgrade-blocked (a strike within the last 1000 ticks) ⇒ no
    // intent this tick — the squad simply HOLDS adjacent until the block clears (the manager's lease keeps it
    // committed across the cadence). The controller going neutral is observed by the manager (the de-claim
    // is achieved → the producer withdraws the Declaim objective → the squad retires).
    if let Some(controller) = game::rooms().get(controller_pos.room_name()).and_then(|room| room.controller()) {
        let owned_or_reserved = controller.owner().is_some() || controller.reservation().is_some();
        let upgrade_blocked = controller.upgrade_blocked().unwrap_or(0) > 0;
        if owned_or_reserved && !upgrade_blocked && tick_context.action_flags.consume(SimultaneousActionFlags::ATTACK_CONTROLLER) {
            let _ = creep.attack_controller(&controller);
        }
    }
}

/// Whether the squad has a populated anchor path (`SquadPath`). Anchor-driven
/// formation movement only applies when one exists; manager-fielded squads
/// (P2.G3) have none and own their movement via the job (kiting).
fn squad_has_anchor(squad: Option<SquadRef>, tick_context: &JobTickContext) -> bool {
    squad
        .and_then(|s| s.resolve(tick_context.system_data.entities))
        .and_then(|e| tick_context.system_data.squad_contexts.get(e))
        .map(|ctx| ctx.squad_path.is_some())
        .unwrap_or(false)
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
fn get_tick_orders(squad: Option<SquadRef>, creep_entity: Entity, tick_context: &JobTickContext) -> Option<TickOrders> {
    let entity = squad?.resolve(tick_context.system_data.entities)?;
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

#[cfg(test)]
mod tests {
    use super::cross_room_formation_target;
    use screeps::{Position, RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8, room: &str) -> Position {
        Position::new(
            RoomCoordinate::new(x).unwrap(),
            RoomCoordinate::new(y).unwrap(),
            room.parse::<RoomName>().unwrap(),
        )
    }

    /// Regression for the W7N3 border ping-pong: a formation member that has already crossed into the
    /// squad's destination room while the anchor is still held in the rear room must HOLD in place,
    /// NOT be handed an exit-edge tile (which the engine bounces back across the boundary).
    #[test]
    fn crossed_member_holds_instead_of_being_expelled() {
        // Lead member is inside the destination room (W7N3) at its top edge; the anchor/slot is still
        // frozen in the rear room (W7N4).
        let lead = pos(36, 0, "W7N3");
        let slot_in_rear = pos(25, 25, "W7N4");
        let dest = Some("W7N3".parse::<RoomName>().unwrap());
        assert_eq!(
            cross_room_formation_target(lead, slot_in_rear, dest),
            Some(lead),
            "a member already in the destination room must hold, not be expelled to the exit ring"
        );
    }

    #[test]
    fn member_follows_its_slot_when_in_the_slot_room() {
        let lead = pos(36, 5, "W7N3");
        let slot = pos(30, 30, "W7N3");
        assert_eq!(
            cross_room_formation_target(lead, slot, Some("W7N3".parse().unwrap())),
            Some(slot),
            "same room as the slot -> go to the slot"
        );
    }

    #[test]
    fn laggard_in_rear_room_heads_for_the_edge() {
        // A laggard still in the rear room (W7N4) with its slot in the destination room (W7N3) heads
        // for an edge tile of its OWN room (not held, not the slot).
        let laggard = pos(25, 40, "W7N4");
        let slot = pos(30, 30, "W7N3");
        let r = cross_room_formation_target(laggard, slot, Some("W7N3".parse().unwrap())).unwrap();
        assert_eq!(r.room_name(), laggard.room_name(), "edge tile is on the creep's own room");
        let on_edge = r.x().u8() == 0 || r.x().u8() == 49 || r.y().u8() == 0 || r.y().u8() == 49;
        assert!(on_edge, "laggard is routed to a room-edge tile toward the destination");
    }
}
