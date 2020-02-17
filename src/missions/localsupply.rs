use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::missionsystem::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use jobs::data::*;
use spawnsystem::*;

#[derive(Clone, Debug, ConvertSaveload)]
pub struct LocalSupplyMission {
    room_data: Entity,
    harvesters: EntityVec,
    miners: EntityVec,
    haulers: EntityVec,
}

impl LocalSupplyMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalSupplyMission::new(room_data);

        builder
            .with(MissionData::LocalSupply(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> LocalSupplyMission {
        LocalSupplyMission {
            room_data,
            harvesters: EntityVec::new(),
            miners: EntityVec::new(),
            haulers: EntityVec::new(),
        }
    }
}

impl Mission for LocalSupplyMission {
    fn run_mission<'a>(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &MissionExecutionRuntimeData,
    ) -> MissionResult {
        scope_timing!("LocalSupplyMission");

        //
        // Cleanup creeps that no longer exist.
        //

        self.harvesters
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));
        self.miners
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));
        self.haulers
            .0
            .retain(|entity| system_data.entities.is_alive(*entity));

        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            if let Some(room) = game::rooms::get(room_data.name) {
                let mut sources = room.find(find::SOURCES);

                //
                // Container mining data gathering.
                //

                let room_containers: Vec<StructureContainer> = room
                    .find(find::STRUCTURES)
                    .into_iter()
                    .filter_map(|structure| match structure {
                        Structure::Container(container) => Some(container),
                        _ => None,
                    })
                    .collect();

                let mut sources_to_containers = sources
                    .iter()
                    .filter_map(|source| {
                        let nearby_container = room_containers
                            .iter()
                            .cloned()
                            .find(|container| container.pos().is_near_to(source));

                        nearby_container.map(|container| (source.remote_id(), container))
                    })
                    .into_group_map();

                //
                // Creep data gathering.
                //

                //TODO: Store this mapping data as part of the mission. (Blocked on specs collection serialization.)
                let mut sources_to_harvesters = self
                    .harvesters
                    .0
                    .iter()
                    .filter_map(|harvester_entity| {
                        if let Some(JobData::Harvest(harvester_data)) =
                            system_data.job_data.get(*harvester_entity)
                        {
                            Some((harvester_data.harvest_target, harvester_entity))
                        } else {
                            None
                        }
                    })
                    .into_group_map();

                let mut sources_to_miners = self
                    .miners
                    .0
                    .iter()
                    .filter_map(|miner_entity| {
                        if let Some(JobData::StaticMine(miner_data)) =
                            system_data.job_data.get(*miner_entity)
                        {
                            Some((
                                miner_data.mine_target,
                                (miner_entity, miner_data.mine_location),
                            ))
                        } else {
                            None
                        }
                    })
                    .into_group_map();

                let mut containers_to_haulers = self
                    .haulers
                    .0
                    .iter()
                    .filter_map(|hauler_entity| {
                        if let Some(JobData::Haul(hauler_data)) =
                            system_data.job_data.get(*hauler_entity)
                        {
                            Some((
                                hauler_data.primary_container,
                                (hauler_entity, hauler_data.primary_container.id()),
                            ))
                        } else {
                            None
                        }
                    })
                    .into_group_map();

                //
                // Sort sources so requests with equal priority go to the source with the least activity.
                //

                let total_harvesters = self.harvesters.0.len();
                let total_miners = self.miners.0.len();
                let total_haulers = self.haulers.0.len();
                let total_harvesting_creeps = total_harvesters + total_miners;

                sources.sort_by_cached_key(|source| {
                    let source_id = source.remote_id();
                    let source_harvesters = sources_to_harvesters.get(&source_id).map(|harvesters| harvesters.len()).unwrap_or(0);
                    let source_miners = sources_to_miners.get(&source_id).map(|miners| miners.len()).unwrap_or(0);

                    source_harvesters + source_miners
                });
                
                //
                // Spawn needed creeps for each source.
                //

                for source in sources.iter() {
                    let source_id = source.remote_id();

                    let source_containers = sources_to_containers
                        .remove(&source_id)
                        .unwrap_or_else(Vec::new);
                    let source_harvesters = sources_to_harvesters
                        .remove(&source_id)
                        .unwrap_or_else(Vec::new);
                    let source_miners = sources_to_miners
                        .remove(&source_id)
                        .unwrap_or_else(Vec::new);
                    let source_miners_count = source_miners.len();
                    let source_haulers: Vec<(&Entity, ObjectId<StructureContainer>)> =
                        source_containers
                            .iter()
                            .flat_map(|container| {
                                containers_to_haulers
                                    .remove(&container.remote_id())
                                    .unwrap_or_else(Vec::new)
                            })
                            .collect();

                    let alive_source_miners: Vec<(&Entity, RoomPosition)> = source_miners
                        .into_iter()
                        .filter(|(&miner_entity, _)| {
                            if let Some(creep_owner) = system_data.creep_owner.get(miner_entity) {
                                if let Some(creep) = creep_owner.owner.resolve() {
                                    creep.ticks_to_live().unwrap_or(0) > 100
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        })
                        .collect();

                    let alive_source_haulers: Vec<(&Entity, ObjectId<StructureContainer>)> =
                        source_haulers
                            .into_iter()
                            .filter(|(&hauler_entity, _)| {
                                if let Some(creep_owner) =
                                    system_data.creep_owner.get(hauler_entity)
                                {
                                    if let Some(creep) = creep_owner.owner.resolve() {
                                        creep.ticks_to_live().unwrap_or(0) > 100
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            })
                            .collect();

                    let available_containers_for_miners = source_containers
                        .iter()
                        .filter(|container| {
                            !alive_source_miners
                                .iter()
                                .any(|(_, location)| *location == container.pos())
                        })
                        .cloned();

                    let available_containers_for_haulers = source_containers
                        .iter()
                        .filter(|container| {
                            !alive_source_haulers
                                .iter()
                                .any(|(_, primary_container)| *primary_container == container.id())
                        })
                        .cloned();

                    //
                    // Spawn container miners.
                    //

                    for container in available_containers_for_miners {
                        let energy_per_tick =
                            (source.energy_capacity() as f32) / (ENERGY_REGEN_TIME as f32);
                        let work_parts_per_tick =
                            (energy_per_tick / (HARVEST_POWER as f32)).ceil() as usize;

                        let body_definition = crate::creep::SpawnBodyDefinition {
                            maximum_energy: room.energy_capacity_available(),
                            minimum_repeat: Some(1),
                            maximum_repeat: Some(work_parts_per_tick),
                            pre_body: &[Part::Move],
                            repeat_body: &[Part::Work],
                            post_body: &[],
                        };

                        if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                            let mission_entity = *runtime_data.entity;
                            let source_id = source.remote_id();
                            let mine_location = container.pos();

                            let priority = SPAWN_PRIORITY_HIGH;

                            system_data.spawn_queue.request(SpawnRequest::new(
                                room_data.name,
                                &body,
                                priority,
                                Box::new(move |spawn_system_data, name| {
                                    let name = name.to_string();

                                    spawn_system_data.updater.exec_mut(move |world| {
                                        let creep_job = JobData::StaticMine(
                                            ::jobs::staticmine::StaticMineJob::new(
                                                source_id,
                                                mine_location,
                                            ),
                                        );

                                        let creep_entity =
                                            ::creep::Spawning::build(world.create_entity(), &name)
                                                .with(creep_job)
                                                .build();

                                        let mission_data_storage =
                                            &mut world.write_storage::<MissionData>();

                                        if let Some(MissionData::LocalSupply(mission_data)) =
                                            mission_data_storage.get_mut(mission_entity)
                                        {
                                            mission_data.miners.0.push(creep_entity);
                                        }
                                    });
                                }),
                            ));
                        }
                    }

                    //
                    // Spawn haulers
                    //

                    for container in available_containers_for_haulers {
                        let body_definition = crate::creep::SpawnBodyDefinition {
                            maximum_energy: if total_haulers == 0 {
                                room.energy_available()
                            } else {
                                room.energy_capacity_available()
                            },
                            minimum_repeat: Some(1),
                            maximum_repeat: Some(5),
                            pre_body: &[],
                            repeat_body: &[Part::Carry, Part::Move],
                            post_body: &[],
                        };

                        if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                            let mission_entity = *runtime_data.entity;
                            let container_id = container.remote_id();
                            let home_room = room_data.name;

                            let container_used_capacity =
                                container.store_used_capacity(Some(ResourceType::Energy));
                            let container_store_capacity =
                                container.store_capacity(Some(ResourceType::Energy));

                            let storage_fraction = (container_used_capacity as f32)
                                / (container_store_capacity as f32);

                            let priority = if storage_fraction > 0.75 {
                                SPAWN_PRIORITY_CRITICAL
                            } else if source_miners_count > 0 {
                                SPAWN_PRIORITY_HIGH
                            } else {
                                SPAWN_PRIORITY_MEDIUM
                            };

                            system_data.spawn_queue.request(SpawnRequest::new(
                                room_data.name,
                                &body,
                                priority,
                                Box::new(move |spawn_system_data, name| {
                                    let name = name.to_string();

                                    spawn_system_data.updater.exec_mut(move |world| {
                                        let creep_job =
                                            JobData::Haul(::jobs::haul::HaulJob::new(container_id, home_room));

                                        let creep_entity =
                                            ::creep::Spawning::build(world.create_entity(), &name)
                                                .with(creep_job)
                                                .build();

                                        let mission_data_storage =
                                            &mut world.write_storage::<MissionData>();

                                        if let Some(MissionData::LocalSupply(mission_data)) =
                                            mission_data_storage.get_mut(mission_entity)
                                        {
                                            mission_data.haulers.0.push(creep_entity);
                                        }
                                    });
                                }),
                            ));
                        }
                    }

                    //
                    // Spawn harvesters
                    //

                    //TODO: Compute correct number of harvesters to use for source.
                    //TODO: Compute the correct time to spawn emergency harvesters.
                    if (source_containers.is_empty() && source_harvesters.len() < 4)
                        || total_harvesting_creeps == 0
                    {
                        //TODO: Compute best body parts to use.
                        let body_definition = crate::creep::SpawnBodyDefinition {
                            maximum_energy: if total_harvesting_creeps == 0 {
                                room.energy_available()
                            } else {
                                room.energy_capacity_available()
                            },
                            minimum_repeat: Some(1),
                            maximum_repeat: Some(5),
                            pre_body: &[],
                            repeat_body: &[Part::Move, Part::Move, Part::Carry, Part::Work],
                            post_body: &[],
                        };

                        if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                            let priority = if total_harvesting_creeps == 0 {
                                SPAWN_PRIORITY_CRITICAL
                            } else {
                                SPAWN_PRIORITY_HIGH
                            };

                            let mission_entity = *runtime_data.entity;
                            let delivery_room = room_data.name;
                            let source_id = source.remote_id();

                            system_data.spawn_queue.request(SpawnRequest::new(
                                room_data.name,
                                &body,
                                priority,
                                Box::new(move |spawn_system_data, name| {
                                    let name = name.to_string();

                                    spawn_system_data.updater.exec_mut(move |world| {
                                        let creep_job =
                                            JobData::Harvest(::jobs::harvest::HarvestJob::new(
                                                source_id,
                                                delivery_room,
                                            ));

                                        let creep_entity =
                                            ::creep::Spawning::build(world.create_entity(), &name)
                                                .with(creep_job)
                                                .build();

                                        let mission_data_storage =
                                            &mut world.write_storage::<MissionData>();

                                        if let Some(MissionData::LocalSupply(mission_data)) =
                                            mission_data_storage.get_mut(mission_entity)
                                        {
                                            mission_data.harvesters.0.push(creep_entity);
                                        }
                                    });
                                }),
                            ));
                        }
                    }
                }

                MissionResult::Running
            } else {
                MissionResult::Failure
            }
        } else {
            MissionResult::Failure
        }
    }
}
