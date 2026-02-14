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
pub struct TankJobContext {
    target_room: RoomName,
    /// Optional squad entity for coordinated behavior.
    #[serde(default)]
    squad_entity: Option<u32>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum TankState {
        MoveToRoom,
        Tanking,
        Retreating,
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut TankJobContext, tick_context: &mut JobTickContext) -> Option<TankState>;
    }
);

impl MoveToRoom {
    pub fn tick(&mut self, state_context: &mut TankJobContext, tick_context: &mut JobTickContext) -> Option<TankState> {
        let room_options = RoomOptions::new(HostileBehavior::Allow);

        tick_move_to_room(tick_context, state_context.target_room, Some(room_options), TankState::tanking)
    }
}

impl Tanking {
    pub fn tick(&mut self, state_context: &mut TankJobContext, tick_context: &mut JobTickContext) -> Option<TankState> {
        let creep = tick_context.runtime_data.owner;
        let creep_pos = creep.pos();

        // Check squad retreat signal.
        if let Some(squad_state) = get_squad_state(state_context.squad_entity, tick_context) {
            if squad_state == SquadState::Retreating {
                return Some(TankState::retreating());
            }
        }

        // Retreat if HP drops below 30% (tanks are meant to absorb damage,
        // but retreating lets healers catch up).
        if creep.hits() < creep.hits_max() * 3 / 10 {
            return Some(TankState::retreating());
        }

        // If we've left the target room, move back.
        if creep_pos.room_name() != state_context.target_room {
            return Some(TankState::move_to_room());
        }

        if let Some(room) = game::rooms().get(state_context.target_room) {
            let hostiles = room.find(find::HOSTILE_CREEPS, None);

            if hostiles.is_empty() {
                mark_idle(tick_context);
                return None;
            }

            // Move toward the nearest hostile and attack if adjacent.
            let target = hostiles.iter().min_by_key(|c| creep_pos.get_range_to(c.pos()));

            if let Some(target) = target {
                let range = creep_pos.get_range_to(target.pos());

                if range <= 1 {
                    // Attack the hostile.
                    if tick_context.action_flags.consume(SimultaneousActionFlags::ATTACK) {
                        let _ = creep.attack(target);
                    }
                    mark_working(tick_context, target.pos(), 1);
                } else {
                    // Move toward the target aggressively.
                    if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
                        tick_context
                            .runtime_data
                            .movement
                            .move_to(tick_context.runtime_data.creep_entity, target.pos())
                            .range(1)
                            .priority(MovementPriority::High);
                    }
                }
            }
        } else {
            mark_idle(tick_context);
        }

        None
    }
}

impl Retreating {
    pub fn tick(&mut self, state_context: &mut TankJobContext, tick_context: &mut JobTickContext) -> Option<TankState> {
        let creep = tick_context.runtime_data.owner;

        // Check squad state -- only re-engage if squad says so.
        let squad_state = get_squad_state(state_context.squad_entity, tick_context);
        let squad_wants_engage = squad_state
            .map(|s| s == SquadState::Engaged || s == SquadState::Moving)
            .unwrap_or(false);

        // Re-engage once HP recovers above 60%, or if squad signals engage.
        if creep.hits() > creep.hits_max() * 3 / 5 || (squad_wants_engage && creep.hits() > creep.hits_max() * 2 / 5) {
            return Some(TankState::tanking());
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
pub struct TankJob {
    pub context: TankJobContext,
    pub state: TankState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TankJob {
    pub fn new(target_room: RoomName) -> TankJob {
        TankJob {
            context: TankJobContext {
                target_room,
                squad_entity: None,
            },
            state: TankState::move_to_room(),
        }
    }

    pub fn new_with_squad(target_room: RoomName, squad_entity: Entity) -> TankJob {
        TankJob {
            context: TankJobContext {
                target_room,
                squad_entity: Some(squad_entity.id()),
            },
            state: TankState::move_to_room(),
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for TankJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Tank - {}", self.state.status_description()))
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
