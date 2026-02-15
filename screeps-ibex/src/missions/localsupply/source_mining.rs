use super::body_helpers::*;
use super::structure_data::*;
use crate::jobs::data::*;
use crate::jobs::harvest::*;
use crate::jobs::linkmine::*;
use crate::jobs::staticmine::*;
use crate::missions::data::*;
use crate::missions::missionsystem::*;
use crate::remoteobjectid::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use itertools::*;
use lerp::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

pub struct SourceMiningMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    source: RemoteObjectId<Source>,
    link_miners: EntityVec<Entity>,
    container_miners: EntityVec<Entity>,
    harvesters: EntityVec<Entity>,
    room_name: RoomName,
    allow_spawning: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(bound = "MA: Marker")]
pub struct SourceMiningMissionSaveloadData<MA>
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    owner: <EntityOption<Entity> as ConvertSaveload<MA>>::Data,
    room_data: <Entity as ConvertSaveload<MA>>::Data,
    home_room_datas: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    source: <RemoteObjectId<Source> as ConvertSaveload<MA>>::Data,
    link_miners: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    container_miners: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    harvesters: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    room_name: <RoomName as ConvertSaveload<MA>>::Data,
    allow_spawning: <bool as ConvertSaveload<MA>>::Data,
}

impl<MA> ConvertSaveload<MA> for SourceMiningMission
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    type Data = SourceMiningMissionSaveloadData<MA>;
    #[allow(deprecated)]
    type Error = NoError;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<MA>,
    {
        Ok(SourceMiningMissionSaveloadData {
            owner: ConvertSaveload::convert_into(&self.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_into(&self.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_into(&self.home_room_datas, &mut ids)?,
            source: ConvertSaveload::convert_into(&self.source, &mut ids)?,
            link_miners: ConvertSaveload::convert_into(&self.link_miners, &mut ids)?,
            container_miners: ConvertSaveload::convert_into(&self.container_miners, &mut ids)?,
            harvesters: ConvertSaveload::convert_into(&self.harvesters, &mut ids)?,
            room_name: ConvertSaveload::convert_into(&self.room_name, &mut ids)?,
            allow_spawning: ConvertSaveload::convert_into(&self.allow_spawning, &mut ids)?,
        })
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(MA) -> Option<Entity>,
    {
        Ok(SourceMiningMission {
            owner: ConvertSaveload::convert_from(data.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_from(data.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_from(data.home_room_datas, &mut ids)?,
            source: ConvertSaveload::convert_from(data.source, &mut ids)?,
            link_miners: ConvertSaveload::convert_from(data.link_miners, &mut ids)?,
            container_miners: ConvertSaveload::convert_from(data.container_miners, &mut ids)?,
            harvesters: ConvertSaveload::convert_from(data.harvesters, &mut ids)?,
            room_name: ConvertSaveload::convert_from(data.room_name, &mut ids)?,
            allow_spawning: ConvertSaveload::convert_from(data.allow_spawning, &mut ids)?,
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SourceMiningMission {
    pub fn build<B>(
        builder: B,
        owner: Option<Entity>,
        room_data: Entity,
        home_room_datas: &[Entity],
        source: RemoteObjectId<Source>,
        room_name: RoomName,
    ) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SourceMiningMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            source,
            link_miners: EntityVec::new(),
            container_miners: EntityVec::new(),
            harvesters: EntityVec::new(),
            room_name,
            allow_spawning: true,
        };

        builder
            .with(MissionData::SourceMining(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn source(&self) -> &RemoteObjectId<Source> {
        &self.source
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.home_room_datas.as_slice() != home_room_datas {
            self.home_room_datas = home_room_datas.to_owned().into();
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow;
    }

    fn create_handle_link_miner_spawn(
        mission_entity: Entity,
        source_id: RemoteObjectId<Source>,
        link_id: RemoteObjectId<StructureLink>,
        container_id: Option<RemoteObjectId<StructureContainer>>,
    ) -> SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::LinkMine(LinkMineJob::new(source_id, link_id, container_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<SourceMiningMission>()
                {
                    mission_data.link_miners.push(creep_entity);
                }
            });
        })
    }

    fn create_handle_container_miner_spawn(
        mission_entity: Entity,
        source_id: RemoteObjectId<Source>,
        container_id: RemoteObjectId<StructureContainer>,
    ) -> SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::StaticMine(StaticMineJob::new(StaticMineTarget::Source(source_id), container_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<SourceMiningMission>()
                {
                    mission_data.container_miners.push(creep_entity);
                }
            });
        })
    }

    fn create_handle_harvester_spawn(
        mission_entity: Entity,
        source_id: RemoteObjectId<Source>,
        delivery_room: Entity,
    ) -> SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Harvest(HarvestJob::new(source_id, delivery_room, true));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<SourceMiningMission>()
                {
                    mission_data.harvesters.push(creep_entity);
                }
            });
        })
    }

    fn spawn_creeps(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility")?;
        let likely_owned_room = dynamic_visibility_data.updated_within(2000)
            && (dynamic_visibility_data.owner().mine() || dynamic_visibility_data.reservation().mine());
        let has_visibility = dynamic_visibility_data.visible();

        let structure_data_rc = system_data.supply_structure_cache.get_room(self.room_name);
        let mut structure_data = structure_data_rc.maybe_access(
            |d| game::time() - d.last_updated >= 10 && has_visibility,
            || create_structure_data(room_data),
        );

        if structure_data.get().is_none() {
            system_data.visibility.request(VisibilityRequest::new(
                room_data.name,
                VISIBILITY_PRIORITY_CRITICAL,
                VisibilityRequestFlags::ALL,
            ));
            return Ok(());
        }

        let structure_data = structure_data.get().ok_or("Expected structure data")?;

        let source_id = &self.source;

        let source_containers = structure_data
            .sources_to_containers
            .get(source_id)
            .map(|c| c.as_slice())
            .unwrap_or(&[]);

        let source_links = structure_data.sources_to_links.get(source_id).map(|c| c.as_slice()).unwrap_or(&[]);

        // Gather creep data for this source.
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

        let links_to_miners = self
            .link_miners
            .iter()
            .filter_map(|miner_entity| {
                if let Some(JobData::LinkMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    Some((*miner_data.get_link_target(), *miner_entity))
                } else {
                    None
                }
            })
            .into_group_map();

        let source_container_miners: Vec<_> = source_containers
            .iter()
            .filter_map(|container| containers_to_miners.get(container))
            .flat_map(|m| m.iter())
            .collect();

        let source_link_miners: Vec<_> = source_links
            .iter()
            .filter_map(|link| links_to_miners.get(link))
            .flat_map(|m| m.iter())
            .collect();

        let total_harvesting_creeps = self.harvesters.len() + self.container_miners.len() + self.link_miners.len();

        // Compute replacement lead time.
        let work_parts = source_work_parts(likely_owned_room);

        let miner_replacement_lead = self
            .home_room_datas
            .iter()
            .filter_map(|home_room_entity| {
                let home_room_data = system_data.room_data.get(*home_room_entity)?;
                let home_room = game::rooms().get(home_room_data.name)?;
                let is_local = source_id.pos().room_name() == home_room_data.name;
                let has_link = !source_links.is_empty();

                let body_definition = source_miner_body(is_local, home_room.energy_capacity_available(), work_parts, has_link);
                let body = crate::creep::spawning::create_body(&body_definition).ok()?;
                Some(miner_lead_ticks(&body, source_id.pos(), structure_data))
            })
            .min()
            .unwrap_or(MIN_REPLACEMENT_LEAD_TICKS)
            .max(MIN_REPLACEMENT_LEAD_TICKS);

        let alive_miners: Vec<_> = source_container_miners
            .iter()
            .chain(source_link_miners.iter())
            .filter(|entity| {
                system_data.creep_spawning.get(***entity).is_some()
                    || system_data
                        .creep_owner
                        .get(***entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live())
                        .map(|count| count > miner_replacement_lead)
                        .unwrap_or(false)
            })
            .map(|entity| **entity)
            .collect();

        // Determine home room properties.
        let any_home_room_has_storage = self
            .home_room_datas
            .iter()
            .filter_map(|home_room_entity| {
                let home_room_data = system_data.room_data.get(*home_room_entity)?;
                let home_room_structures = home_room_data.get_structures()?;
                Some(!home_room_structures.storages().is_empty())
            })
            .any(|has_storage| has_storage);

        let min_home_room_distance = self
            .home_room_datas
            .iter()
            .filter_map(|home_room_entity| {
                let home_room_data = system_data.room_data.get(*home_room_entity)?;
                let room_offset_distance = home_room_data.name - source_id.pos().room_name();
                let room_manhattan_distance = room_offset_distance.0.abs() + room_offset_distance.1.abs();
                Some(room_manhattan_distance)
            })
            .min()
            .unwrap_or(0);

        //
        // Spawn harvesters (fallback when no containers/links, or bootstrapping).
        //
        let source_harvesters: Vec<_> = self
            .harvesters
            .iter()
            .filter_map(|harvester_entity| {
                if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                    if *harvester_data.harvest_target() == *source_id {
                        return Some(*harvester_entity);
                    }
                }
                None
            })
            .collect();

        for home_room_entity in self.home_room_datas.iter() {
            let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
            let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

            let room_offset_distance = home_room_data.name - source_id.pos().room_name();
            let room_manhattan_distance = room_offset_distance.0.abs() + room_offset_distance.1.abs();

            if (source_containers.is_empty() && source_links.is_empty())
                || (room_manhattan_distance == 0 && total_harvesting_creeps == 0)
                || (room_manhattan_distance > 0 && !any_home_room_has_storage)
            {
                let home_rooms_to_harvesters: Vec<_> = self
                    .harvesters
                    .iter()
                    .filter_map(|harvester_entity| {
                        if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                            if harvester_data.delivery_room() == *home_room_entity {
                                return Some(*harvester_entity);
                            }
                        }
                        None
                    })
                    .collect();

                let current_source_room_harvesters = home_rooms_to_harvesters.iter().filter(|e| source_harvesters.contains(e)).count();

                //TODO: Compute correct number of harvesters to use for source.
                let desired_harvesters = 4;

                if current_source_room_harvesters < desired_harvesters {
                    let body_definition = harvester_body(if total_harvesting_creeps == 0 {
                        home_room.energy_available().max(SPAWN_ENERGY_CAPACITY)
                    } else {
                        home_room.energy_capacity_available()
                    });

                    if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                        let priority_range = if room_manhattan_distance == 0 {
                            (SPAWN_PRIORITY_CRITICAL, SPAWN_PRIORITY_HIGH)
                        } else if room_manhattan_distance <= 1 {
                            (SPAWN_PRIORITY_MEDIUM, SPAWN_PRIORITY_NONE)
                        } else {
                            (SPAWN_PRIORITY_LOW, SPAWN_PRIORITY_NONE)
                        };

                        let interp = (current_source_room_harvesters as f32) / (desired_harvesters as f32);
                        let priority = priority_range.0.lerp_bounded(priority_range.1, interp);

                        let spawn_request = SpawnRequest::new(
                            format!("Harvester - Source: {}", source_id.id()),
                            &body,
                            priority,
                            None,
                            Self::create_handle_harvester_spawn(mission_entity, *source_id, *home_room_entity),
                        );

                        system_data.spawn_queue.request(*home_room_entity, spawn_request);
                    }
                }
            }
        }

        //
        // Spawn link miners.
        //
        if !source_links.is_empty() {
            let mut available_containers = source_containers.iter().filter(|container| {
                containers_to_miners
                    .get(container)
                    .map(|miners| !miners.iter().any(|miner| alive_miners.contains(miner)))
                    .unwrap_or(true)
            });

            let available_links = source_links.iter().filter(|link| {
                links_to_miners
                    .get(link)
                    .map(|miners| !miners.iter().any(|miner| alive_miners.contains(miner)))
                    .unwrap_or(true)
            });

            for link in available_links {
                let token = system_data.spawn_queue.token();

                for home_room_entity in self.home_room_datas.iter() {
                    let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                    let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

                    let is_local = link.pos().room_name() == home_room_data.name;
                    let body_definition = source_miner_body(is_local, home_room.energy_capacity_available(), work_parts, true);

                    if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                        let target_container = available_containers.next();

                        let spawn_request = SpawnRequest::new(
                            format!("Link Miner - Source: {}", source_id.id()),
                            &body,
                            SPAWN_PRIORITY_HIGH,
                            Some(token),
                            Self::create_handle_link_miner_spawn(mission_entity, *source_id, *link, target_container.cloned()),
                        );

                        system_data.spawn_queue.request(*home_room_entity, spawn_request);
                    }
                }
            }
        } else if !source_containers.is_empty() && (min_home_room_distance == 0 || any_home_room_has_storage) {
            //
            // Spawn container miners.
            //
            let available_containers = source_containers.iter().filter(|container| {
                containers_to_miners
                    .get(container)
                    .map(|miners| !miners.iter().any(|miner| alive_miners.contains(miner)))
                    .unwrap_or(true)
            });

            for container in available_containers {
                let token = system_data.spawn_queue.token();

                for home_room_entity in self.home_room_datas.iter() {
                    let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                    let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

                    let is_local = container.pos().room_name() == home_room_data.name;
                    let body_definition = source_miner_body(is_local, home_room.energy_capacity_available(), work_parts, false);

                    if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                        let spawn_request = SpawnRequest::new(
                            format!("Container Miner - Source: {}", source_id.id()),
                            &body,
                            SPAWN_PRIORITY_HIGH,
                            Some(token),
                            Self::create_handle_container_miner_spawn(mission_entity, *source_id, *container),
                        );

                        system_data.spawn_queue.request(*home_room_entity, spawn_request);
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SourceMiningMission {
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

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!(
            "Source Mining - Link: {} Container: {} Harvest: {}",
            self.link_miners.len(),
            self.container_miners.len(),
            self.harvesters.len()
        )
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!(
            "Source Mining (L:{} C:{} H:{})",
            self.link_miners.len(),
            self.container_miners.len(),
            self.harvesters.len()
        ))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        self.harvesters
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        self.container_miners
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        self.link_miners
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        if self.allow_spawning {
            self.spawn_creeps(system_data, mission_entity)?;
        }

        Ok(MissionResult::Running)
    }
}
