use super::data::*;
use super::missionsystem::*;
use super::utility::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct DefendMissionContext {
    home_room_datas: EntityVec<Entity>,
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

        let dynamic_visibility_data = defend_room_data
            .get_dynamic_visibility_data()
            .ok_or("Expected dynamic visibility")?;

        if dynamic_visibility_data.visible() && (dynamic_visibility_data.hostile_creeps() || dynamic_visibility_data.hostile_structures()) {
            return Ok(Some(DefendState::active(EntityVec::new(), None)));
        }

        let visibility_age = dynamic_visibility_data.age();

        if visibility_age >= 100 {
            system_data.visibility.request(VisibilityRequest::new(
                defend_room_data.name,
                VISIBILITY_PRIORITY_MEDIUM,
                VisibilityRequestFlags::ALL,
            ));
        } else if visibility_age >= 20 {
            system_data.visibility.request(VisibilityRequest::new(
                defend_room_data.name,
                VISIBILITY_PRIORITY_MEDIUM,
                VisibilityRequestFlags::OBSERVE,
            ));
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

        let dynamic_visibility_data = defend_room_data
            .get_dynamic_visibility_data()
            .ok_or("Expected dynamic visibility")?;

        if dynamic_visibility_data.visible() {
            if dynamic_visibility_data.hostile_creeps() || dynamic_visibility_data.hostile_structures() {
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
    pub fn build<B>(builder: B, owner: Option<Entity>, defend_room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = DefendMission::new(owner, defend_room_data, home_room_datas);

        builder
            .with(MissionData::Defend(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, defend_room_data: Entity, home_room_datas: &[Entity]) -> DefendMission {
        DefendMission {
            owner: owner.into(),
            context: DefendMissionContext {
                defend_room_data,
                home_room_datas: home_room_datas.to_owned().into(),
            },
            state: DefendState::idle(std::marker::PhantomData),
        }
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.context.home_room_datas.as_slice() != home_room_datas {
            self.context.home_room_datas = home_room_datas.to_owned().into();
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

        //
        // Cleanup home rooms that no longer exist.
        //

        self.context.home_room_datas
            .retain(|entity| {
                system_data.room_data
                    .get(*entity)
                    .map(is_valid_home_room)
                    .unwrap_or(false)
            });

        if self.context.home_room_datas.is_empty() {
            return Err("No home rooms for defend mission".to_owned());
        }

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
