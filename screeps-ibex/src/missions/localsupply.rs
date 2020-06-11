use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::harvest::*;
use crate::jobs::linkmine::*;
use crate::jobs::staticmine::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::store::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use std::cell::*;
use std::collections::HashMap;
use std::rc::*;

#[derive(ConvertSaveload)]
pub struct LocalSupplyMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    harvesters: EntityVec<Entity>,
    source_container_miners: EntityVec<Entity>,
    source_link_miners: EntityVec<Entity>,
    mineral_container_miners: EntityVec<Entity>,
    #[convert_save_load_attr(serde(skip))]
    #[convert_save_load_skip_convert]
    structure_data: Rc<RefCell<Option<StructureData>>>,
}

type MineralExtractorPair = (RemoteObjectId<Mineral>, RemoteObjectId<StructureExtractor>);

#[derive(Clone)]
struct StructureData {
    last_updated: u32,
    sources_to_containers: HashMap<RemoteObjectId<Source>, Vec<RemoteObjectId<StructureContainer>>>,
    sources_to_links: HashMap<RemoteObjectId<Source>, Vec<RemoteObjectId<StructureLink>>>,
    storage_links: Vec<RemoteObjectId<StructureLink>>,
    mineral_extractors_to_containers: HashMap<MineralExtractorPair, Vec<RemoteObjectId<StructureContainer>>>,
    controllers_to_containers: HashMap<RemoteObjectId<StructureController>, Vec<RemoteObjectId<StructureContainer>>>,
    controller_links: Vec<RemoteObjectId<StructureLink>>,
    containers: Vec<RemoteObjectId<StructureContainer>>,
    spawns: Vec<RemoteObjectId<StructureSpawn>>,
    extensions: Vec<RemoteObjectId<StructureExtension>>,
    storage: Vec<RemoteObjectId<StructureStorage>>,
}

struct CreepData {
    sources_to_harvesters: EntityHashMap<RemoteObjectId<Source>, EntityVec<Entity>>,
    containers_to_source_miners: EntityHashMap<RemoteObjectId<StructureContainer>, EntityVec<Entity>>,
    links_to_source_miners: EntityHashMap<RemoteObjectId<StructureLink>, EntityVec<Entity>>,
    containers_to_mineral_miners: EntityHashMap<RemoteObjectId<StructureContainer>, EntityVec<Entity>>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LocalSupplyMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalSupplyMission::new(owner, room_data);

        builder
            .with(MissionData::LocalSupply(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> LocalSupplyMission {
        LocalSupplyMission {
            owner: owner.into(),
            room_data,
            harvesters: EntityVec::new(),
            source_container_miners: EntityVec::new(),
            source_link_miners: EntityVec::new(),
            mineral_container_miners: EntityVec::new(),
            structure_data: Rc::new(RefCell::new(None)),
        }
    }

    fn create_handle_container_miner_spawn(
        mission_entity: Entity,
        target: StaticMineTarget,
        container_id: RemoteObjectId<StructureContainer>,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::StaticMine(StaticMineJob::new(target, container_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<LocalSupplyMission>()
                {
                    match target {
                        StaticMineTarget::Source(_) => mission_data.source_container_miners.push(creep_entity),
                        StaticMineTarget::Mineral(_, _) => mission_data.mineral_container_miners.push(creep_entity),
                    }
                }
            });
        })
    }

    fn create_handle_link_miner_spawn(
        mission_entity: Entity,
        source_id: RemoteObjectId<Source>,
        link_id: RemoteObjectId<StructureLink>,
        container_id: Option<RemoteObjectId<StructureContainer>>,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::LinkMine(LinkMineJob::new(source_id, link_id, container_id));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<LocalSupplyMission>()
                {
                    mission_data.source_link_miners.push(creep_entity);
                }
            });
        })
    }

    fn create_handle_harvester_spawn(
        mission_entity: Entity,
        source_id: RemoteObjectId<Source>,
        delivery_room: Entity,
    ) -> Box<dyn Fn(&SpawnQueueExecutionSystemData, &str)> {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Harvest(HarvestJob::new(source_id, delivery_room, true));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<LocalSupplyMission>()
                {
                    mission_data.harvesters.push(creep_entity);
                }
            });
        })
    }

    fn create_structure_data(room_data: &RoomData) -> Option<StructureData> {
        let structure_data = room_data.get_structures()?;
        let static_visibility_data = room_data.get_static_visibility_data()?;

        let sources = static_visibility_data.sources();
        let controller = static_visibility_data.controller();

        let storages = structure_data.storages();
        let spawns = structure_data.spawns();
        let extensions = structure_data.extensions();
        let links = structure_data.links();
        let containers = structure_data.containers();
        let extractors = structure_data.extractors();

        let sources_to_containers = sources
            .iter()
            .filter_map(|source| {
                let nearby_container = containers.iter().find(|container| container.pos().is_near_to(&source.pos()));

                nearby_container.map(|container| (*source, container.remote_id()))
            })
            .into_group_map();

        let controllers_to_containers = controller
            .iter()
            .filter_map(|controller| {
                let nearby_container = containers.iter().find(|container| container.pos().is_near_to(&controller.pos()));

                nearby_container.map(|container| (**controller, container.remote_id()))
            })
            .into_group_map();

        //TODO: This may need more validate that the link is reachable - or assume it is and bad placemented is filtered out
        //      during room planning.
        //TODO: May need additional work to make sure link is not used by a controller or storage.
        let controller_links: Vec<_> = controller
            .iter()
            .filter_map(|controller| {
                let nearby_link = links.iter().find(|link| link.pos().in_range_to(&controller.pos(), 2));

                nearby_link.map(|link| link.remote_id())
            })
            .collect();

        let minerals = static_visibility_data.minerals();

        let mineral_extractors_to_containers = extractors
            .iter()
            .filter_map(|extractor| {
                if let Some(mineral) = minerals.iter().find(|m| m.pos() == extractor.pos()) {
                    let nearby_container = containers.iter().find(|container| container.pos().is_near_to(&extractor.pos()));

                    nearby_container.map(|container| ((*mineral, extractor.remote_id()), container.remote_id()))
                } else {
                    None
                }
            })
            .into_group_map();

        //TODO: This may need more validate that the link is reachable - or assume it is and bad placemented is filtered out
        //      during room planning.
        //TODO: May need additional work to make sure link is not used by a controller or storage.
        let sources_to_links = sources
            .iter()
            .flat_map(|&source| {
                links
                    .iter()
                    .filter(move |link| link.pos().in_range_to(&source.pos(), 2))
                    .map(|link| link.remote_id())
                    .filter(|id| !controller_links.contains(id))
                    .map(move |id| (source, id))
            })
            .into_group_map();

        let storage_links = links
            .into_iter()
            .filter(|link| storages.iter().any(|storage| link.pos().in_range_to(&storage.pos(), 2)))
            .map(|link| link.remote_id())
            .collect();

        Some(StructureData {
            last_updated: game::time(),
            sources_to_containers,
            sources_to_links,
            storage_links,
            mineral_extractors_to_containers,
            controllers_to_containers,
            controller_links,
            containers: containers.iter().map(|s| s.remote_id()).collect(),
            spawns: spawns.iter().map(|s| s.remote_id()).collect(),
            extensions: extensions.iter().map(|e| e.remote_id()).collect(),
            storage: storages.iter().map(|s| s.remote_id()).collect(),
        })
    }

    fn create_creep_data(&self, system_data: &MissionExecutionSystemData) -> Result<CreepData, String> {
        //
        // Creep data gathering.
        //

        //TODO: Store this mapping data as part of the mission. (Blocked on specs collection serialization.)
        let sources_to_harvesters = self
            .harvesters
            .iter()
            .filter_map(|harvester_entity| {
                if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                    Some((*harvester_data.harvest_target(), *harvester_entity))
                } else {
                    None
                }
            })
            .into_entity_group_map();

        let containers_to_source_miners = self
            .source_container_miners
            .iter()
            .filter_map(|miner_entity| {
                if let Some(JobData::StaticMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    Some((miner_data.context.container_target, *miner_entity))
                } else if let Some(JobData::LinkMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    if let Some(container_target) = miner_data.get_container_target() {
                        Some((*container_target, *miner_entity))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .into_entity_group_map();

        let links_to_source_miners = self
            .source_link_miners
            .iter()
            .filter_map(|miner_entity| {
                if let Some(JobData::LinkMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    Some((*miner_data.get_link_target(), *miner_entity))
                } else {
                    None
                }
            })
            .into_entity_group_map();

        let containers_to_mineral_miners = self
            .mineral_container_miners
            .iter()
            .filter_map(|miner_entity| {
                if let Some(JobData::StaticMine(miner_data)) = system_data.job_data.get(*miner_entity) {
                    Some((miner_data.context.container_target, *miner_entity))
                } else {
                    None
                }
            })
            .into_entity_group_map();

        let creep_data = CreepData {
            sources_to_harvesters,
            containers_to_source_miners,
            links_to_source_miners,
            containers_to_mineral_miners,
        };

        Ok(creep_data)
    }

    fn get_all_links(&mut self, system_data: &mut MissionExecutionSystemData) -> Result<Vec<RemoteObjectId<StructureLink>>, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let mut structure_data = self
            .structure_data
            .maybe_access(|d| game::time() - d.last_updated >= 10, || Self::create_structure_data(&room_data));
        let structure_data = structure_data.get().ok_or("Expected structure data")?;

        let all_links = structure_data
            .sources_to_links
            .values()
            .flatten()
            .chain(structure_data.storage_links.iter())
            .cloned()
            .collect();

        Ok(all_links)
    }

    fn link_transfer(&mut self, system_data: &mut MissionExecutionSystemData) -> Result<(), String> {
        let all_links = self.get_all_links(system_data)?;

        let transfer_queue = &mut system_data.transfer_queue;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Link Transfer",
            room_data: &*system_data.room_data,
        };

        for link_id in all_links {
            if let Some(link) = link_id.resolve() {
                if link.cooldown() == 0 && link.store_of(ResourceType::Energy) > 0 {
                    let link_pos = link.pos();
                    let room_name = link_pos.room_name();

                    //TODO: Potentially use active priority pairs to iterate here. Currently relies on there never being a None -> None priority request.
                    let best_transfer = ALL_TRANSFER_PRIORITIES
                        .iter()
                        .filter_map(|priority| {
                            transfer_queue.get_delivery_from_target(
                                &transfer_queue_data,
                                &[room_name],
                                &TransferTarget::Link(link_id),
                                TransferPriorityFlags::ACTIVE,
                                priority.into(),
                                TransferType::Link,
                                TransferCapacity::Infinite,
                                link_pos,
                            )
                        })
                        .next();

                    if let Some((pickup, delivery)) = best_transfer {
                        transfer_queue.register_pickup(&pickup, TransferType::Link);
                        transfer_queue.register_delivery(&delivery, TransferType::Link);

                        //TODO: Validate there isn't non-energy in here?
                        let transfer_amount = delivery
                            .resources()
                            .get(&ResourceType::Energy)
                            .map(|entries| entries.iter().map(|entry| entry.amount()).sum())
                            .unwrap_or(0);

                        delivery.target().link_transfer_energy_amount(&link, transfer_amount);
                    }
                }
            }
        }

        Ok(())
    }

    fn spawn_creeps(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        mission_entity: Entity,
        creep_data: &CreepData,
    ) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;
        let sources = static_visibility_data.sources();

        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility")?;
        let likely_owned_room = dynamic_visibility_data.updated_within(2000)
            && (dynamic_visibility_data.owner().mine() || dynamic_visibility_data.reservation().mine());

        let mut structure_data = self
            .structure_data
            .maybe_access(|d| game::time() - d.last_updated >= 10, || Self::create_structure_data(&room_data));
        let structure_data = structure_data.get().ok_or("Expected structure data")?;

        //
        // Sort sources so requests with equal priority go to the source with the least activity.
        //

        let total_harvesters = self.harvesters.len();
        let total_source_container_miners = self.source_container_miners.len();
        let total_source_link_miners = self.source_link_miners.len();
        let total_harvesting_creeps = total_harvesters + total_source_container_miners + total_source_link_miners;

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

        for source_id in prioritized_sources.iter().rev() {
            let source_harvesters = &creep_data.sources_to_harvesters.get(source_id).map(|c| c.as_slice()).unwrap_or(&[]);

            let source_containers = structure_data
                .sources_to_containers
                .get(source_id)
                .map(|c| c.as_slice())
                .unwrap_or(&[]);

            let source_links = structure_data.sources_to_links.get(source_id).map(|c| c.as_slice()).unwrap_or(&[]);

            let source_container_miners = source_containers
                .iter()
                .filter_map(|container| creep_data.containers_to_source_miners.get(container))
                .flat_map(|m| m.iter())
                .collect_vec();

            let source_link_miners = source_links
                .iter()
                .filter_map(|link| creep_data.links_to_source_miners.get(link))
                .flat_map(|m| m.iter())
                .collect_vec();

            //
            // Spawn harvesters
            //

            //TODO: Compute correct number of harvesters to use for source.
            //TODO: Compute the correct time to spawn emergency harvesters.
            if (source_containers.is_empty() && source_links.is_empty() && source_harvesters.len() < 4) || total_harvesting_creeps == 0 {
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

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let priority = if total_harvesting_creeps == 0 {
                        SPAWN_PRIORITY_CRITICAL
                    } else {
                        SPAWN_PRIORITY_HIGH
                    };

                    let spawn_request = SpawnRequest::new(
                        format!("Harvester - Source: {}", source_id.id()),
                        &body,
                        priority,
                        Self::create_handle_harvester_spawn(mission_entity, *source_id, self.room_data),
                    );

                    system_data.spawn_queue.request(self.room_data, spawn_request);
                }
            } else {
                let alive_source_miners = source_container_miners
                    .iter()
                    .chain(source_link_miners.iter())
                    .filter(|entity| {
                        system_data.creep_spawning.get(***entity).is_some()
                            || system_data
                                .creep_owner
                                .get(***entity)
                                .and_then(|creep_owner| creep_owner.owner.resolve())
                                .and_then(|creep| creep.ticks_to_live().ok())
                                .map(|count| count > 50)
                                .unwrap_or(false)
                    })
                    .map(|entity| **entity)
                    .collect_vec();

                if !source_links.is_empty() {
                    //
                    // Spawn link miners.
                    //

                    let mut available_containers_for_source_miners = source_containers.iter().filter(|container| {
                        creep_data
                            .containers_to_source_miners
                            .get(container)
                            .map(|miners| !miners.iter().any(|miner| alive_source_miners.contains(miner)))
                            .unwrap_or(true)
                    });

                    let available_links_for_source_miners = source_links.iter().filter(|link| {
                        creep_data
                            .links_to_source_miners
                            .get(link)
                            .map(|miners| !miners.iter().any(|miner| alive_source_miners.contains(miner)))
                            .unwrap_or(true)
                    });

                    for link in available_links_for_source_miners {
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
                            pre_body: &[Part::Move, Part::Carry],
                            repeat_body: &[Part::Work],
                            post_body: &[],
                        };

                        if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                            //TODO: Should the container be removed from the available ones?
                            let target_container = available_containers_for_source_miners.next();

                            let spawn_request = SpawnRequest::new(
                                format!("Link Miner - Source: {}", source_id.id()),
                                &body,
                                SPAWN_PRIORITY_HIGH,
                                Self::create_handle_link_miner_spawn(mission_entity, *source_id, *link, target_container.cloned()),
                            );

                            system_data.spawn_queue.request(self.room_data, spawn_request);
                        }
                    }
                } else if !source_containers.is_empty() {
                    //
                    // Spawn container miners.
                    //

                    let available_containers_for_source_miners = source_containers.iter().filter(|container| {
                        creep_data
                            .containers_to_source_miners
                            .get(container)
                            .map(|miners| !miners.iter().any(|miner| alive_source_miners.contains(miner)))
                            .unwrap_or(true)
                    });

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

                        if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                            let spawn_request = SpawnRequest::new(
                                format!("Container Miner - Source: {}", source_id.id()),
                                &body,
                                SPAWN_PRIORITY_HIGH,
                                Self::create_handle_container_miner_spawn(mission_entity, StaticMineTarget::Source(*source_id), *container),
                            );

                            system_data.spawn_queue.request(self.room_data, spawn_request);
                        }
                    }
                }
            }
        }

        for ((mineral_id, extractor_id), container_ids) in structure_data.mineral_extractors_to_containers.iter() {
            if let Some(mineral) = mineral_id.resolve() {
                if mineral.mineral_amount() == 0 {
                    continue;
                }
            }

            let mineral_miners = container_ids
                .iter()
                .filter_map(|container| creep_data.containers_to_mineral_miners.get(container))
                .flat_map(|m| m.iter())
                .collect_vec();

            let alive_mineral_miners = mineral_miners
                .iter()
                .filter(|entity| {
                    system_data.creep_spawning.get(***entity).is_some()
                        || system_data
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

                if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                    let spawn_request = SpawnRequest::new(
                        format!("Container Miner - Extractor: {}", extractor_id.id()),
                        &body,
                        SPAWN_PRIORITY_LOW,
                        Self::create_handle_container_miner_spawn(
                            mission_entity,
                            StaticMineTarget::Mineral(*mineral_id, *extractor_id),
                            *container,
                        ),
                    );

                    system_data.spawn_queue.request(self.room_data, spawn_request);
                }
            }
        }

        Ok(())
    }

    fn request_transfer_for_containers(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
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
                            let transfer_request = TransferWithdrawRequest::new(
                                TransferTarget::Container(*container_id),
                                resource,
                                priority,
                                resource_amount,
                                TransferType::Haul,
                            );

                            transfer.request_withdraw(transfer_request);
                        }
                    }
                }
            }
        }

        for containers in structure_data.controllers_to_containers.values() {
            for container_id in containers {
                if let Some(container) = container_id.resolve() {
                    let container_used_capacity = container.store_used_capacity(Some(ResourceType::Energy));
                    let container_available_capacity = container.store_capacity(Some(ResourceType::Energy));
                    let container_free_capacity = container_available_capacity - container_used_capacity;

                    let storage_fraction = container_used_capacity as f32 / container_available_capacity as f32;

                    if container_free_capacity > 0 {
                        let priority = if storage_fraction < 0.75 {
                            TransferPriority::Low
                        } else {
                            TransferPriority::None
                        };

                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Container(*container_id),
                            Some(ResourceType::Energy),
                            priority,
                            container_free_capacity,
                            TransferType::Haul,
                        );

                        transfer.request_deposit(transfer_request);
                    }

                    let container_used_capacity = container.store_used_capacity(Some(ResourceType::Energy));
                    if container_used_capacity > 0 {
                        let transfer_request = TransferWithdrawRequest::new(
                            TransferTarget::Container(*container_id),
                            ResourceType::Energy,
                            TransferPriority::None,
                            container_used_capacity,
                            TransferType::Use,
                        );

                        transfer.request_withdraw(transfer_request);
                    }
                }
            }
        }

        let storage_containers = structure_data.containers.iter().filter(|container| {
            !structure_data.sources_to_containers.values().any(|c| c.contains(container))
                && !structure_data.controllers_to_containers.values().any(|c| c.contains(container))
                && !structure_data
                    .mineral_extractors_to_containers
                    .values()
                    .any(|c| c.contains(container))
        });

        for container_id in storage_containers {
            if let Some(container) = container_id.resolve() {
                let container_free_capacity = container.expensive_store_free_capacity();
                if container_free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Container(*container_id),
                        None,
                        TransferPriority::None,
                        container_free_capacity,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_spawns(transfer: &mut dyn TransferRequestSystem, spawns: &[RemoteObjectId<StructureSpawn>]) {
        for spawn_id in spawns.iter() {
            if let Some(spawn) = spawn_id.resolve() {
                let free_capacity = spawn.store_free_capacity(Some(ResourceType::Energy));
                if free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Spawn(*spawn_id),
                        Some(ResourceType::Energy),
                        TransferPriority::High,
                        free_capacity as u32,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_extension(transfer: &mut dyn TransferRequestSystem, extensions: &[RemoteObjectId<StructureExtension>]) {
        for extension_id in extensions.iter() {
            if let Some(extension) = extension_id.resolve() {
                let free_capacity = extension.store_free_capacity(Some(ResourceType::Energy));
                if free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Extension(*extension_id),
                        Some(ResourceType::Energy),
                        TransferPriority::High,
                        free_capacity as u32,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_storage(transfer: &mut dyn TransferRequestSystem, stores: &[RemoteObjectId<StructureStorage>]) {
        for storage_id in stores.iter() {
            if let Some(storage) = storage_id.resolve() {
                let mut used_capacity = 0;

                for resource in storage.store_types() {
                    let resource_amount = storage.store_used_capacity(Some(resource));
                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Storage(*storage_id),
                        resource,
                        TransferPriority::None,
                        resource_amount,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);

                    used_capacity += resource_amount;
                }

                let free_capacity = storage.store_capacity(None) - used_capacity;

                if free_capacity > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Storage(*storage_id),
                        None,
                        TransferPriority::None,
                        free_capacity,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_storage_links(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
        for link_id in &structure_data.storage_links {
            if let Some(link) = link_id.resolve() {
                let free_capacity = link.store_free_capacity(Some(ResourceType::Energy));

                if free_capacity > 1 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        Some(ResourceType::Energy),
                        TransferPriority::None,
                        free_capacity as u32,
                        TransferType::Link,
                    );

                    transfer.request_deposit(transfer_request);
                }

                let used_capacity = link.store_used_capacity(Some(ResourceType::Energy));

                if used_capacity > 0 {
                    let available_capacity = link.store_capacity(Some(ResourceType::Energy));
                    let storage_fraction = (used_capacity as f32) / (available_capacity as f32);

                    let priority = if storage_fraction > 0.5 {
                        TransferPriority::High
                    } else if storage_fraction > 0.25 {
                        TransferPriority::Low
                    } else {
                        TransferPriority::None
                    };

                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        ResourceType::Energy,
                        priority,
                        used_capacity,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_source_links(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
        for link_id in structure_data.sources_to_links.values().flatten() {
            if let Some(link) = link_id.resolve() {
                let used_capacity = link.store_used_capacity(Some(ResourceType::Energy));

                if used_capacity > 0 {
                    let available_capacity = link.store_capacity(Some(ResourceType::Energy));
                    let storage_fraction = (used_capacity as f32) / (available_capacity as f32);

                    let priority = if storage_fraction > 0.5 {
                        TransferPriority::High
                    } else if storage_fraction > 0.25 {
                        TransferPriority::Medium
                    } else {
                        TransferPriority::Low
                    };

                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        ResourceType::Energy,
                        priority,
                        used_capacity,
                        TransferType::Link,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_controller_links(transfer: &mut dyn TransferRequestSystem, structure_data: &StructureData) {
        for link_id in &structure_data.controller_links {
            if let Some(link) = link_id.resolve() {
                let free_capacity = link.store_free_capacity(Some(ResourceType::Energy));

                if free_capacity > 1 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Link(link.remote_id()),
                        Some(ResourceType::Energy),
                        TransferPriority::Low,
                        free_capacity as u32,
                        TransferType::Link,
                    );

                    transfer.request_deposit(transfer_request);
                }

                let used_capacity = link.store_used_capacity(Some(ResourceType::Energy));

                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Link(link.remote_id()),
                    ResourceType::Energy,
                    TransferPriority::None,
                    used_capacity,
                    TransferType::Haul,
                );

                transfer.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_ruins(transfer: &mut dyn TransferRequestSystem, room: &Room) {
        for ruin in room.find(find::RUINS) {
            let ruin_id = ruin.remote_id();

            for resource in ruin.store_types() {
                let resource_amount = ruin.store_used_capacity(Some(resource));
                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Ruin(ruin_id),
                    resource,
                    TransferPriority::Medium,
                    resource_amount,
                    TransferType::Haul,
                );

                transfer.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_tombstones(transfer: &mut dyn TransferRequestSystem, room: &Room) {
        for tombstone in room.find(find::TOMBSTONES) {
            let tombstone_id = tombstone.remote_id();

            for resource in tombstone.store_types() {
                let resource_amount = tombstone.store_used_capacity(Some(resource));

                //TODO: Only apply this if no hostiles in the room?
                let priority = if resource_amount > 200 || resource != ResourceType::Energy {
                    TransferPriority::High
                } else {
                    TransferPriority::Medium
                };

                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Tombstone(tombstone_id),
                    resource,
                    priority,
                    resource_amount,
                    TransferType::Haul,
                );

                transfer.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_dropped_resources(transfer: &mut dyn TransferRequestSystem, room: &Room) {
        for dropped_resource in room.find(find::DROPPED_RESOURCES) {
            let dropped_resource_id = dropped_resource.remote_id();

            let resource = dropped_resource.resource_type();
            let resource_amount = dropped_resource.amount();

            //TODO: Only apply this if no hostiles in the room?
            let priority = if resource_amount > 500 || resource != ResourceType::Energy {
                TransferPriority::High
            } else {
                TransferPriority::Medium
            };

            let transfer_request = TransferWithdrawRequest::new(
                TransferTarget::Resource(dropped_resource_id),
                resource,
                priority,
                resource_amount,
                TransferType::Haul,
            );

            transfer.request_withdraw(transfer_request);
        }
    }

    fn transfer_request_haul_generator(room_entity: Entity, structure_data: Rc<RefCell<Option<StructureData>>>) -> TransferQueueGenerator {
        Box::new(move |system, transfer, _room_name| {
            let room_data = system.get_room_data(room_entity).ok_or("Expected room data")?;
            let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

            let mut structure_data =
                structure_data.maybe_access(|d| game::time() - d.last_updated >= 10, || Self::create_structure_data(&room_data));
            let structure_data = structure_data.get().ok_or("Expected structure data")?;

            Self::request_transfer_for_spawns(transfer, &structure_data.spawns);
            Self::request_transfer_for_extension(transfer, &structure_data.extensions);
            Self::request_transfer_for_storage(transfer, &structure_data.storage);
            Self::request_transfer_for_containers(transfer, &structure_data);

            Self::request_transfer_for_ruins(transfer, &room);
            Self::request_transfer_for_tombstones(transfer, &room);
            Self::request_transfer_for_dropped_resources(transfer, &room);

            Ok(())
        })
    }

    fn transfer_request_link_generator(room_entity: Entity, structure_data: Rc<RefCell<Option<StructureData>>>) -> TransferQueueGenerator {
        Box::new(move |system, transfer, _room_name| {
            let room_data = system.get_room_data(room_entity).ok_or("Expected room data")?;

            let mut structure_data =
                structure_data.maybe_access(|d| game::time() - d.last_updated >= 10, || Self::create_structure_data(&room_data));
            let structure_data = structure_data.get().ok_or("Expected structure data")?;

            Self::request_transfer_for_source_links(transfer, &structure_data);
            Self::request_transfer_for_storage_links(transfer, &structure_data);
            Self::request_transfer_for_controller_links(transfer, &structure_data);

            Ok(())
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for LocalSupplyMission {
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
            "Local Supply - Miners: {} Harvesters: {} Minerals: {}",
            self.source_container_miners.len() + self.source_link_miners.len(),
            self.harvesters.len(),
            self.mineral_container_miners.len()
        )
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        //
        // Cleanup creeps that no longer exist.
        //

        self.harvesters
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        self.source_container_miners
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        self.source_link_miners
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());
        self.mineral_container_miners
            .retain(|entity| system_data.entities.is_alive(*entity) && system_data.job_data.get(*entity).is_some());

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Self::transfer_request_haul_generator(self.room_data, self.structure_data.clone()),
        );
        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::LINK,
            Self::transfer_request_link_generator(self.room_data, self.structure_data.clone()),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let creep_data = self.create_creep_data(system_data)?;

        self.spawn_creeps(system_data, mission_entity, &creep_data)?;

        self.link_transfer(system_data)?;

        Ok(MissionResult::Running)
    }
}
