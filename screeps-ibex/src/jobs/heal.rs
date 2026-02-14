use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::movebehavior::*;
use crate::military::squad::SquadState;
use screeps::*;
use screeps_machine::*;
use screeps_rover::*;
use serde::*;
use specs::Entity;

#[derive(Clone, Serialize, Deserialize)]
pub struct HealJobContext {
    target_room: RoomName,
    /// Optional squad entity for coordinated behavior.
    #[serde(default)]
    squad_entity: Option<u32>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum HealState {
        MoveToRoom,
        Healing,
        Retreating,
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut HealJobContext, tick_context: &mut JobTickContext) -> Option<HealState>;
    }
);

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut HealJobContext, tick_context: &mut JobTickContext) -> Option<HealState> {
        let room_options = RoomOptions::new(HostileBehavior::Allow);

        tick_move_to_room(tick_context, state_context.target_room, Some(room_options), HealState::healing)
    }
}

impl Healing {
    pub fn tick(&mut self, state_context: &mut HealJobContext, tick_context: &mut JobTickContext) -> Option<HealState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();

        // Check squad retreat signal.
        if let Some(squad_state) = get_squad_state(state_context.squad_entity, tick_context) {
            if squad_state == SquadState::Retreating {
                return Some(HealState::retreating());
            }
        }

        // Retreat if HP drops below 40% (healers are high-value, retreat earlier).
        if creep.hits() < creep.hits_max() * 2 / 5 {
            return Some(HealState::retreating());
        }

        // If we've left the target room, move back.
        if creep_pos.room_name() != state_context.target_room {
            return Some(HealState::move_to_room());
        }

        // Find the most damaged friendly creep, preferring squad heal priority target.
        if let Some(room) = game::rooms().get(state_context.target_room) {
            let friendlies = room.find(find::MY_CREEPS, None);

            // If we have a squad heal priority, try to find the creep at that position first.
            let squad_heal_pos = get_squad_heal_priority_pos(state_context.squad_entity, tick_context);

            let heal_target = if let Some(priority_pos) = squad_heal_pos {
                // Try to find the priority target at the squad-specified position.
                friendlies
                    .iter()
                    .find(|c| c.pos() == priority_pos && c.hits() < c.hits_max())
                    .or_else(|| friendlies.iter().filter(|c| c.hits() < c.hits_max()).min_by_key(|c| c.hits()))
            } else {
                friendlies.iter().filter(|c| c.hits() < c.hits_max()).min_by_key(|c| c.hits())
            };

            if let Some(target) = heal_target {
                let range = creep_pos.get_range_to(target.pos());

                if range <= 1 {
                    // Adjacent heal (12 HP per HEAL part).
                    if tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                        let _ = creep.heal(target);
                    }
                    mark_working(tick_context, target.pos(), 1);
                } else if range <= 3 {
                    // Ranged heal (4 HP per HEAL part).
                    if tick_context.action_flags.consume(SimultaneousActionFlags::RANGED_HEAL) {
                        let _ = creep.ranged_heal(target);
                    }
                    // Move closer for better healing.
                    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                        tick_context
                            .runtime_data
                            .movement
                            .move_to(tick_context.runtime_data.creep_entity, target.pos())
                            .range(1)
                            .priority(MovementPriority::High);
                    }
                } else {
                    // Move toward the target.
                    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                        tick_context
                            .runtime_data
                            .movement
                            .move_to(tick_context.runtime_data.creep_entity, target.pos())
                            .range(1)
                            .priority(MovementPriority::High);
                    }
                }
            } else {
                // No one to heal -- self-heal if damaged.
                if creep.hits() < creep.hits_max() && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                    let _ = creep.heal(creep);
                }
                mark_idle(tick_context);
            }
        } else {
            // Self-heal and idle.
            if creep.hits() < creep.hits_max() && tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
                let _ = creep.heal(creep);
            }
            mark_idle(tick_context);
        }

        None
    }
}

impl Retreating {
    pub fn tick(&mut self, state_context: &mut HealJobContext, tick_context: &mut JobTickContext) -> Option<HealState> {
        let creep = tick_context.runtime_data.owner;

        // Check squad state -- only re-engage if squad says so.
        let squad_state = get_squad_state(state_context.squad_entity, tick_context);
        let squad_wants_engage = squad_state
            .map(|s| s == SquadState::Engaged || s == SquadState::Moving)
            .unwrap_or(false);

        // Re-engage once HP recovers above 80%, or if squad signals engage.
        if creep.hits() > creep.hits_max() * 4 / 5 || (squad_wants_engage && creep.hits() > creep.hits_max() * 3 / 5) {
            return Some(HealState::healing());
        }

        // Self-heal while retreating.
        if tick_context.action_flags.consume(SimultaneousActionFlags::HEAL) {
            let _ = creep.heal(creep);
        }

        // Flee from hostiles.
        if let Some(room) = game::rooms().get(creep.pos().room_name()) {
            let hostiles = room.find(find::HOSTILE_CREEPS, None);
            let flee_targets: Vec<FleeTarget> = hostiles.iter().map(|c| FleeTarget { pos: c.pos(), range: 8 }).collect();

            if !flee_targets.is_empty() && tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                tick_context
                    .runtime_data
                    .movement
                    .flee(tick_context.runtime_data.creep_entity, flee_targets)
                    .range(8);
            }
        }

        None
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HealJob {
    pub context: HealJobContext,
    pub state: HealState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl HealJob {
    pub fn new(target_room: RoomName) -> HealJob {
        HealJob {
            context: HealJobContext {
                target_room,
                squad_entity: None,
            },
            state: HealState::move_to_room(),
        }
    }

    pub fn new_with_squad(target_room: RoomName, squad_entity: Entity) -> HealJob {
        HealJob {
            context: HealJobContext {
                target_room,
                squad_entity: Some(squad_entity.id()),
            },
            state: HealState::move_to_room(),
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

/// Look up the squad heal priority target's position from the SquadContext.
fn get_squad_heal_priority_pos(squad_entity_id: Option<u32>, tick_context: &JobTickContext) -> Option<Position> {
    let id = squad_entity_id?;
    let entity = tick_context.system_data.entities.entity(id);
    let squad_ctx = tick_context.system_data.squad_contexts.get(entity)?;
    let heal_entity = (*squad_ctx.heal_priority)?;
    // Find the member's position from the squad context.
    let member = squad_ctx.get_member(heal_entity)?;
    member.position
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for HealJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Heal - {}", self.state.status_description()))
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

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context) {
            self.state = tick_result
        }
    }
}
