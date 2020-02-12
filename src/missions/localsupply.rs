use specs::*;
use specs::error::NoError;
use specs::saveload::*;
use screeps::*;
use specs_derive::*;
use serde::{Serialize, Deserialize};
use itertools::*;

use super::data::*;
use super::missionsystem::*;
use ::jobs::data::*;
use ::spawnsystem::*;
use crate::serialize::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct LocalSupplyMission {
    harvesters: EntityVec,
    miners: EntityVec,
    haulers: EntityVec
}

impl LocalSupplyMission
{
    pub fn build<B>(builder: B, room_name: &RoomName) -> B where B: Builder + MarkedBuilder {
        let mission = LocalSupplyMission::new();

        builder.with(MissionData::LocalSupply(mission))
            .marked::<::serialize::SerializeMarker>()
            .with(::room::data::RoomOwnerData::new(room_name))
    }

    pub fn new() -> LocalSupplyMission {
        LocalSupplyMission {
            harvesters: EntityVec::new(),
            miners: EntityVec::new(),
            haulers: EntityVec::new()
        }
    }
}

impl Mission for LocalSupplyMission
{
    fn run_mission<'a>(&mut self, system_data: &MissionExecutionSystemData, runtime_data: &MissionExecutionRuntimeData) -> MissionResult {
        scope_timing!("LocalSupply - Room: {}", runtime_data.room_owner.owner);

        //
        // Cleanup creeps that no longer exist.
        //

        self.harvesters.0.retain(|entity| system_data.entities.is_alive(*entity));
        self.miners.0.retain(|entity| system_data.entities.is_alive(*entity));
        self.haulers.0.retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room) = game::rooms::get(runtime_data.room_owner.owner) {
            let sources = room.find(find::SOURCES);

            //
            // Container mining data gathering.
            //

            let room_containers: Vec<StructureContainer> = room.find(find::STRUCTURES)
                .into_iter()
                .filter_map(|structure| {
                    match structure {
                        Structure::Container(container) => Some(container),
                        _ => None
                    }})
                .collect();

            let mut sources_to_containers = sources.iter()
                .filter_map(|source| {
                    let nearby_container = room_containers.iter().cloned().find(|container| {
                        container.pos().is_near_to(source)
                    });

                    nearby_container.map(|container| (source.id(), container))
                })
                .into_group_map();

            //
            // Creep data gathering.
            //

            //TODO: Store this mapping data as part of the mission. (Blocked on specs collection serialization.)
            let mut sources_to_harvesters = self.harvesters.0.iter()
                .filter_map(|harvester_entity| {
                    if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                        Some((harvester_data.harvest_target, harvester_entity))
                    } else {
                        None
                    }
                })
                .into_group_map();

            let mut sources_to_miners = self.miners.0.iter()
                .filter_map(|miner_entity| {
                    if let Some(JobData::StaticMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                        Some((miner_data.mine_target, (miner_entity, miner_data.mine_location)))
                    } else {
                        None
                    }
                })
                .into_group_map();

            let mut containers_to_haulers = self.haulers.0.iter()
                .filter_map(|hauler_entity| {
                    if let Some(JobData::Haul(hauler_data)) = system_data.job_data.get(*hauler_entity) {
                        Some((hauler_data.primary_container, (hauler_entity, hauler_data.primary_container)))
                    } else {
                        None
                    }
                })
                .into_group_map();

            let total_harvesters = self.harvesters.0.len();
            let total_miners = self.miners.0.len();
            let total_harvesting_creeps = total_harvesters + total_miners;

            for source in sources.iter() {
                let source_id = source.id();

                let source_containers = sources_to_containers.remove(&source_id).unwrap_or_else(Vec::new);
                let source_harvesters = sources_to_harvesters.remove(&source_id).unwrap_or_else(Vec::new);
                let source_miners = sources_to_miners.remove(&source_id).unwrap_or_else(Vec::new);
                let source_haulers: Vec<(&Entity, ObjectId<StructureContainer>)> = source_containers
                    .iter()
                    .flat_map(|container| {
                        containers_to_haulers.remove(&container.id()).unwrap_or_else(Vec::new)
                    })
                    .collect();

                let available_containers_for_miners = source_containers
                    .iter()
                    //TODO: Take in to account miners who are about to expire and should be ignored.
                    .filter(|container| !source_miners.iter().any(|(_, location)| *location == container.pos()))
                    .cloned();

                let available_containers_for_haulers = source_containers
                    .iter()
                    //TODO: Take in to account haulers who are about to expire and should be ignored.
                    .filter(|container| !source_haulers.iter().any(|(_, primary_container)| *primary_container == container.id()))
                    .cloned();

                //
                // Spawn container miners.
                //

                for container in available_containers_for_miners {
                    let priority = SPAWN_PRIORITY_HIGH;

                    let base_body = [Part::Move];

                    let energy_per_tick = (source.energy_capacity() as f32) / (ENERGY_REGEN_TIME as f32);
                    let work_parts_per_tick = energy_per_tick / (HARVEST_POWER as f32);

                    let room_max_energy = room.energy_capacity_available();
                    let base_body_cost: u32 = base_body.iter().map(|p| p.cost()).sum();
                    let work_available_energy: u32 = room_max_energy - base_body_cost;
                    let max_work_parts = (work_available_energy as f32) / (Part::Work.cost() as f32);

                    let spawn_work_parts = std::cmp::min(work_parts_per_tick.ceil() as usize, max_work_parts.floor() as usize);

                    let repeat_body = [Part::Work];
                    let body = repeat_body
                        .iter()
                        .cycle()
                        .take(spawn_work_parts * repeat_body.len())
                        .chain(base_body.iter()).cloned()
                        .collect::<Vec<Part>>();

                    let mission_entity = runtime_data.entity.clone();
                    let source_id = source.id();
                    let mine_location = container.pos();

                    system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                        let name = name.to_string();

                        spawn_system_data.updater.exec_mut(move |world| {
                            let creep_job = JobData::StaticMine(::jobs::staticmine::StaticMineJob::new(source_id, &mine_location));

                            let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                                .with(creep_job)
                                .build();

                            let mission_data_storage = &mut world.write_storage::<MissionData>();

                            if let Some(MissionData::LocalSupply(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                                mission_data.miners.0.push(creep_entity);
                            }       
                        });
                    })));
                }

                //
                // Spawn haulers
                //

                for container in available_containers_for_haulers {
                    let priority = if container.store_used_capacity(Some(ResourceType::Energy)) > 0 {
                        SPAWN_PRIORITY_HIGH
                    } else {
                        SPAWN_PRIORITY_MEDIUM
                    };

                    //TODO: Compute number of work parts needed.
                    let body = [Part::Carry, Part::Carry, Part::Move, Part::Move];

                    let mission_entity = runtime_data.entity.clone();
                    let container_id = container.id();

                    system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                        let name = name.to_string();

                        spawn_system_data.updater.exec_mut(move |world| {
                            let creep_job = JobData::Haul(::jobs::haul::HaulJob::new(container_id));

                            let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                                .with(creep_job)
                                .build();

                            let mission_data_storage = &mut world.write_storage::<MissionData>();

                            if let Some(MissionData::LocalSupply(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                                mission_data.haulers.0.push(creep_entity);
                            }       
                        });
                    })));
                }

                //
                // Spawn harvesters
                //

                //TODO: Compute correct number of harvesters to use for source.
                //TODO: Compute the correct time to spawn emergency harvesters.
                if (source_containers.is_empty() && source_harvesters.len() < 4) || total_harvesting_creeps == 0 {
                    let priority = if total_harvesting_creeps == 0 { SPAWN_PRIORITY_CRITICAL } else { SPAWN_PRIORITY_HIGH };
                    //TODO: Compute best body parts to use.
                    let body = [Part::Move, Part::Move, Part::Carry, Part::Work];

                    let mission_entity = runtime_data.entity.clone();
                    let source_id = source.id();

                    system_data.spawn_queue.request(SpawnRequest::new(&runtime_data.room_owner.owner, &body, priority, Box::new(move |spawn_system_data, name| {
                        let name = name.to_string();

                        spawn_system_data.updater.exec_mut(move |world| {
                            let creep_job = JobData::Harvest(::jobs::harvest::HarvestJob::new(source_id));

                            let creep_entity = ::creep::Spawning::build(world.create_entity(), &name)
                                .with(creep_job)
                                .build();

                            let mission_data_storage = &mut world.write_storage::<MissionData>();

                            if let Some(MissionData::LocalSupply(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                                mission_data.harvesters.0.push(creep_entity);
                            }       
                        });
                    })));
                }
            }

            return MissionResult::Running;
        } else {
            return MissionResult::Failure;
        }
    }
}