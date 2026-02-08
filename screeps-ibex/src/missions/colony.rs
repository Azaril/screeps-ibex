#![allow(clippy::too_many_arguments)]

use super::construction::*;
use super::data::*;
use super::defend::*;
use super::haul::*;
use super::labs::*;
use super::localbuild::*;
use super::localsupply::*;
use super::missionsystem::*;
use super::powerspawn::*;
use super::terminal::*;
use super::tower::*;
use super::upgrade::*;
use crate::room::data::*;
use crate::serialize::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

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
            upgrade_mission: EntityOption<Entity>,
            power_spawn_mission: EntityOption<Entity>,
            labs_mission: EntityOption<Entity>,
            defend_mission: EntityOption<Entity>,
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &ColonyMissionContext) -> String {
            format!("Colony - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &ColonyMissionContext) {}

        * => fn get_children(&self) -> Vec<Entity> {
            self.get_children_internal()
                .iter()
                .filter_map(|e| e.as_ref())
                .cloned()
                .collect()
        }

        * => fn child_complete(&mut self, child: Entity) {
            for mission_child in self.get_children_internal_mut().iter_mut() {
                if mission_child.map(|e| e == child).unwrap_or(false) {
                    mission_child.take();
                }
            }
        }

        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut ColonyMissionContext) -> Result<Option<ColonyState>, String>;

        * => fn complete(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) {
            for mission_child in self.get_children_internal_mut().iter_mut() {
                if let Some(e) = mission_child.take() { system_data.mission_requests.abort(e) }
            }
        }
    }
);

impl Incubate {
    fn get_children_internal(&self) -> [&Option<Entity>; 10] {
        [
            &self.construction_mission,
            &self.local_supply_mission,
            &self.local_build_mission,
            &self.haul_mission,
            &self.terminal_mission,
            &self.tower_mission,
            &self.upgrade_mission,
            &self.power_spawn_mission,
            &self.labs_mission,
            &self.defend_mission,
        ]
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 10] {
        [
            &mut self.construction_mission,
            &mut self.local_supply_mission,
            &mut self.local_build_mission,
            &mut self.haul_mission,
            &mut self.terminal_mission,
            &mut self.tower_mission,
            &mut self.upgrade_mission,
            &mut self.power_spawn_mission,
            &mut self.labs_mission,
            &mut self.defend_mission,
        ]
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut ColonyMissionContext,
    ) -> Result<Option<ColonyState>, String> {
        let room_data = system_data
            .room_data
            .get_mut(state_context.room_data)
            .ok_or("Expected colony room data")?;

        if !ColonyMission::can_run(room_data) {
            return Err("Colony room not owned!".to_owned());
        }

        if self.construction_mission.is_none() {
            let mission_entity = ConstructionMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
            )
            .build();

            room_data.add_mission(mission_entity);

            self.construction_mission = Some(mission_entity).into();
        }

        if self.local_supply_mission.is_none() {
            let mission_entity = LocalSupplyMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
                &[state_context.room_data],
            )
            .build();

            room_data.add_mission(mission_entity);

            self.local_supply_mission = Some(mission_entity).into();
        }

        if self.local_build_mission.is_none() {
            let mission_entity = LocalBuildMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
            )
            .build();

            room_data.add_mission(mission_entity);

            self.local_build_mission = Some(mission_entity).into();
        }

        if self.haul_mission.is_none() {
            let mission_entity = HaulMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
                &[state_context.room_data],
            )
            .build();

            room_data.add_mission(mission_entity);

            self.haul_mission = Some(mission_entity).into();
        }

        if self.terminal_mission.is_none() && TerminalMission::can_run(room_data) {
            let mission_entity = TerminalMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
            )
            .build();

            room_data.add_mission(mission_entity);

            self.terminal_mission = Some(mission_entity).into();
        }

        if self.tower_mission.is_none() {
            let mission_entity = TowerMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
            )
            .build();

            room_data.add_mission(mission_entity);

            self.tower_mission = Some(mission_entity).into();
        }

        if self.upgrade_mission.is_none() && UpgradeMission::can_run(room_data) {
            let mission_entity = UpgradeMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
            )
            .build();

            room_data.add_mission(mission_entity);

            self.upgrade_mission = Some(mission_entity).into();
        }

        if self.power_spawn_mission.is_none() && PowerSpawnMission::can_run(room_data) {
            let mission_entity = PowerSpawnMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
            )
            .build();

            room_data.add_mission(mission_entity);

            self.power_spawn_mission = Some(mission_entity).into();
        }

        if self.labs_mission.is_none() && LabsMission::can_run(room_data) {
            let mission_entity = LabsMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
            )
            .build();

            room_data.add_mission(mission_entity);

            self.labs_mission = Some(mission_entity).into();
        }

        if self.defend_mission.is_none() {
            let mission_entity = DefendMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.room_data,
                &[state_context.room_data],
            )
            .build();

            room_data.add_mission(mission_entity);

            self.defend_mission = Some(mission_entity).into();
        }

        Ok(None)
    }
}

#[derive(ConvertSaveload)]
pub struct ColonyMission {
    owner: EntityOption<Entity>,
    context: ColonyMissionContext,
    state: ColonyState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ColonyMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ColonyMission::new(owner, room_data);

        builder
            .with(MissionData::Colony(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> ColonyMission {
        ColonyMission {
            owner: owner.into(),
            context: ColonyMissionContext { room_data },
            state: ColonyState::incubate(
                None.into(),
                None.into(),
                None.into(),
                None.into(),
                None.into(),
                None.into(),
                None.into(),
                None.into(),
                None.into(),
                None.into(),
            ),
        }
    }

    pub fn can_run(room_data: &RoomData) -> bool {
        if let Some(structures) = room_data.get_structures() {
            if structures.spawns().iter().any(|spawn| spawn.my()) {
                return true;
            }
        } else {
            return false;
        }

        room_data.get_dynamic_visibility_data().map(|v| v.owner().mine()).unwrap_or(false)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ColonyMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
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

        self.state.visualize(system_data, mission_entity, &self.context);

        Ok(MissionResult::Running)
    }

    fn complete(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) {
        self.state.complete(system_data, mission_entity);
    }
}
