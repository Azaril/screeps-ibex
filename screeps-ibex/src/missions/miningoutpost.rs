use super::data::*;
use super::defend::*;
use super::dismantle::*;
use super::missionsystem::*;
use super::raid::*;
use super::remotemine::*;
use super::reserve::*;
use crate::componentaccess::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use log::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct MiningOutpostMissionContext {
    home_room_data: Entity,
    outpost_room_data: Entity,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum MiningOutpostState {
        Scout {
            phantom: std::marker::PhantomData<Entity>
        },
        Cleanup {
            raid_mission: EntityOption<Entity>,
            dismantle_mission: EntityOption<Entity>,
            defend_mission: EntityOption<Entity>
        },
        Mine {
            remote_mine_mission: EntityOption<Entity>,
            reserve_mission: EntityOption<Entity>,
            defend_mission: EntityOption<Entity>
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &MiningOutpostMissionContext) -> String {
            format!("Mining Outpost - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

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

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut MiningOutpostMissionContext) -> Result<Option<MiningOutpostState>, String>;

        * => fn complete(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) {
            for mission_child in self.get_children_internal_mut().iter_mut() {
                mission_child.take().map(|e| system_data.mission_requests.abort(e));
            }
        }
    }
);

fn can_run_mission(
    system_data: &mut MissionExecutionSystemData,
    _mission_entity: Entity,
    state_context: &mut MiningOutpostMissionContext,
) -> Result<bool, String> {
    let outpost_room_data = system_data
        .room_data
        .get(state_context.outpost_room_data)
        .ok_or("Expected outpost room data")?;

    if let Some(dynamic_visibility_data) = outpost_room_data.get_dynamic_visibility_data() {
        if dynamic_visibility_data.updated_within(1000)
            && (!dynamic_visibility_data.owner().neutral()
                || dynamic_visibility_data.reservation().hostile()
                || dynamic_visibility_data.reservation().friendly())
        {
            return Ok(false);
        }
    }

    Ok(true)
}

impl Scout {
    fn get_children_internal(&self) -> [&Option<Entity>; 0] {
        []
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 0] {
        []
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<Option<MiningOutpostState>, String> {
        if !can_run_mission(system_data, mission_entity, state_context)? {
            return Err("Mission cannot run in current room state".to_string());
        }

        let outpost_room_data = system_data
            .room_data
            .get_mut(state_context.outpost_room_data)
            .ok_or("Expected outpost room data")?;

        if let Some(static_visibility_data) = outpost_room_data.get_static_visibility_data() {
            if static_visibility_data.sources().is_empty() {
                return Err("No sources available for mining outpost, aborting mission.".to_string());
            }
        }

        if outpost_room_data
            .get_dynamic_visibility_data()
            .map(|v| !v.visible())
            .unwrap_or(true)
        {
            system_data
                .visibility
                .request(VisibilityRequest::new(outpost_room_data.name, VISIBILITY_PRIORITY_MEDIUM));

            Ok(None)
        } else {
            info!("Completed scouting of room - transitioning to cleanup");

            Ok(Some(MiningOutpostState::cleanup(None.into(), None.into(), None.into())))
        }
    }
}

impl Cleanup {
    fn get_children_internal(&self) -> [&Option<Entity>; 3] {
        [&self.raid_mission, &self.dismantle_mission, &self.defend_mission]
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 3] {
        [&mut self.raid_mission, &mut self.dismantle_mission, &mut self.defend_mission]
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<Option<MiningOutpostState>, String> {
        if !can_run_mission(system_data, mission_entity, state_context)? {
            return Err("Mission cannot run in current room state".to_string());
        }

        self.tick_scouting(system_data, mission_entity, state_context)?;
        self.tick_raiding(system_data, mission_entity, state_context)?;
        self.tick_dismantling(system_data, mission_entity, state_context)?;
        self.tick_defend(system_data, mission_entity, state_context)?;

        let room_is_safe = system_data
            .missions
            .try_get(self.defend_mission)
            .as_mission_type::<DefendMission>()
            .map(|d| d.is_room_safe())
            .unwrap_or(true);

        if let Some(mut raid_mission) = system_data.missions.try_get(self.raid_mission).as_mission_type_mut::<RaidMission>() {
            raid_mission.allow_spawning(room_is_safe);
        }

        if let Some(mut dismantle_mission) = system_data
            .missions
            .try_get(self.dismantle_mission)
            .as_mission_type_mut::<DismantleMission>()
        {
            dismantle_mission.allow_spawning(room_is_safe);
        }

        if self.raid_mission.is_none() && self.dismantle_mission.is_none() {
            info!("No active raiding or dismantling - transitioning to mining");

            Ok(Some(MiningOutpostState::mine(
                None.into(),
                None.into(),
                self.defend_mission.clone(),
            )))
        } else {
            Ok(None)
        }
    }

    fn tick_scouting(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<(), String> {
        let needs_scout = self.requires_scouting(system_data, mission_entity, state_context)?;

        if needs_scout {
            let outpost_room_data = system_data
                .room_data
                .get(state_context.outpost_room_data)
                .ok_or("Expected outpost room")?;

            system_data
                .visibility
                .request(VisibilityRequest::new(outpost_room_data.name, VISIBILITY_PRIORITY_MEDIUM));
        }

        Ok(())
    }

    fn requires_scouting(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<bool, String> {
        let outpost_room_data = system_data
            .room_data
            .get(state_context.outpost_room_data)
            .ok_or("Expected outpost room")?;

        let requires_scouting = outpost_room_data
            .get_dynamic_visibility_data()
            .map(|v| !v.updated_within(1000))
            .unwrap_or(true);

        Ok(requires_scouting)
    }

    fn tick_raiding(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<(), String> {
        let has_raid = self.raid_mission.map(|e| system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_raid {
            let needs_raiding = self.requires_raiding(system_data, mission_entity, state_context)?;

            if needs_raiding.unwrap_or(false) {
                let outpost_room_data = system_data
                    .room_data
                    .get_mut(state_context.outpost_room_data)
                    .ok_or("Expected outpost room data")?;

                let mission_entity = RaidMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(mission_entity),
                    state_context.outpost_room_data,
                    state_context.home_room_data,
                )
                .build();

                outpost_room_data.add_mission(mission_entity);

                self.raid_mission = Some(mission_entity).into();
            }
        }

        Ok(())
    }

    fn requires_raiding(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<Option<bool>, String> {
        let outpost_room_data = system_data
            .room_data
            .get(state_context.outpost_room_data)
            .ok_or("Expected outpost room")?;

        if let Some(structures) = outpost_room_data.get_structures() {
            let structures = structures.all();

            let has_resources = structures.iter().any(|structure| {
                if let Some(store) = structure.as_has_store() {
                    let store_types = store.store_types();

                    return store_types.iter().any(|t| store.store_used_capacity(Some(*t)) > 0);
                }

                false
            });

            return Ok(Some(has_resources));
        }

        Ok(None)
    }

    fn tick_dismantling(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<(), String> {
        let has_dismantle = self.dismantle_mission.map(|e| system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_dismantle {
            let needs_dismantling = self.requires_dismantling(system_data, mission_entity, state_context)?;

            if needs_dismantling.unwrap_or(false) {
                let outpost_room_data = system_data
                    .room_data
                    .get_mut(state_context.outpost_room_data)
                    .ok_or("Expected outpost room data")?;

                let mission_entity = DismantleMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(mission_entity),
                    state_context.outpost_room_data,
                    state_context.home_room_data,
                    false,
                )
                .build();

                outpost_room_data.add_mission(mission_entity);

                self.dismantle_mission = Some(mission_entity).into();
            }
        }

        Ok(())
    }

    fn requires_dismantling(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<Option<bool>, String> {
        let outpost_room_data = system_data
            .room_data
            .get(state_context.outpost_room_data)
            .ok_or("Expected outpost room")?;

        if let Some(structures) = outpost_room_data.get_structures() {
            let requires_dismantling = DismantleMission::requires_dismantling(structures.all());

            return Ok(Some(requires_dismantling));
        }

        Ok(None)
    }

    fn tick_defend(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<(), String> {
        let has_defend = self.defend_mission.map(|e| system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_defend {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = DefendMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                state_context.home_room_data,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.defend_mission = Some(mission_entity).into();
        }

        Ok(())
    }
}

impl Mine {
    fn get_children_internal(&self) -> [&Option<Entity>; 3] {
        [&self.remote_mine_mission, &self.reserve_mission, &self.defend_mission]
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 3] {
        [&mut self.remote_mine_mission, &mut self.reserve_mission, &mut self.defend_mission]
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<Option<MiningOutpostState>, String> {
        if !can_run_mission(system_data, mission_entity, state_context)? {
            return Err("Mission cannot run in current room state".to_string());
        }

        let has_remote_mine = self.remote_mine_mission.map(|e| system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_remote_mine {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = RemoteMineMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                state_context.home_room_data,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.remote_mine_mission = Some(mission_entity).into();
        }

        let has_reserve = self.reserve_mission.map(|e| system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_reserve {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = ReserveMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                state_context.home_room_data,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.reserve_mission = Some(mission_entity).into();
        }

        let has_defend = self.defend_mission.map(|e| system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_defend {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = DefendMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                state_context.home_room_data,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.defend_mission = Some(mission_entity).into();
        }

        let room_is_safe = system_data
            .missions
            .try_get(self.defend_mission)
            .as_mission_type::<DefendMission>()
            .map(|d| d.is_room_safe())
            .unwrap_or(true);

        if let Some(mut remote_mine_mission) = system_data
            .missions
            .try_get(self.remote_mine_mission)
            .as_mission_type_mut::<RemoteMineMission>()
        {
            remote_mine_mission.allow_spawning(room_is_safe);
        }

        if let Some(mut reserve_mission) = system_data
            .missions
            .try_get(self.reserve_mission)
            .as_mission_type_mut::<ReserveMission>()
        {
            reserve_mission.allow_spawning(room_is_safe);
        }

        Ok(None)
    }
}

#[derive(ConvertSaveload)]
pub struct MiningOutpostMission {
    owner: EntityOption<Entity>,
    context: MiningOutpostMissionContext,
    state: MiningOutpostState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MiningOutpostMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, outpost_room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = MiningOutpostMission::new(owner, outpost_room_data, home_room_data);

        builder
            .with(MissionData::MiningOutpost(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, outpost_room_data: Entity, home_room_data: Entity) -> MiningOutpostMission {
        MiningOutpostMission {
            owner: owner.into(),
            context: MiningOutpostMissionContext {
                home_room_data,
                outpost_room_data,
            },
            state: MiningOutpostState::scout(std::marker::PhantomData),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for MiningOutpostMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.context.outpost_room_data
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

        self.state.visualize(system_data, mission_entity);

        Ok(MissionResult::Running)
    }

    fn complete(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) {
        self.state.complete(system_data, mission_entity);
    }
}