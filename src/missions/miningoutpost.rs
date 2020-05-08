use super::data::*;
use super::missionsystem::*;
use super::context::*;
use crate::ownership::*;
use crate::serialize::*;
use crate::jobs::utility::dismantle::*;
use super::raid::*;
use super::remotemine::*;
use super::reserve::*;
use super::dismantle::*;
use super::defend::*;
use crate::room::visibilitysystem::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use screeps_machine::*;
use screeps::*;
use log::*;

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
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _describe_data: &mut MissionDescribeData, _state_context: &MiningOutpostMissionContext) -> String {
            format!("Mining Outpost - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _runtime_data: &mut MissionExecutionRuntimeData) {}

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
        
        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _runtime_data: &mut MissionExecutionRuntimeData) {}
        
        _ => fn tick(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<Option<MiningOutpostState>, String>;

        * => fn complete(&mut self, system_data: &mut MissionExecutionSystemData, _runtime_data: &mut MissionExecutionRuntimeData) {
            for mission_child in self.get_children_internal_mut().iter_mut() {
                mission_child.take().map(|e| system_data.mission_requests.abort(e));
            }
        }
    }
);

fn can_run_mission(state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<bool, String> {
    let outpost_room_data = tick_context.system_data.room_data.get(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

    if let Some(dynamic_visibility_data) = outpost_room_data.get_dynamic_visibility_data() {
        if dynamic_visibility_data.updated_within(1000) && 
            (!dynamic_visibility_data.owner().neutral() || 
            dynamic_visibility_data.reservation().hostile() || 
            dynamic_visibility_data.reservation().friendly()) {
                
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

    fn tick(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<Option<MiningOutpostState>, String> {
        if !can_run_mission(state_context, tick_context)? {
            return Err("Mission cannot run in current room state".to_string());
        }

        let outpost_room_data = tick_context.system_data.room_data.get_mut(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

        if let Some(static_visibility_data) = outpost_room_data.get_static_visibility_data() {
            if static_visibility_data.sources().is_empty() {
                return Err("No sources available for mining outpost, aborting mission.".to_string());
            }
        }
        
        if outpost_room_data.get_dynamic_visibility_data().map(|v| !v.visible()).unwrap_or(true) {
            tick_context
                .system_data
                .visibility
                .request(VisibilityRequest::new(outpost_room_data.name, VISIBILITY_PRIORITY_MEDIUM));

            Ok(None)
        } else {
            info!("Completed scouting of room - transitioning to cleanup");

            Ok(Some(MiningOutpostState::cleanup(None.into(), None.into(), None.into())))
        }
    }

    fn requires_scouting(&mut self, state_context: &MiningOutpostMissionContext, tick_context: &MissionTickContext) -> Result<bool, String> {
        let outpost_room_data = tick_context.system_data.room_data.get(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

        let needs_scouting = outpost_room_data.get_dynamic_visibility_data().map(|v| !v.visible()).unwrap_or(true);
    
        Ok(needs_scouting)
    }
}

impl Cleanup {
    fn get_children_internal(&self) -> [&Option<Entity>; 3] {
        [&self.raid_mission, &self.dismantle_mission, &self.defend_mission]
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 3] {
        [&mut self.raid_mission, &mut self.dismantle_mission, &mut self.defend_mission]
    }

    fn tick(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<Option<MiningOutpostState>, String> {
        if !can_run_mission(state_context, tick_context)? {
            return Err("Mission cannot run in current room state".to_string());
        }

        self.tick_scouting(state_context, tick_context)?;
        self.tick_raiding(state_context, tick_context)?;
        self.tick_dismantling(state_context, tick_context)?;
        self.tick_defend(state_context, tick_context)?;

        if self.raid_mission.is_none() && self.dismantle_mission.is_none() {
            info!("No active raiding or dismantling - transitioning to mining");
            
            Ok(Some(MiningOutpostState::mine(None.into(), None.into(), self.defend_mission.clone())))
        } else {
            Ok(None)
        }
    }

    fn tick_scouting(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<(), String> {
        let needs_scout = self.requires_scouting(state_context, tick_context)?;

        if needs_scout {
            let outpost_room_data = tick_context.system_data.room_data.get(state_context.outpost_room_data).ok_or("Expected outpost room")?;

            tick_context
                .system_data
                .visibility
                .request(VisibilityRequest::new(outpost_room_data.name, VISIBILITY_PRIORITY_MEDIUM));
        }

        Ok(())
    }

    fn requires_scouting(&mut self, state_context: &MiningOutpostMissionContext, tick_context: &MissionTickContext) -> Result<bool, String> {
        let outpost_room_data = tick_context.system_data.room_data.get(state_context.outpost_room_data).ok_or("Expected outpost room")?;

        let requires_scouting = outpost_room_data.get_dynamic_visibility_data().map(|v| !v.updated_within(1000)).unwrap_or(true);

        Ok(requires_scouting)
    }

    fn tick_raiding(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<(), String> {
        let has_raid = self.raid_mission.map(|e| tick_context.system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_raid {
            let needs_raiding = self.requires_raiding(state_context, tick_context)?;

            if needs_raiding.unwrap_or(false) {            
                let outpost_room_data = tick_context.system_data.room_data.get_mut(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

                let mission_entity = RaidMission::build(
                    tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                    Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                    state_context.outpost_room_data,
                    state_context.home_room_data,
                ).build();

                outpost_room_data.add_mission(mission_entity);

                self.raid_mission = Some(mission_entity).into();
            }
        }

        Ok(())
    }

    fn requires_raiding(&mut self, state_context: &MiningOutpostMissionContext, tick_context: &MissionTickContext) -> Result<Option<bool>, String> {
        let outpost_room_data = tick_context.system_data.room_data.get(state_context.outpost_room_data).ok_or("Expected outpost room")?;

        if let Some(room) = game::rooms::get(outpost_room_data.name) {
            let structures = room.find(find::STRUCTURES);

            let has_resources = structures
                .iter()
                .any(|structure| {
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

    fn tick_dismantling(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<(), String> {
        let has_dismantle = self.dismantle_mission.map(|e| tick_context.system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_dismantle {
            let needs_dismantling = self.requires_dismantling(state_context, tick_context)?;

            if needs_dismantling.unwrap_or(false) {            
                let outpost_room_data = tick_context.system_data.room_data.get_mut(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

                let mission_entity = DismantleMission::build(
                    tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                    Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                    state_context.outpost_room_data,
                    state_context.home_room_data,
                    false,
                ).build();

                outpost_room_data.add_mission(mission_entity);

                self.dismantle_mission = Some(mission_entity).into();
            }
        }

        Ok(())
    }

    fn requires_dismantling(&mut self, state_context: &MiningOutpostMissionContext, tick_context: &MissionTickContext) -> Result<Option<bool>, String> {
        let outpost_room_data = tick_context.system_data.room_data.get(state_context.outpost_room_data).ok_or("Expected outpost room")?;

        if let Some(room) = game::rooms::get(outpost_room_data.name) {
            let requires_dismantling = get_dismantle_structures(room, false).next().is_some();

            return Ok(Some(requires_dismantling));
        }
        
        Ok(None)
    }

    fn tick_defend(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<(), String> {
        let has_defend = self.defend_mission.map(|e| tick_context.system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_defend {
            let outpost_room_data = tick_context.system_data.room_data.get_mut(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

            let mission_entity = DefendMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.outpost_room_data,
                state_context.home_room_data
            ).build();

            outpost_room_data.add_mission(mission_entity);

            self.defend_mission = Some(mission_entity).into();
        }

        Ok(())
    }
}

impl Mine {
    fn get_children_internal(&self) -> [&Option<Entity>; 2] {
        [&self.remote_mine_mission, &self.reserve_mission]
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 2] {
        [&mut self.remote_mine_mission, &mut self.reserve_mission]
    }

    fn tick(&mut self, state_context: &mut MiningOutpostMissionContext, tick_context: &mut MissionTickContext) -> Result<Option<MiningOutpostState>, String> {
        if !can_run_mission(state_context, tick_context)? {
            return Err("Mission cannot run in current room state".to_string());
        }

        let has_remote_mine = self.remote_mine_mission.map(|e| tick_context.system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_remote_mine {
            let outpost_room_data = tick_context.system_data.room_data.get_mut(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

            let mission_entity = RemoteMineMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.outpost_room_data,
                state_context.home_room_data,
            ).build();

            outpost_room_data.add_mission(mission_entity);

            self.remote_mine_mission = Some(mission_entity).into();
        }

        let has_reserve = self.reserve_mission.map(|e| tick_context.system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_reserve {
            let outpost_room_data = tick_context.system_data.room_data.get_mut(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

            let mission_entity = ReserveMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.outpost_room_data,
                state_context.home_room_data,
            ).build();

            outpost_room_data.add_mission(mission_entity);

            self.reserve_mission = Some(mission_entity).into();
        }

        let has_defend = self.defend_mission.map(|e| tick_context.system_data.entities.is_alive(e)).unwrap_or(false);

        if !has_defend {
            let outpost_room_data = tick_context.system_data.room_data.get_mut(state_context.outpost_room_data).ok_or("Expected outpost room data")?;

            let mission_entity = DefendMission::build(
                tick_context.system_data.updater.create_entity(tick_context.system_data.entities),
                Some(OperationOrMissionEntity::Mission(tick_context.runtime_data.entity)),
                state_context.outpost_room_data,
                state_context.home_room_data
            ).build();

            outpost_room_data.add_mission(mission_entity);

            self.defend_mission = Some(mission_entity).into();
        }

        Ok(None)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct MiningOutpostMission {
    owner: EntityOption<OperationOrMissionEntity>,
    context: MiningOutpostMissionContext,
    state: MiningOutpostState
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MiningOutpostMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, outpost_room_data: Entity, home_room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = MiningOutpostMission::new(owner, outpost_room_data, home_room_data);

        builder.with(MissionData::MiningOutpost(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, outpost_room_data: Entity, home_room_data: Entity) -> MiningOutpostMission {
        MiningOutpostMission {
            owner: owner.into(),
            context: MiningOutpostMissionContext { 
                home_room_data,
                outpost_room_data
            },
            state: MiningOutpostState::scout(std::marker::PhantomData)
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for MiningOutpostMission {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: OperationOrMissionEntity) {
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
