use super::data::*;
use super::missionsystem::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct DefendMissionContext {
    home_room_data: Entity,
    defend_room_data: Entity,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum DefendState {
        Idle {
            phantom: std::marker::PhantomData<Entity>
        },
        Active {
            squads: EntityVec<Entity>,
            last_hostiles: Option<u32>
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &DefendMissionContext) -> String {
            format!("Defend - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &DefendMissionContext) {}

        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut DefendMissionContext) -> Result<Option<DefendState>, String>;

        _ => fn is_room_safe(&self) -> bool;
    }
);

fn room_has_hostiles(room: &Room) -> bool {
    let hostile_creeps = room.find(find::HOSTILE_CREEPS);

    if !hostile_creeps.is_empty() {
        return true;
    }

    let hostile_structures = room.find(find::HOSTILE_STRUCTURES);

    if !hostile_structures.is_empty() {
        return true;
    }

    false
}

impl Idle {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut DefendMissionContext,
    ) -> Result<Option<DefendState>, String> {
        let defend_room_data = system_data
            .room_data
            .get(state_context.defend_room_data)
            .ok_or("Expected defend room data")?;

        if game::rooms::get(defend_room_data.name)
            .as_ref()
            .map(room_has_hostiles)
            .unwrap_or(false)
        {
            return Ok(Some(DefendState::active(EntityVec::new(), None)));
        } else if defend_room_data
            .get_dynamic_visibility_data()
            .map(|v| !v.updated_within(1000))
            .unwrap_or(true)
        {
            system_data
                .visibility
                .request(VisibilityRequest::new(defend_room_data.name, VISIBILITY_PRIORITY_MEDIUM));
        }

        Ok(None)
    }

    fn is_room_safe(&self) -> bool {
        true
    }
}

impl Active {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut DefendMissionContext,
    ) -> Result<Option<DefendState>, String> {
        let defend_room_data = system_data
            .room_data
            .get(state_context.defend_room_data)
            .ok_or("Expected defend room data")?;

        let has_hostiles = game::rooms::get(defend_room_data.name).as_ref().map(room_has_hostiles);

        if let Some(has_hostiles) = has_hostiles {
            if has_hostiles {
                self.last_hostiles = Some(game::time());
            } else if self.last_hostiles.map(|last| game::time() - last >= 20).unwrap_or(false) {
                return Ok(Some(DefendState::idle(std::marker::PhantomData)));
            }
        }

        Ok(None)
    }

    fn is_room_safe(&self) -> bool {
        false
    }
}

#[derive(ConvertSaveload)]
pub struct DefendMission {
    owner: EntityOption<Entity>,
    context: DefendMissionContext,
    state: DefendState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DefendMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, defend_room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = DefendMission::new(owner, defend_room_data, home_room_data);

        builder
            .with(MissionData::Defend(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, defend_room_data: Entity, home_room_data: Entity) -> DefendMission {
        DefendMission {
            owner: owner.into(),
            context: DefendMissionContext {
                defend_room_data,
                home_room_data,
            },
            state: DefendState::idle(std::marker::PhantomData),
        }
    }

    pub fn is_room_safe(&self) -> bool {
        self.state.is_room_safe()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for DefendMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.context.defend_room_data
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String {
        self.state.describe_state(system_data, mission_entity, &self.context)
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        self.state.gather_data(system_data, mission_entity);

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        while let Some(tick_result) = self.state.tick(system_data, mission_entity, &mut self.context)? {
            self.state = tick_result
        }

        self.state.visualize(system_data, mission_entity, &mut self.context);

        Ok(MissionResult::Running)
    }
}
