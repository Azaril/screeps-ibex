use super::data::*;
use super::defend::*;
use super::haul::*;
use super::localsupply::*;
use super::missionsystem::*;
use super::reserve::*;
use super::utility::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use log::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct MiningOutpostMissionContext {
    home_room_datas: EntityVec<Entity>,
    outpost_room_data: Entity,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum MiningOutpostState {
        Scout {
            phantom: std::marker::PhantomData<Entity>
        },
        Mine {
            supply_mission: EntityOption<Entity>,
            haul_mission: EntityOption<Entity>,
            reserve_mission: EntityOption<Entity>,
            defend_mission: EntityOption<Entity>
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &MiningOutpostMissionContext) -> String {
            format!("Mining Outpost - {}", self.status_description())
        }

        _ => fn status_description(&self) -> String;

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
    }
);

fn can_run_mission(
    system_data: &mut MissionExecutionSystemData,
    _mission_entity: Entity,
    state_context: &mut MiningOutpostMissionContext,
) -> Result<bool, String> {
    let derelict_features = system_data.features.derelict;
    let outpost_room_data = system_data
        .room_data
        .get(state_context.outpost_room_data)
        .ok_or("Expected outpost room data")?;

    if let Some(dynamic_visibility_data) = outpost_room_data.get_dynamic_visibility_data() {
        // A confirmed-derelict (hostile-owned but dead) room is minable
        // pre-neutral: harvesting needs no ownership. The Mine phase defers
        // reservation/containers until the controller is neutralized.
        let confirmed_derelict = derelict_features.on
            && dynamic_visibility_data.confirmed_derelict(derelict_features.confirm_ticks, derelict_features.path_max_age);

        if dynamic_visibility_data.updated_within(1000)
            && (!(dynamic_visibility_data.owner().neutral() || confirmed_derelict)
                || dynamic_visibility_data.reservation().hostile()
                || dynamic_visibility_data.reservation().friendly())
        {
            return Ok(false);
        }
    }

    Ok(true)
}

impl Scout {
    fn status_description(&self) -> String {
        "Scout".to_string()
    }

    fn get_children_internal(&self) -> [&Option<Entity>; 0] {
        []
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 0] {
        []
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut MiningOutpostMissionContext,
    ) -> Result<Option<MiningOutpostState>, String> {
        let outpost_room_data = system_data
            .room_data
            .get(state_context.outpost_room_data)
            .ok_or("Expected outpost room data")?;

        if let Some(static_visibility_data) = outpost_room_data.get_static_visibility_data() {
            if static_visibility_data.sources().is_empty() {
                return Err("No sources available for mining outpost, aborting mission.".to_string());
            }
        }

        let dynamic_visibility_data = outpost_room_data.get_dynamic_visibility_data();

        // Keep intel fresh while evaluating — or while waiting out a derelict
        // owner's controller decay below.
        if dynamic_visibility_data.map(|v| !v.updated_within(1000)).unwrap_or(true) {
            system_data.visibility.request(VisibilityRequest::new(
                outpost_room_data.name,
                VISIBILITY_PRIORITY_MEDIUM,
                VisibilityRequestFlags::ALL,
            ));
        }

        let Some(dynamic_visibility_data) = dynamic_visibility_data else {
            return Ok(None);
        };

        if !dynamic_visibility_data.updated_within(1000) {
            return Ok(None);
        }

        if dynamic_visibility_data.reservation().hostile() || dynamic_visibility_data.reservation().friendly() {
            return Err("Mission cannot run in current room state".to_string());
        }

        if dynamic_visibility_data.owner().neutral() {
            info!("Completed scouting of room - transitioning to mining");

            return Ok(Some(MiningOutpostState::mine(None.into(), None.into(), None.into(), None.into())));
        }

        if dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly() {
            return Err("Mission cannot run in current room state".to_string());
        }

        // Hostile owner. A CONFIRMED-derelict one (dead: no spawns / armed
        // towers / threat creeps, held long enough) is minable RIGHT NOW —
        // harvesting needs no ownership. Proceed to Mine for pre-neutral
        // mining; the Mine phase harvests reachable sources and defers
        // reservation + containers until the controller is neutralized (by the
        // salvage de-claim role / natural decay), then upgrades automatically.
        // An armed owner aborts; the operation decides if/when to retry.
        let derelict_features = system_data.features.derelict;

        if derelict_features.on
            && dynamic_visibility_data.confirmed_derelict(derelict_features.confirm_ticks, derelict_features.path_max_age)
        {
            info!(
                "Derelict outpost {} - starting pre-neutral mining (reserve deferred until controller is neutral)",
                outpost_room_data.name
            );

            Ok(Some(MiningOutpostState::mine(None.into(), None.into(), None.into(), None.into())))
        } else {
            Err("Mission cannot run in current room state".to_string())
        }
    }
}

impl Mine {
    fn status_description(&self) -> String {
        let mut parts = Vec::new();
        if self.supply_mission.is_some() {
            parts.push("supply");
        }
        if self.haul_mission.is_some() {
            parts.push("haul");
        }
        if self.reserve_mission.is_some() {
            parts.push("reserve");
        }
        if self.defend_mission.is_some() {
            parts.push("defend");
        }
        if parts.is_empty() {
            "Mine".to_string()
        } else {
            format!("Mine - {}", parts.join(", "))
        }
    }

    fn get_children_internal(&self) -> [&Option<Entity>; 4] {
        [
            &self.supply_mission,
            &self.haul_mission,
            &self.reserve_mission,
            &self.defend_mission,
        ]
    }

    fn get_children_internal_mut(&mut self) -> [&mut Option<Entity>; 4] {
        [
            &mut self.supply_mission,
            &mut self.haul_mission,
            &mut self.reserve_mission,
            &mut self.defend_mission,
        ]
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

        // Reservation requires a NEUTRAL controller (engine: reserveController
        // is rejected on owned controllers). While the room is still
        // hostile-owned-but-derelict we mine pre-neutral (harvesters/haul; no
        // container construction is possible either) and defer the reserver
        // until de-claim/decay neutralizes it — then it is created on a later
        // tick automatically.
        let is_neutral = system_data
            .room_data
            .get(state_context.outpost_room_data)
            .and_then(|rd| rd.get_dynamic_visibility_data())
            .map(|dvd| dvd.owner().neutral())
            .unwrap_or(false);

        if let Some(mut supply_mission) = self
            .supply_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<LocalSupplyMission>()
        {
            supply_mission.set_home_rooms(&state_context.home_room_datas);
        } else if self.supply_mission.is_none() {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = LocalSupplyMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                &state_context.home_room_datas,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.supply_mission = Some(mission_entity).into();
        }

        if let Some(mut haul_mission) = self
            .haul_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<HaulMission>()
        {
            haul_mission.set_home_rooms(&state_context.home_room_datas);
        } else if self.haul_mission.is_none() {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = HaulMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                &state_context.home_room_datas,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.haul_mission = Some(mission_entity).into();
        }

        if let Some(mut reserve_mission) = self
            .reserve_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<ReserveMission>()
        {
            reserve_mission.set_home_rooms(&state_context.home_room_datas);
        } else if is_neutral && self.reserve_mission.is_none() {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = ReserveMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                &state_context.home_room_datas,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.reserve_mission = Some(mission_entity).into();
        }

        if let Some(mut defend_mission) = self
            .defend_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<DefendMission>()
        {
            defend_mission.set_home_rooms(&state_context.home_room_datas);
        } else if self.defend_mission.is_none() {
            let outpost_room_data = system_data
                .room_data
                .get_mut(state_context.outpost_room_data)
                .ok_or("Expected outpost room data")?;

            let mission_entity = DefendMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                state_context.outpost_room_data,
                &state_context.home_room_datas,
            )
            .build();

            outpost_room_data.add_mission(mission_entity);

            self.defend_mission = Some(mission_entity).into();
        }

        let room_is_safe = self
            .defend_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type::<DefendMission>()
            .map(|d| d.is_room_safe())
            .unwrap_or(true);

        if let Some(mut supply_mission) = self
            .supply_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<LocalSupplyMission>()
        {
            supply_mission.allow_spawning(room_is_safe);
        }

        if let Some(mut haul_mission) = self
            .haul_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<HaulMission>()
        {
            haul_mission.allow_spawning(room_is_safe);
        }

        if let Some(mut reserve_mission) = self
            .reserve_mission
            .and_then(|e| system_data.missions.get(e))
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
    pub fn build<B>(builder: B, owner: Option<Entity>, outpost_room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = MiningOutpostMission::new(owner, outpost_room_data, home_room_datas);

        builder
            .with(MissionData::MiningOutpost(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, outpost_room_data: Entity, home_room_datas: &[Entity]) -> MiningOutpostMission {
        MiningOutpostMission {
            owner: owner.into(),
            context: MiningOutpostMissionContext {
                home_room_datas: home_room_datas.to_owned().into(),
                outpost_room_data,
            },
            state: MiningOutpostState::scout(std::marker::PhantomData),
        }
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.context.home_room_datas.as_slice() != home_room_datas {
            self.context.home_room_datas = home_room_datas.to_owned().into();
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

    fn get_room(&self) -> Option<Entity> {
        Some(self.context.outpost_room_data)
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

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Mining Outpost - {}", self.state.status_description()))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        self.context
            .home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).map(is_valid_home_room).unwrap_or(false));

        if self.context.home_room_datas.is_empty() {
            return Err("No home rooms available for mining outpost".to_owned());
        }

        self.state.gather_data(system_data, mission_entity);

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        crate::machine_tick::run_state_machine_result(&mut self.state, "MiningOutpostMission", |state| {
            state.tick(system_data, mission_entity, &mut self.context)
        })?;

        self.state.visualize(system_data, mission_entity);

        Ok(MissionResult::Running)
    }
}
