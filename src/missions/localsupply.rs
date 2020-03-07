use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
use std::collections::HashMap;
#[cfg(feature = "time")]
use timing_annotate::*;

use super::data::*;
use super::missionsystem::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use crate::jobs::staticmine::*;
use jobs::data::*;
use spawnsystem::*;

#[derive(Clone, ConvertSaveload)]
pub struct LocalSupplyMission {
    room_data: Entity,
    harvesters: EntityVec,
    source_miners: EntityVec,
    mineral_miners: EntityVec
}

type MineralExtractorPair = (RemoteObjectId<Mineral>, RemoteObjectId<StructureExtractor>);

struct StructureData {
    sources_to_containers: HashMap<RemoteObjectId<Source>, Vec<RemoteObjectId<StructureContainer>>>,
    sources_to_links: HashMap<RemoteObjectId<Source>, Vec<RemoteObjectId<StructureLink>>>,
    mineral_extractors_to_containers: HashMap<MineralExtractorPair, Vec<RemoteObjectId<StructureContainer>>>,
    controllers_to_containers: HashMap<RemoteObjectId<StructureController>, Vec<RemoteObjectId<StructureContainer>>>,
    containers: Vec<RemoteObjectId<StructureContainer>>,
}

struct CreepData {
    sources_to_harvesters: HashMap<RemoteObjectId<Source>, Vec<Entity>>,
    containers_to_source_miners: HashMap<RemoteObjectId<StructureContainer>, Vec<Entity>>,
    containers_to_mineral_miners: HashMap<RemoteObjectId<StructureContainer>, Vec<Entity>>
}

#[cfg_attr(feature = "time", timing)]
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
            source_miners: EntityVec::new(),
            mineral_miners: EntityVec::new(),
        }
    }

    fn create_handle_container_miner_spawn(
        mission_entity: Entity,
        target: StaticMineTarget,
        container_id: RemoteObjectId<StructureContainer>
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::StaticMine(::jobs::staticmine::StaticMineJob::new(target, container_id));

                let creep_entity = ::creep::Spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::LocalSupply(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    match target {
                        StaticMineTarget::Source(_) => mission_data.source_miners.0.push(creep_entity),
                        StaticMineTarget::Mineral(_, _) => mission_data.mineral_miners.0.push(creep_entity)
                    }
                }
            });
        })
    }

    fn create_handle_harvester_spawn(
        mission_entity: Entity,
        source_id: RemoteObjectId<Source>,
        delivery_room: Entity,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str) + Send + Sync> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Harvest(::jobs::harvest::HarvestJob::new(source_id, delivery_room, true));

                let creep_entity = ::creep::Spawning::build(world.create_entity(), &name).with(creep_job).build();

                let mission_data_storage = &mut world.write_storage::<MissionData>();

                if let Some(MissionData::LocalSupply(mission_data)) = mission_data_storage.get_mut(mission_entity) {
                    mission_data.harvesters.0.push(creep_entity);
                }
            });
        })
    }

    fn create_structure_data(room_data: &RoomData, room: &Room) -> Result<StructureData, String> {
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;
        
        let sources = static_visibility_data.sources();
        let controller = static_visibility_data.controller();

        let structures = room.find(find::STRUCTURES);

        let containers = structures
            .iter()
            .filter_map(|structure| match structure {
                Structure::Container(container) => Some(container.remote_id()),
                _ => None,
            })
            .collect_vec();

        let room_extractors = structures
            .iter()
            .filter_map(|structure| match structure {
                Structure::Extractor(extractor) => Some(extractor.remote_id()),
                _ => None,
            })
            .collect_vec();

        let sources_to_containers = sources
            .iter()
            .filter_map(|source| {
                let nearby_container = containers
                    .iter()
                    .cloned()
                    .find(|container| container.pos().is_near_to(&source.pos()));

                nearby_container.map(|container| (*source, container))
            })
            .into_group_map();

        let controllers_to_containers = controller
            .iter()
            .filter_map(|controller| {
                let nearby_container = containers
                    .iter()
                    .cloned()
                    .find(|container| container.pos().is_near_to(&controller.pos()));

                nearby_container.map(|container| (**controller, container))
            })
            .into_group_map();

        let minerals = room.find(find::MINERALS);

        let mineral_extractors_to_containers = room_extractors
            .iter()
            .filter_map(|extractor| {
                if let Some(mineral) = minerals.iter().find(|m| m.pos() == extractor.pos()) {
                    let nearby_container = containers
                        .iter()
                        .cloned()
                        .find(|container| container.pos().is_near_to(&extractor.pos()));

                    nearby_container.map(|container| ((mineral.remote_id(), *extractor), container))
                } else {
                    None
                }
            })
            .into_group_map();

        let room_links = structures
            .iter()
            .filter_map(|structure| match structure {
                Structure::Link(link) => Some(link),
                _ => None,
            })
            .collect_vec();

        //TODO: This should actually be range 2 since creeps cannot stand on top of it.
        let sources_to_links = sources
            .iter()
            .filter_map(|source| {
                let nearby_link = room_links.iter().cloned().find(|link| link.pos().is_near_to(&source.pos()));

                nearby_link.map(|link| (*source, link.remote_id()))
            })
            .into_group_map();

        let structure_data = StructureData {
            sources_to_containers,
            sources_to_links,
            mineral_extractors_to_containers,
            controllers_to_containers,
            containers,
        };

        Ok(structure_data)
    }

    fn create_creep_data(&self, system_data: &MissionExecutionSystemData) -> Result<CreepData, String> {
        //
        // Creep data gathering.
        //

        //TODO: Store this mapping data as part of the mission. (Blocked on specs collection serialization.)
        let sources_to_harvesters = self
            .harvesters
            .0
            .iter()
            .filter_map(|harvester_entity| {
                if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                    Some((*harvester_data.harvest_target(), *harvester_entity))
                } else {
                    None
                }
            })
            .into_group_map();

        let containers_to_source_miners = self
            .source_miners
            .0
            .iter()
            .filter_map(|miner_entity| {
                if let Some(JobData::StaticMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    Some((miner_data.container_target, *miner_entity))
                } else {
                    None
                }
            })
            .into_group_map();

        let containers_to_mineral_miners = self
            .mineral_miners
            .0
            .iter()
            .filter_map(|miner_entity| {
                if let Some(JobData::StaticMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    Some((miner_data.container_target, *miner_entity))
                } else {
                    None
                }
            })
            .into_group_map();

        let creep_data = CreepData {
            sources_to_harvesters,
            containers_to_source_miners,
            containers_to_mineral_miners,
        };

        Ok(creep_data)
    }

    fn spawn_creeps(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
        structure_data: &StructureData,
        creep_data: &CreepData,
    ) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;
        let sources = static_visibility_data.sources();

        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility")?;
        let likely_owned_room = dynamic_visibility_data.updated_within(2000)
            && (dynamic_visibility_data.owner().mine() || dynamic_visibility_data.reservation().mine());

        //
        // Sort sources so requests with equal priority go to the source with the least activity.
        //

        let total_harvesters = self.harvesters.0.len();
        let total_source_miners = self.source_miners.0.len();
        let total_harvesting_creeps = total_harvesters + total_source_miners;

        let mut prioritized_sources = sources.clone();

        prioritized_sources.sort_by_cached_key(|source| {
            let source_harvesters = creep_data
                .sources_to_harvesters
                .get(source)
                .map(|harvesters| harvesters.len())
                .unwrap_or(0);

            let source_miners = structure_data
                .sources_to_containers
                .get(source)
                .map(|containers| {
                    containers
                        .iter()
                        .map(|container| {
                            creep_data
                                .containers_to_source_miners
                                .get(container)
                                .map(|miners| miners.len())
                                .unwrap_or(0)
                        })
                        .sum::<usize>()
                })
                .unwrap_or(0);

            source_harvesters + source_miners
        });

        //
        // Spawn needed creeps for each source.
        //

        for source_id in prioritized_sources.iter() {
            let source_containers = structure_data
                .sources_to_containers
                .get(source_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let source_harvesters = creep_data.sources_to_harvesters.get(source_id).map(Vec::as_slice).unwrap_or(&[]);
            let source_miners = source_containers
                .iter()
                .filter_map(|container| creep_data.containers_to_source_miners.get(container))
                .flat_map(|m| m)
                .collect_vec();

            let alive_source_miners = source_miners
                .iter()
                .filter(|entity| {
                    system_data
                        .creep_spawning
                        .get(***entity)
                        .is_some() ||

                    system_data
                        .creep_owner
                        .get(***entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live().ok())
                        .map(|count| count > 50)
                        .unwrap_or(false)
                })
                .map(|entity| **entity)
                .collect_vec();

            let available_containers_for_source_miners = source_containers.iter().filter(|container| {
                creep_data
                    .containers_to_source_miners
                    .get(container)
                    .map(|miners| !miners.iter().any(|miner| alive_source_miners.contains(miner)))
                    .unwrap_or(true)
            });

            //
            // Spawn container miners.
            //

            for container in available_containers_for_source_miners {
                let energy_capacity = if likely_owned_room {
                    SOURCE_ENERGY_CAPACITY
                } else {
                    SOURCE_ENERGY_NEUTRAL_CAPACITY
                };

                let energy_per_tick = (energy_capacity as f32) / (ENERGY_REGEN_TIME as f32);
                let work_parts_per_tick = (energy_per_tick / (HARVEST_POWER as f32)).ceil() as usize;

                let body_definition = crate::creep::SpawnBodyDefinition {
                    maximum_energy: room.energy_capacity_available(),
                    minimum_repeat: Some(1),
                    maximum_repeat: Some(work_parts_per_tick),
                    pre_body: &[Part::Move],
                    repeat_body: &[Part::Work],
                    post_body: &[],
                };

                if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                    let spawn_request = SpawnRequest::new(
                        format!("Container Miner - Source: {}", source_id.id()),
                        &body,
                        SPAWN_PRIORITY_HIGH,
                        Self::create_handle_container_miner_spawn(*runtime_data.entity, StaticMineTarget::Source(*source_id), *container),
                    );

                    runtime_data.spawn_queue.request(room_data.name, spawn_request);
                }
            }

            //
            // Spawn harvesters
            //

            //TODO: Compute correct number of harvesters to use for source.
            //TODO: Compute the correct time to spawn emergency harvesters.
            if (source_containers.is_empty() && source_harvesters.len() < 4) || total_harvesting_creeps == 0 {
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

                    let spawn_request = SpawnRequest::new(
                        format!("Harvester - Source: {}", source_id.id()),
                        &body,
                        priority,
                        Self::create_handle_harvester_spawn(*runtime_data.entity, *source_id, self.room_data),
                    );

                    runtime_data.spawn_queue.request(room_data.name, spawn_request);
                }
            }
        }

        for ((mineral_id, extractor_id), container_ids) in structure_data.mineral_extractors_to_containers.iter() {
            let mineral_miners = container_ids
                .iter()
                .filter_map(|container| creep_data.containers_to_mineral_miners.get(container))
                .flat_map(|m| m)
                .collect_vec();

            let alive_mineral_miners = mineral_miners
                .iter()
                .filter(|entity| {
                    system_data
                        .creep_spawning
                        .get(***entity)
                        .is_some() ||

                    system_data
                        .creep_owner
                        .get(***entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live().ok())
                        .map(|count| count > 50)
                        .unwrap_or(false)
                })
                .map(|entity| **entity)
                .collect_vec();

            let available_containers_for_miners = container_ids.iter().filter(|container| {
                    creep_data
                        .containers_to_mineral_miners
                        .get(container)
                        .map(|miners| !miners.iter().any(|miner| alive_mineral_miners.contains(miner)))
                        .unwrap_or(true)
                });

            //
            // Spawn container miners.
            //

            for container in available_containers_for_miners {
                //TODO: Compute correct body type.
                let body_definition = crate::creep::SpawnBodyDefinition {
                    maximum_energy: room.energy_capacity_available(),
                    minimum_repeat: Some(1),
                    maximum_repeat: Some(10),
                    pre_body: &[Part::Move],
                    repeat_body: &[Part::Work],
                    post_body: &[],
                };

                if let Ok(body) = crate::creep::Spawning::create_body(&body_definition) {
                    let spawn_request = SpawnRequest::new(
                        format!("Container Miner - Extractor: {}", extractor_id.id()),
                        &body,
                        SPAWN_PRIORITY_LOW,
                        Self::create_handle_container_miner_spawn(*runtime_data.entity, StaticMineTarget::Mineral(*mineral_id, *extractor_id), *container),
                    );

                    runtime_data.spawn_queue.request(room_data.name, spawn_request);
                }
            }
        }

        Ok(())
    }

    fn request_transfer_for_containers(
        transfer_queue: &mut TransferQueue,
        structure_data: &StructureData,
    ) {
        let provider_containers = structure_data
            .sources_to_containers
            .values()
            .chain(structure_data.mineral_extractors_to_containers.values());

        for containers in provider_containers {
            for container_id in containers {
                if let Some(container) = container_id.resolve() {
                    let container_used_capacity = container.store_total();
                    if container_used_capacity > 0 {
                        let container_store_capacity = container.store_capacity(None);

                        let storage_fraction = (container_used_capacity as f32) / (container_store_capacity as f32);
                        let priority = if storage_fraction > 0.75 {
                            TransferPriority::High
                        } else if storage_fraction > 0.5 {
                            TransferPriority::Medium
                        } else if storage_fraction > 0.25 {
                            TransferPriority::Low
                        } else {
                            TransferPriority::None
                        };

                        for resource in container.store_types() {
                            let resource_amount = container.store_used_capacity(Some(resource));
                            let transfer_request =
                                TransferWithdrawRequest::new(TransferTarget::Container(*container_id), resource, priority, resource_amount);

                            transfer_queue.request_withdraw(transfer_request);
                        }
                    }
                }
            }
        }

        for containers in structure_data.controllers_to_containers.values() {
            for container_id in containers {
                if let Some(container) = container_id.resolve() {
                    let container_free_capacity = container.store_free_capacity(Some(ResourceType::Energy));
                    if container_free_capacity > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Container(*container_id),
                            Some(ResourceType::Energy),
                            TransferPriority::Low,
                            container_free_capacity,
                        );
    
                        transfer_queue.request_deposit(transfer_request);
                    }

                    let container_used_capacity = container.store_used_capacity(Some(ResourceType::Energy));
                    if container_used_capacity > 0 {
                        let transfer_request = TransferWithdrawRequest::new(
                            TransferTarget::Container(*container_id),
                            ResourceType::Energy,
                            TransferPriority::None,
                            container_used_capacity,
                        );
    
                        transfer_queue.request_withdraw(transfer_request);
                    }
                }
            }
        }

        let storage_containers = structure_data.containers.iter().filter(|container| {
            !structure_data.sources_to_containers.values().any(|c| c.contains(container))
                && !structure_data.controllers_to_containers.values().any(|c| c.contains(container))
                && !structure_data.mineral_extractors_to_containers.values().any(|c| c.contains(container))
        });

        for container_id in storage_containers {
            if let Some(container) = container_id.resolve() {
                let capacity = container.store_capacity(None);
                let store_types = container.store_types();
                let used_capacity = store_types.iter().map(|r| container.store_used_capacity(Some(*r))).sum::<u32>();
                //TODO: Fix this when _sum double count bug is fixed.
                //let container_free_capacity = container.store_free_capacity(None);
                let container_free_capacity = capacity - used_capacity;
                if container_free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Container(*container_id),
                        None,
                        TransferPriority::None,
                        container_free_capacity,
                    );

                    transfer_queue.request_deposit(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_structures(
        transfer_queue: &mut TransferQueue,
        room: &Room,
    ) {
        //TODO: Migrate these to a better place?
        //TODO: Fill out remaining structures.

        for structure in room.find(find::STRUCTURES) {
            match structure {
                Structure::Spawn(spawn) => {
                    let free_capacity = spawn.store_free_capacity(Some(ResourceType::Energy));
                    if free_capacity > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Spawn(spawn.remote_id()),
                            Some(ResourceType::Energy),
                            TransferPriority::High,
                            free_capacity,
                        );

                        transfer_queue.request_deposit(transfer_request);
                    }
                }
                Structure::Extension(extension) => {
                    let free_capacity = extension.store_free_capacity(Some(ResourceType::Energy));
                    if free_capacity > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Extension(extension.remote_id()),
                            Some(ResourceType::Energy),
                            TransferPriority::High,
                            free_capacity,
                        );

                        transfer_queue.request_deposit(transfer_request);
                    }
                }
                Structure::Storage(storage) => {
                    let storage_id = storage.remote_id();

                    let mut used_capacity = 0;

                    for resource in storage.store_types() {
                        let resource_amount = storage.store_used_capacity(Some(resource));
                        let transfer_request = TransferWithdrawRequest::new(
                            TransferTarget::Storage(storage_id),
                            resource,
                            TransferPriority::None,
                            resource_amount,
                        );

                        transfer_queue.request_withdraw(transfer_request);

                        used_capacity += resource_amount;
                    }

                    let free_capacity = storage.store_capacity(None) - used_capacity;

                    if free_capacity > 0 {
                        let transfer_request =
                            TransferDepositRequest::new(TransferTarget::Storage(storage_id), None, TransferPriority::None, free_capacity);

                        transfer_queue.request_deposit(transfer_request);
                    }
                },
                Structure::Link(link) => {
                    let free_capacity = link.store_free_capacity(Some(ResourceType::Energy));
                    if free_capacity > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Link(link.remote_id()),
                            Some(ResourceType::Energy),
                            TransferPriority::None,
                            free_capacity,
                        );

                        transfer_queue.request_deposit(transfer_request);
                    }

                    let used_capacity = link.store_used_capacity(Some(ResourceType::Energy));
                    if used_capacity > 0 {
                        let transfer_request = TransferWithdrawRequest::new(
                            TransferTarget::Link(link.remote_id()),
                            ResourceType::Energy,
                            TransferPriority::None,
                            used_capacity,
                        );

                        transfer_queue.request_withdraw(transfer_request);
                    }
                }
                _ => {}
            }
        }
    }

    fn request_transfer_for_ruins(
        transfer_queue: &mut TransferQueue,
        room: &Room,
    ) {
        for ruin in room.find(find::RUINS) {
            let ruin_id = ruin.remote_id();

            for resource in ruin.store_types() {
                let resource_amount = ruin.store_used_capacity(Some(resource));
                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Ruin(ruin_id),
                    resource,
                    TransferPriority::Medium,
                    resource_amount,
                );

                transfer_queue.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_tombstones(
        transfer_queue: &mut TransferQueue,
        room: &Room,
    ) {
        for tombstone in room.find(find::TOMBSTONES) {
            let tombstone_id = tombstone.remote_id();

            for resource in tombstone.store_types() {
                let resource_amount = tombstone.store_used_capacity(Some(resource));
                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Tombstone(tombstone_id),
                    resource,
                    TransferPriority::Medium,
                    resource_amount,
                );

                transfer_queue.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_dropped_resources(
        transfer_queue: &mut TransferQueue,
        room: &Room,
    ) {
        for dropped_resource in room.find(find::DROPPED_RESOURCES) {
            let dropped_resource_id = dropped_resource.remote_id();

            let resource = dropped_resource.resource_type();
            let resource_amount = dropped_resource.amount();
            let transfer_request = TransferWithdrawRequest::new(
                TransferTarget::Resource(dropped_resource_id),
                resource,
                TransferPriority::Medium,
                resource_amount,
            );

            transfer_queue.request_withdraw(transfer_request);
        }
    }
}

#[cfg_attr(feature = "time", timing)]
impl Mission for LocalSupplyMission {
    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text(
                    format!("Local Supply - Miners: {} Harvesters: {} Minerals: {}", self.source_miners.0.len(), self.harvesters.0.len(), self.mineral_miners.0.len()),
                    None,
                );
            })
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.harvesters.0.retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        self.source_miners.0.retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        self.mineral_miners.0.retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        //TODO: Cache structure + creep data.
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let structure_data = Self::create_structure_data(room_data, &room)?;

        Self::request_transfer_for_containers(&mut runtime_data.transfer_queue, &structure_data);
        Self::request_transfer_for_structures(&mut runtime_data.transfer_queue, &room);
        Self::request_transfer_for_ruins(&mut runtime_data.transfer_queue, &room);
        Self::request_transfer_for_tombstones(&mut runtime_data.transfer_queue, &room);
        Self::request_transfer_for_dropped_resources(&mut runtime_data.transfer_queue, &room);

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let structure_data = Self::create_structure_data(room_data, &room)?;
        let creep_data = self.create_creep_data(system_data)?;

        self.spawn_creeps(system_data, runtime_data, &structure_data, &creep_data)?;

        Ok(MissionResult::Running)
    }
}
