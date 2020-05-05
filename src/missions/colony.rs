use super::data::*;
use super::missionsystem::*;
use super::context::*;
use crate::ownership::*;
use crate::serialize::*;
use super::construction::*;
use super::localsupply::*;
use super::localbuild::*;
use super::haul::*;
use super::terminal::*;
use super::tower::*;
use super::upgrade::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use screeps_machine::*;
use screeps::*;

#[derive(Clone, ConvertSaveload)]
pub struct ColonyMissionContext {
    room_data: Entity,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum ColonyState {
        Incubate { 
            construction_mission: EntityOption<Entity>,
            local_supply_mission: EntityOption<Entity>,
            local_build_mission: EntityOption<Entity>,
            haul_mission: EntityOption<Entity>,
            terminal_mission: EntityOption<Entity>,
            tower_mission: EntityOption<Entity>,
            upgrade_mission: EntityOption<Entity>
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _describe_data: &mut MissionDescribeData, _state_context: &ColonyMissionContext) -> String {
            format!("Colony - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _runtime_data: &mut MissionExecutionRuntimeData) {}

        _ => fn get_children(&self) -> Vec<Entity>;

        _ => fn child_complete(&mut self, child: Entity);
        
        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _runtime_data: &mut MissionExecutionRuntimeData) {}
        
        _ => fn tick(&mut self, state_context: &mut ColonyMissionContext, tick_context: &mut MissionTickContext) -> Result<Option<ColonyState>, String>;

        _ => fn complete(&mut self, _system_data: &mut MissionExecutionSystemData, _runtime_data: &mut MissionExecutionRuntimeData);
    }
);

impl Incubate {
    fn get_children(&self) -> Vec<Entity> {
        [&self.construction_mission, &self.local_supply_mission, &self.local_build_mission, &self.haul_mission, &self.terminal_mission, &self.tower_mission, &self.upgrade_mission]
            .iter()
            .filter_map(|e| e.as_ref())
            .cloned()
            .collect()
    }

    fn child_complete(&mut self, child: Entity) {
        let mut all_children = [&mut self.construction_mission, &mut self.local_supply_mission, &mut self.local_build_mission, &mut self.haul_mission, &mut self.terminal_mission, &mut self.tower_mission, &mut self.upgrade_mission];

        for mission_child in all_children.iter_mut() {
            if mission_child.map(|e| e == child).unwrap_or(false) {
                mission_child.take();
            }
        }
    }

    fn tick(&mut self, state_context: &mut ColonyMissionContext, tick_context: &mut MissionTickContext) -> Result<Option<ColonyState>, String> {
        let room_data = tick_context.system_data.room_data.get_mut(state_context.room_data).ok_or("Expected colony room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected coloy room")?;
        
        if !room_data.get_dynamic_visibility_data().map(|v| v.owner().mine()).unwrap_or(false) {
            return Err("Colony room not owned!".to_owned());
        }

        if self.construction_mission.is_none() { 
            let mission_entity = ConstructionMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.room_data,
            ).build();

            room_data.add_mission(mission_entity);

            self.construction_mission = Some(mission_entity).into();
        }

        if self.local_supply_mission.is_none() { 
            let mission_entity = LocalSupplyMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.room_data,
            ).build();

            room_data.add_mission(mission_entity);

            self.local_supply_mission = Some(mission_entity).into();
        }

        if self.local_build_mission.is_none() { 
            let mission_entity = LocalBuildMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.room_data,
            ).build();

            room_data.add_mission(mission_entity);

            self.local_build_mission = Some(mission_entity).into();
        }

        if self.haul_mission.is_none() { 
            let mission_entity = HaulMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.room_data,
            ).build();

            room_data.add_mission(mission_entity);

            self.haul_mission = Some(mission_entity).into();
        }

        if self.terminal_mission.is_none() && room.terminal().is_some() { 
            let mission_entity = TerminalMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.room_data,
            ).build();

            room_data.add_mission(mission_entity);

            self.terminal_mission = Some(mission_entity).into();
        }

        if self.tower_mission.is_none() { 
            let mission_entity = TowerMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.room_data,
            ).build();

            room_data.add_mission(mission_entity);

            self.tower_mission = Some(mission_entity).into();
        }

        if self.upgrade_mission.is_none() { 
            let mission_entity = UpgradeMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.room_data,
            ).build();

            room_data.add_mission(mission_entity);

            self.upgrade_mission = Some(mission_entity).into();
        }

        Ok(None)
    }

    fn complete(&mut self, system_data: &mut MissionExecutionSystemData, _runtime_data: &mut MissionExecutionRuntimeData) {
        self.construction_mission.take().map(|e| system_data.mission_requests.abort(e));
        self.local_supply_mission.take().map(|e| system_data.mission_requests.abort(e));
        self.local_build_mission.take().map(|e| system_data.mission_requests.abort(e));
        self.haul_mission.take().map(|e| system_data.mission_requests.abort(e));
        self.terminal_mission.take().map(|e| system_data.mission_requests.abort(e));
        self.tower_mission.take().map(|e| system_data.mission_requests.abort(e));
        self.upgrade_mission.take().map(|e| system_data.mission_requests.abort(e));
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct ColonyMission {
    owner: EntityOption<OperationOrMissionEntity>,
    context: ColonyMissionContext,
    state: ColonyState
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ColonyMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ColonyMission::new(owner, room_data);

        builder.with(MissionData::Colony(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity) -> ColonyMission {
        ColonyMission {
            owner: owner.into(),
            context: ColonyMissionContext { 
                room_data,
            },
            state: ColonyState::incubate(None.into(), None.into(), None.into(), None.into(), None.into(), None.into(), None.into())
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ColonyMission {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: OperationOrMissionEntity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.context.room_data
    }

    fn get_children(&self) -> Vec<Entity> {
        self.state.get_children()
    }

    fn child_complete(&mut self, child: Entity) {
        self.state.child_complete(child);
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, describe_data: &mut MissionDescribeData) -> String {
        self.state.describe_state(system_data, describe_data, &self.context)
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, runtime_data: &mut MissionExecutionRuntimeData) -> Result<(), String> {
        self.state.gather_data(system_data, runtime_data);

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let mut tick_context = MissionTickContext {
            system_data,
            runtime_data
        };

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context)? {
            self.state = tick_result
        }

        self.state.visualize(system_data, runtime_data);

        Ok(MissionResult::Running)
    }

    fn complete(&mut self, system_data: &mut MissionExecutionSystemData, runtime_data: &mut MissionExecutionRuntimeData) {
        self.state.complete(system_data, runtime_data);
    }
}
