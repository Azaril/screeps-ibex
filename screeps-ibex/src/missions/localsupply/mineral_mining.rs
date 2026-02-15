use super::body_helpers::*;
use super::structure_data::*;
use crate::jobs::data::*;
use crate::jobs::staticmine::*;
use crate::missions::data::*;
use crate::missions::missionsystem::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use itertools::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

pub struct MineralMiningMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    mineral: RemoteObjectId<Mineral>,
    extractor: RemoteObjectId<StructureExtractor>,
    container_miners: EntityVec<Entity>,
    room_name: RoomName,
    allow_spawning: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(bound = "MA: Marker")]
pub struct MineralMiningMissionSaveloadData<MA>
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    owner: <EntityOption<Entity> as ConvertSaveload<MA>>::Data,
    room_data: <Entity as ConvertSaveload<MA>>::Data,
    home_room_datas: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    mineral: <RemoteObjectId<Mineral> as ConvertSaveload<MA>>::Data,
    extractor: <RemoteObjectId<StructureExtractor> as ConvertSaveload<MA>>::Data,
    container_miners: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    room_name: <RoomName as ConvertSaveload<MA>>::Data,
    allow_spawning: <bool as ConvertSaveload<MA>>::Data,
}

impl<MA> ConvertSaveload<MA> for MineralMiningMission
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    type Data = MineralMiningMissionSaveloadData<MA>;
    #[allow(deprecated)]
    type Error = NoError;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<MA>,
    {
        Ok(MineralMiningMissionSaveloadData {
            owner: ConvertSaveload::convert_into(&self.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_into(&self.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_into(&self.home_room_datas, &mut ids)?,
            mineral: ConvertSaveload::convert_into(&self.mineral, &mut ids)?,
            extractor: ConvertSaveload::convert_into(&self.extractor, &mut ids)?,
            container_miners: ConvertSaveload::convert_into(&self.container_miners, &mut ids)?,
            room_name: ConvertSaveload::convert_into(&self.room_name, &mut ids)?,
            allow_spawning: ConvertSaveload::convert_into(&self.allow_spawning, &mut ids)?,
        })
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(MA) -> Option<Entity>,
    {
        Ok(MineralMiningMission {
            owner: ConvertSaveload::convert_from(data.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_from(data.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_from(data.home_room_datas, &mut ids)?,
            mineral: ConvertSaveload::convert_from(data.mineral, &mut ids)?,
            extractor: ConvertSaveload::convert_from(data.extractor, &mut ids)?,
            container_miners: ConvertSaveload::convert_from(data.container_miners, &mut ids)?,
            room_name: ConvertSaveload::convert_from(data.room_name, &mut ids)?,
            allow_spawning: ConvertSaveload::convert_from(data.allow_spawning, &mut ids)?,
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MineralMiningMission {
    pub fn build<B>(
        builder: B,
        owner: Option<Entity>,
        room_data: Entity,
        home_room_datas: &[Entity],
        mineral: RemoteObjectId<Mineral>,
        extractor: RemoteObjectId<StructureExtractor>,
        room_name: RoomName,
    ) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = MineralMiningMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            mineral,
            extractor,
            container_miners: EntityVec::new(),
            room_name,
            allow_spawning: true,
        };

        builder
            .with(MissionData::MineralMining(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn mineral(&self) -> &RemoteObjectId<Mineral> {
        &self.mineral
    }

    pub fn extractor(&self) -> &RemoteObjectId<StructureExtractor> {
        &self.extractor
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.home_room_datas.as_slice() != home_room_datas {
            self.home_room_datas = home_room_datas.to_owned().into();
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow;
    }

    fn create_handle_container_miner_spawn(
        mission_entity: Entity,
        mineral_id: RemoteObjectId<Mineral>,
        extractor_id: RemoteObjectId<StructureExtractor>,
        container_id: RemoteObjectId<StructureContainer>,
    ) -> SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::StaticMine(StaticMineJob::new(
                    StaticMineTarget::Mineral(mineral_id, extractor_id),
                    container_id,
                ));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<MineralMiningMission>()
                {
                    mission_data.container_miners.push(creep_entity);
                }
            });
        })
    }

    fn spawn_creeps(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

        let structure_data_rc = system_data.supply_structure_cache.get_room(self.room_name);
        let mut structure_data = structure_data_rc.maybe_access(
            |d| game::time() - d.last_updated >= 10 && has_visibility,
            || create_structure_data(room_data),
        );

        if structure_data.get().is_none() {
            return Ok(());
        }

        let structure_data = structure_data.get().ok_or("Expected structure data")?;

        // Check if mineral has resources.
        if let Some(mineral) = self.mineral.resolve() {
            if mineral.mineral_amount() == 0 {
                return Ok(());
            }
        }

        let mineral_extractor_pair = (self.mineral, self.extractor);
        let container_ids = structure_data
            .mineral_extractors_to_containers
            .get(&mineral_extractor_pair)
            .map(|c| c.as_slice())
            .unwrap_or(&[]);

        if container_ids.is_empty() {
            return Ok(());
        }

        let containers_to_miners = self
            .container_miners
            .iter()
            .filter_map(|miner_entity| {
                if let Some(JobData::StaticMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    Some((miner_data.context.container_target, *miner_entity))
                } else {
                    None
                }
            })
            .into_group_map();

        let mineral_miners: Vec<_> = container_ids
            .iter()
            .filter_map(|container| containers_to_miners.get(container))
            .flat_map(|m| m.iter())
            .collect();

        // Compute replacement lead time.
        let mineral_replacement_lead = self
            .home_room_datas
            .iter()
            .filter_map(|home_room_entity| {
                let home_room_data = system_data.room_data.get(*home_room_entity)?;
                let home_room = game::rooms().get(home_room_data.name)?;
                let is_local = self.mineral.pos().room_name() == home_room_data.name;

                let body_definition = mineral_miner_body(is_local, home_room.energy_capacity_available());
                let body = crate::creep::spawning::create_body(&body_definition).ok()?;
                Some(miner_lead_ticks(&body, self.mineral.pos(), structure_data))
            })
            .min()
            .unwrap_or(MIN_REPLACEMENT_LEAD_TICKS)
            .max(MIN_REPLACEMENT_LEAD_TICKS);

        let alive_mineral_miners: Vec<_> = mineral_miners
            .iter()
            .filter(|entity| {
                system_data.creep_spawning.get(***entity).is_some()
                    || system_data
                        .creep_owner
                        .get(***entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live())
                        .map(|count| count > mineral_replacement_lead)
                        .unwrap_or(false)
            })
            .map(|entity| **entity)
            .collect();

        let available_containers = container_ids.iter().filter(|container| {
            containers_to_miners
                .get(container)
                .map(|miners| !miners.iter().any(|miner| alive_mineral_miners.contains(miner)))
                .unwrap_or(true)
        });

        for container in available_containers {
            let token = system_data.spawn_queue.token();

            for home_room_entity in self.home_room_datas.iter() {
                let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

                let is_local = container.pos().room_name() == home_room_data.name;
                let body_definition = mineral_miner_body(is_local, home_room.energy_capacity_available());

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let spawn_request = SpawnRequest::new(
                        format!("Container Miner - Extractor: {}", self.extractor.id()),
                        &body,
                        SPAWN_PRIORITY_LOW,
                        Some(token),
                        Self::create_handle_container_miner_spawn(mission_entity, self.mineral, self.extractor, *container),
                    );

                    system_data.spawn_queue.request(*home_room_entity, spawn_request);
                }
            }
        }

        Ok(())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for MineralMiningMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn remove_creep(&mut self, entity: Entity) {
        self.container_miners.retain(|e| *e != entity);
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Mineral Mining - Miners: {}", self.container_miners.len())
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Mineral Mining ({})", self.container_miners.len()))
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        if self.allow_spawning {
            self.spawn_creeps(system_data, mission_entity)?;
        }

        Ok(MissionResult::Running)
    }
}
