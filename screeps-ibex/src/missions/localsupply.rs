use super::data::*;
use super::missionsystem::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::jobs::harvest::*;
use crate::jobs::linkmine::*;
use crate::jobs::staticmine::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::store::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use lerp::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use std::cell::*;
use std::collections::HashMap;
use std::rc::*;

/// Minimum TTL threshold used when we cannot compute a more accurate lead
/// time (e.g. no spawn positions visible). This is deliberately conservative
/// — a small overlap is cheaper than losing mining ticks.
const MIN_REPLACEMENT_LEAD_TICKS: u32 = 30;

/// Estimate how many ticks it takes for a creep with the given body to
/// traverse `distance` tiles, assuming roads are present. Returns the
/// travel time in ticks.
///
/// Screeps movement on roads:
///   fatigue_per_tile = MOVE_COST_ROAD (1) * non_move_parts
///   fatigue_removed_per_tick = MOVE_POWER (2) * move_parts
///   ticks_per_tile = ceil(fatigue_per_tile / fatigue_removed_per_tick)
///
/// If the creep has no MOVE parts it cannot move; returns u32::MAX.
fn estimate_travel_ticks(body: &[Part], distance: u32) -> u32 {
    let move_parts = body.iter().filter(|p| **p == Part::Move).count() as u32;
    if move_parts == 0 {
        return u32::MAX;
    }

    let non_move_parts = body.len() as u32 - move_parts;
    let fatigue_per_tile = MOVE_COST_ROAD * non_move_parts;
    let fatigue_removed_per_tick = MOVE_POWER * move_parts;

    // Ceiling division: ticks needed to clear fatigue from one road tile.
    let ticks_per_tile = fatigue_per_tile.div_ceil(fatigue_removed_per_tick);
    // At minimum 1 tick per tile (even with excess MOVE parts).
    let ticks_per_tile = ticks_per_tile.max(1);

    distance * ticks_per_tile
}

/// Compute the total lead time (in ticks) needed to spawn a replacement
/// creep and have it walk to `target_pos`. Uses the Chebyshev distance from
/// the nearest spawn in `structure_data` to the target as the path estimate.
fn replacement_lead_ticks(body: &[Part], target_pos: screeps::Position, structure_data: &StructureData) -> u32 {
    let spawn_ticks = body.len() as u32 * CREEP_SPAWN_TIME;

    let nearest_spawn_distance = structure_data
        .spawns
        .iter()
        .filter_map(|spawn_id| spawn_id.resolve())
        .map(|spawn| spawn.pos().get_range_to(target_pos))
        .min()
        .unwrap_or(0);

    let travel_ticks = estimate_travel_ticks(body, nearest_spawn_distance);

    spawn_ticks + travel_ticks
}

//TODO: This mission is overloaded and should be split in to separate mission components.

pub struct LocalSupplyMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    harvesters: EntityVec<Entity>,
    source_container_miners: EntityVec<Entity>,
    source_link_miners: EntityVec<Entity>,
    mineral_container_miners: EntityVec<Entity>,
    structure_data: Rc<RefCell<Option<StructureData>>>,
    allow_spawning: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(bound = "MA: Marker")]
pub struct LocalSupplyMissionSaveloadData<MA>
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    owner: <EntityOption<Entity> as ConvertSaveload<MA>>::Data,
    room_data: <Entity as ConvertSaveload<MA>>::Data,
    home_room_datas: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    harvesters: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    source_container_miners: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    source_link_miners: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    mineral_container_miners: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    allow_spawning: <bool as ConvertSaveload<MA>>::Data,
}

impl<MA> ConvertSaveload<MA> for LocalSupplyMission
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    type Data = LocalSupplyMissionSaveloadData<MA>;
    #[allow(deprecated)]
    type Error = NoError;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<MA>,
    {
        Ok(LocalSupplyMissionSaveloadData {
            owner: ConvertSaveload::convert_into(&self.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_into(&self.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_into(&self.home_room_datas, &mut ids)?,
            harvesters: ConvertSaveload::convert_into(&self.harvesters, &mut ids)?,
            source_container_miners: ConvertSaveload::convert_into(&self.source_container_miners, &mut ids)?,
            source_link_miners: ConvertSaveload::convert_into(&self.source_link_miners, &mut ids)?,
            mineral_container_miners: ConvertSaveload::convert_into(&self.mineral_container_miners, &mut ids)?,
            allow_spawning: ConvertSaveload::convert_into(&self.allow_spawning, &mut ids)?,
        })
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(MA) -> Option<Entity>,
    {
        Ok(LocalSupplyMission {
            owner: ConvertSaveload::convert_from(data.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_from(data.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_from(data.home_room_datas, &mut ids)?,
            harvesters: ConvertSaveload::convert_from(data.harvesters, &mut ids)?,
            source_container_miners: ConvertSaveload::convert_from(data.source_container_miners, &mut ids)?,
            source_link_miners: ConvertSaveload::convert_from(data.source_link_miners, &mut ids)?,
            mineral_container_miners: ConvertSaveload::convert_from(data.mineral_container_miners, &mut ids)?,
            structure_data: Rc::new(RefCell::new(None)),
            allow_spawning: ConvertSaveload::convert_from(data.allow_spawning, &mut ids)?,
        })
    }
}

type MineralExtractorPair = (RemoteObjectId<Mineral>, RemoteObjectId<StructureExtractor>);

#[derive(Clone, Serialize, Deserialize)]
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
    home_rooms_to_harvesters: EntityHashMap<Entity, EntityVec<Entity>>,
    sources_to_harvesters: EntityHashMap<RemoteObjectId<Source>, EntityVec<Entity>>,
    containers_to_source_miners: EntityHashMap<RemoteObjectId<StructureContainer>, EntityVec<Entity>>,
    links_to_source_miners: EntityHashMap<RemoteObjectId<StructureLink>, EntityVec<Entity>>,
    containers_to_mineral_miners: EntityHashMap<RemoteObjectId<StructureContainer>, EntityVec<Entity>>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LocalSupplyMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalSupplyMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::LocalSupply(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> LocalSupplyMission {
        LocalSupplyMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            harvesters: EntityVec::new(),
            source_container_miners: EntityVec::new(),
            source_link_miners: EntityVec::new(),
            mineral_container_miners: EntityVec::new(),
            structure_data: Rc::new(RefCell::new(None)),
            allow_spawning: true,
        }
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.home_room_datas.as_slice() != home_room_datas {
            self.home_room_datas = home_room_datas.to_owned().into();
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow
    }

    fn create_handle_container_miner_spawn(
        mission_entity: Entity,
        target: StaticMineTarget,
        container_id: RemoteObjectId<StructureContainer>,
    ) -> crate::spawnsystem::SpawnQueueCallback {
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
    ) -> crate::spawnsystem::SpawnQueueCallback {
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
    ) -> crate::spawnsystem::SpawnQueueCallback {
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
                let nearby_container = containers.iter().find(|container| container.pos().is_near_to(source.pos()));

                nearby_container.map(|container| (*source, container.remote_id()))
            })
            .into_group_map();

        //TODO: This may need more validate that the link is reachable - or assume it is and bad placement and is filtered out
        //      during room planning.
        //TODO: May need additional work to make sure link is not used by a controller or storage.
        let controller_links: Vec<_> = controller
            .iter()
            .filter_map(|controller| {
                let nearby_link = links.iter().find(|link| link.pos().in_range_to(controller.pos(), 3));

                nearby_link.map(|link| link.remote_id())
            })
            .collect();

        let minerals = static_visibility_data.minerals();

        let mineral_extractors_to_containers = extractors
            .iter()
            .filter_map(|extractor| {
                if let Some(mineral) = minerals.iter().find(|m| m.pos() == extractor.pos()) {
                    let nearby_container = containers.iter().find(|container| container.pos().is_near_to(extractor.pos()));

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
                    .filter(move |link| link.pos().in_range_to(source.pos(), 2))
                    .map(|link| link.remote_id())
                    .filter(|id| !controller_links.contains(id))
                    .map(move |id| (source, id))
            })
            .into_group_map();

        let storage_links = links
            .iter()
            .filter(|link| storages.iter().any(|storage| link.pos().in_range_to(storage.pos(), 2)))
            .map(|link| link.remote_id())
            .collect();

        let controllers_to_containers = controller
            .iter()
            .filter_map(|controller| {
                let nearby_container_id = containers
                    .iter()
                    .map(|container| container.remote_id())
                    .filter(|container| {
                        !sources_to_containers
                            .values()
                            .any(|other_containers| other_containers.iter().any(|other_container| other_container == container))
                    })
                    .filter(|container| {
                        !mineral_extractors_to_containers
                            .values()
                            .any(|other_containers| other_containers.iter().any(|other_container| other_container == container))
                    })
                    .find(|container| container.pos().in_range_to(controller.pos(), 2));

                nearby_container_id.map(|container_id| (**controller, container_id))
            })
            .into_group_map();

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

        let home_rooms_to_harvesters = self
            .harvesters
            .iter()
            .filter_map(|harvester_entity| {
                if let Some(JobData::Harvest(harvester_data)) = system_data.job_data.get(*harvester_entity) {
                    Some((harvester_data.delivery_room(), *harvester_entity))
                } else {
                    None
                }
            })
            .into_entity_group_map();

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
                    miner_data
                        .get_container_target()
                        .as_ref()
                        .map(|container_target| (*container_target, *miner_entity))
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
            home_rooms_to_harvesters,
            sources_to_harvesters,
            containers_to_source_miners,
            links_to_source_miners,
            containers_to_mineral_miners,
        };

        Ok(creep_data)
    }

    fn get_all_links(&mut self, system_data: &mut MissionExecutionSystemData) -> Result<Vec<RemoteObjectId<StructureLink>>, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

        let mut structure_data = self.structure_data.maybe_access(
            |d| game::time() - d.last_updated >= 10 && has_visibility,
            || Self::create_structure_data(room_data),
        );
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
        if let Ok(all_links) = self.get_all_links(system_data) {
            let transfer_queue = &mut system_data.transfer_queue;

            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Link Transfer",
                room_data: &*system_data.room_data,
            };

            for link_id in all_links {
                if let Some(link) = link_id.resolve() {
                    if link.cooldown() == 0 && link.store().get(ResourceType::Energy).unwrap_or(0) > 0 {
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
                                    link_pos.into(),
                                    target_filters::link,
                                )
                            })
                            .next();

                        if let Some((pickup, delivery)) = best_transfer {
                            transfer_queue.register_pickup(&pickup);
                            transfer_queue.register_delivery(&delivery);

                            //TODO: Validate there isn't non-energy in here?
                            let transfer_amount = delivery
                                .resources()
                                .get(&ResourceType::Energy)
                                .map(|entries| entries.iter().map(|entry| entry.amount()).sum())
                                .unwrap_or(0);

                            let _ = delivery.target().link_transfer_energy_amount(&link, transfer_amount);
                        }
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

        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility")?;
        let sources = static_visibility_data.sources();

        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility")?;
        let likely_owned_room = dynamic_visibility_data.updated_within(2000)
            && (dynamic_visibility_data.owner().mine() || dynamic_visibility_data.reservation().mine());
        let has_visibility = dynamic_visibility_data.visible();

        let mut structure_data = self.structure_data.maybe_access(
            |d| game::time() - d.last_updated >= 10 && has_visibility,
            || Self::create_structure_data(room_data),
        );
        let structure_data = structure_data.get();

        if structure_data.is_none() {
            system_data.visibility.request(VisibilityRequest::new(
                room_data.name,
                VISIBILITY_PRIORITY_CRITICAL,
                VisibilityRequestFlags::ALL,
            ));

            return Ok(());
        }

        let structure_data = structure_data.ok_or("Expected structure data")?;

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

            for home_room_entity in self.home_room_datas.iter() {
                let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

                //TODO: Use find route plus cache.
                let room_offset_distance = home_room_data.name - source_id.pos().room_name();
                let room_manhattan_distance = room_offset_distance.0.abs() + room_offset_distance.1.abs();

                if (source_containers.is_empty() && source_links.is_empty())
                    || (room_manhattan_distance == 0 && total_harvesting_creeps == 0)
                    || (room_manhattan_distance > 0 && !any_home_room_has_storage)
                {
                    let current_source_room_harvesters = creep_data
                        .home_rooms_to_harvesters
                        .get(home_room_entity)
                        .iter()
                        .flat_map(|e| e.iter())
                        .filter(|e| source_harvesters.contains(e))
                        .count();

                    //TODO: Compute correct number of harvesters to use for source.
                    let desired_harvesters = 4;

                    if current_source_room_harvesters < desired_harvesters {
                        let body_definition = SpawnBodyDefinition {
                            maximum_energy: if total_harvesting_creeps == 0 {
                                home_room.energy_available().max(SPAWN_ENERGY_CAPACITY)
                            } else {
                                home_room.energy_capacity_available()
                            },
                            minimum_repeat: Some(1),
                            maximum_repeat: Some(5),
                            pre_body: &[],
                            repeat_body: &[Part::Move, Part::Move, Part::Carry, Part::Work],
                            post_body: &[],
                        };

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

            // Estimate the lead time needed to spawn a replacement miner and
            // have it walk to the source. We compute the body that would be
            // spawned (using the best home room's energy capacity) and derive
            // spawn_ticks + travel_ticks so the replacement arrives just as the
            // current miner expires.
            let miner_replacement_lead = {
                let energy_capacity = if likely_owned_room {
                    SOURCE_ENERGY_CAPACITY
                } else {
                    SOURCE_ENERGY_NEUTRAL_CAPACITY
                };
                let energy_per_tick = (energy_capacity as f32) / (ENERGY_REGEN_TIME as f32);
                let work_parts_per_tick = (energy_per_tick / (HARVEST_POWER as f32)).ceil() as usize;

                // Try each home room and pick the smallest lead time (the
                // fastest replacement determines when we need to start).
                self.home_room_datas
                    .iter()
                    .filter_map(|home_room_entity| {
                        let home_room_data = system_data.room_data.get(*home_room_entity)?;
                        let home_room = game::rooms().get(home_room_data.name)?;

                        let body_definition = if source_id.pos().room_name() == home_room_data.name {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: Some(work_parts_per_tick),
                                pre_body: &[Part::Move, Part::Carry],
                                repeat_body: &[Part::Work],
                                post_body: &[],
                            }
                        } else {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: Some(work_parts_per_tick),
                                pre_body: &[Part::Carry],
                                repeat_body: &[Part::Move, Part::Work],
                                post_body: &[],
                            }
                        };

                        let body = crate::creep::spawning::create_body(&body_definition).ok()?;
                        Some(replacement_lead_ticks(&body, source_id.pos(), structure_data))
                    })
                    .min()
                    .unwrap_or(MIN_REPLACEMENT_LEAD_TICKS)
                    .max(MIN_REPLACEMENT_LEAD_TICKS)
            };

            let alive_source_miners = source_container_miners
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
                    let token = system_data.spawn_queue.token();

                    for home_room_entity in self.home_room_datas.iter() {
                        let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                        let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

                        let energy_capacity = if likely_owned_room {
                            SOURCE_ENERGY_CAPACITY
                        } else {
                            SOURCE_ENERGY_NEUTRAL_CAPACITY
                        };

                        let energy_per_tick = (energy_capacity as f32) / (ENERGY_REGEN_TIME as f32);
                        let work_parts_per_tick = (energy_per_tick / (HARVEST_POWER as f32)).ceil() as usize;

                        let body_definition = if link.pos().room_name() == home_room_data.name {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: Some(work_parts_per_tick),
                                pre_body: &[Part::Move, Part::Carry],
                                repeat_body: &[Part::Work],
                                post_body: &[],
                            }
                        } else {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: Some(work_parts_per_tick),
                                pre_body: &[Part::Carry],
                                repeat_body: &[Part::Move, Part::Work],
                                post_body: &[],
                            }
                        };

                        if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                            //TODO: Should the container be removed from the available ones?
                            let target_container = available_containers_for_source_miners.next();

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

                let available_containers_for_source_miners = source_containers.iter().filter(|container| {
                    creep_data
                        .containers_to_source_miners
                        .get(container)
                        .map(|miners| !miners.iter().any(|miner| alive_source_miners.contains(miner)))
                        .unwrap_or(true)
                });

                for container in available_containers_for_source_miners {
                    let token = system_data.spawn_queue.token();

                    for home_room_entity in self.home_room_datas.iter() {
                        let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                        let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

                        let energy_capacity = if likely_owned_room {
                            SOURCE_ENERGY_CAPACITY
                        } else {
                            SOURCE_ENERGY_NEUTRAL_CAPACITY
                        };

                        let energy_per_tick = (energy_capacity as f32) / (ENERGY_REGEN_TIME as f32);
                        let work_parts_per_tick = (energy_per_tick / (HARVEST_POWER as f32)).ceil() as usize;

                        let body_definition = if container.pos().room_name() == home_room_data.name {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: Some(work_parts_per_tick),
                                pre_body: &[Part::Move],
                                repeat_body: &[Part::Work],
                                post_body: &[],
                            }
                        } else {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: Some(work_parts_per_tick + 1),
                                pre_body: &[Part::Carry],
                                repeat_body: &[Part::Move, Part::Work],
                                post_body: &[],
                            }
                        };

                        if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                            let spawn_request = SpawnRequest::new(
                                format!("Container Miner - Source: {}", source_id.id()),
                                &body,
                                SPAWN_PRIORITY_HIGH,
                                Some(token),
                                Self::create_handle_container_miner_spawn(mission_entity, StaticMineTarget::Source(*source_id), *container),
                            );

                            system_data.spawn_queue.request(*home_room_entity, spawn_request);
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

            let mineral_replacement_lead = {
                self.home_room_datas
                    .iter()
                    .filter_map(|home_room_entity| {
                        let home_room_data = system_data.room_data.get(*home_room_entity)?;
                        let home_room = game::rooms().get(home_room_data.name)?;

                        let body_definition = if mineral_id.pos().room_name() == home_room_data.name {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: None,
                                pre_body: &[],
                                repeat_body: &[Part::Work, Part::Work, Part::Move],
                                post_body: &[],
                            }
                        } else {
                            SpawnBodyDefinition {
                                maximum_energy: home_room.energy_capacity_available(),
                                minimum_repeat: Some(1),
                                maximum_repeat: None,
                                pre_body: &[],
                                repeat_body: &[Part::Move, Part::Work],
                                post_body: &[],
                            }
                        };

                        let body = crate::creep::spawning::create_body(&body_definition).ok()?;
                        Some(replacement_lead_ticks(&body, mineral_id.pos(), structure_data))
                    })
                    .min()
                    .unwrap_or(MIN_REPLACEMENT_LEAD_TICKS)
                    .max(MIN_REPLACEMENT_LEAD_TICKS)
            };

            let alive_mineral_miners = mineral_miners
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
                .collect_vec();

            let available_containers_for_miners = container_ids.iter().filter(|container| {
                creep_data
                    .containers_to_mineral_miners
                    .get(container)
                    .map(|miners| !miners.iter().any(|miner| alive_mineral_miners.contains(miner)))
                    .unwrap_or(true)
            });

            //
            // Spawn mineral container miners.
            //

            for container in available_containers_for_miners {
                let token = system_data.spawn_queue.token();

                for home_room_entity in self.home_room_datas.iter() {
                    let home_room_data = system_data.room_data.get(*home_room_entity).ok_or("Expected home room data")?;
                    let home_room = game::rooms().get(home_room_data.name).ok_or("Expected home room")?;

                    //TODO: Compute correct body type.
                    //TODO: Base this on road presence.
                    let body_definition = if container.pos().room_name() == home_room_data.name {
                        SpawnBodyDefinition {
                            maximum_energy: home_room.energy_capacity_available(),
                            minimum_repeat: Some(1),
                            maximum_repeat: None,
                            pre_body: &[],
                            repeat_body: &[Part::Work, Part::Work, Part::Move],
                            post_body: &[],
                        }
                    } else {
                        SpawnBodyDefinition {
                            maximum_energy: home_room.energy_capacity_available(),
                            minimum_repeat: Some(1),
                            maximum_repeat: None,
                            pre_body: &[],
                            repeat_body: &[Part::Move, Part::Work],
                            post_body: &[],
                        }
                    };

                    if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                        let spawn_request = SpawnRequest::new(
                            format!("Container Miner - Extractor: {}", extractor_id.id()),
                            &body,
                            SPAWN_PRIORITY_LOW,
                            Some(token),
                            Self::create_handle_container_miner_spawn(
                                mission_entity,
                                StaticMineTarget::Mineral(*mineral_id, *extractor_id),
                                *container,
                            ),
                        );

                        system_data.spawn_queue.request(*home_room_entity, spawn_request);
                    }
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
                    let container_used_capacity = container.store().get_used_capacity(None);
                    if container_used_capacity > 0 {
                        let container_store_capacity = container.store().get_capacity(None);

                        let storage_fraction = (container_used_capacity as f32) / (container_store_capacity as f32);
                        let priority = if storage_fraction > 0.75 {
                            TransferPriority::Medium
                        } else if storage_fraction > 0.5 {
                            TransferPriority::Low
                        } else {
                            TransferPriority::None
                        };

                        for resource in container.store().store_types() {
                            let resource_amount = container.store().get_used_capacity(Some(resource));
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
                    let container_used_capacity = container.store().get_used_capacity(Some(ResourceType::Energy));
                    let container_available_capacity = container.store().get_capacity(Some(ResourceType::Energy));
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

                    let container_used_capacity = container.store().get_used_capacity(Some(ResourceType::Energy));
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

                for resource in container.store().store_types() {
                    let resource_amount = container.store().get_used_capacity(Some(resource));
                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Container(*container_id),
                        resource,
                        TransferPriority::None,
                        resource_amount,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }
        }
    }

    fn request_transfer_for_spawns(transfer: &mut dyn TransferRequestSystem, spawns: &[RemoteObjectId<StructureSpawn>]) {
        for spawn_id in spawns.iter() {
            if let Some(spawn) = spawn_id.resolve() {
                let free_capacity = spawn.store().get_free_capacity(Some(ResourceType::Energy));
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
                let free_capacity = extension.store().get_free_capacity(Some(ResourceType::Energy));
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

                for resource in storage.store().store_types() {
                    let resource_amount = storage.store().get_used_capacity(Some(resource));
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

                let free_capacity = storage.store().get_capacity(None) - used_capacity;

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
                let free_capacity = link.store().get_free_capacity(Some(ResourceType::Energy));

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

                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));

                if used_capacity > 0 {
                    let available_capacity = link.store().get_capacity(Some(ResourceType::Energy));
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
                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));

                if used_capacity > 0 {
                    let available_capacity = link.store().get_capacity(Some(ResourceType::Energy));
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
                let free_capacity = link.store().get_free_capacity(Some(ResourceType::Energy));

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

                let used_capacity = link.store().get_used_capacity(Some(ResourceType::Energy));

                let transfer_request = TransferWithdrawRequest::new(
                    TransferTarget::Link(link.remote_id()),
                    ResourceType::Energy,
                    TransferPriority::None,
                    used_capacity,
                    TransferType::Use,
                );

                transfer.request_withdraw(transfer_request);
            }
        }
    }

    fn request_transfer_for_ruins(transfer: &mut dyn TransferRequestSystem, room: &Room) {
        for ruin in room.find(find::RUINS, None) {
            let ruin_id = ruin.remote_id();

            for resource in ruin.store().store_types() {
                let resource_amount = ruin.store().get_used_capacity(Some(resource));
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
        for tombstone in room.find(find::TOMBSTONES, None) {
            let tombstone_id = tombstone.remote_id();

            for resource in tombstone.store().store_types() {
                let resource_amount = tombstone.store().get_used_capacity(Some(resource));

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
        for dropped_resource in room.find(find::DROPPED_RESOURCES, None) {
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
            let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

            let mut structure_data = structure_data.maybe_access(
                |d| game::time() - d.last_updated >= 10 && has_visibility,
                || Self::create_structure_data(room_data),
            );
            let Some(structure_data) = structure_data.get() else {
                return Ok(());
            };

            Self::request_transfer_for_spawns(transfer, &structure_data.spawns);
            Self::request_transfer_for_extension(transfer, &structure_data.extensions);
            Self::request_transfer_for_storage(transfer, &structure_data.storage);
            Self::request_transfer_for_containers(transfer, structure_data);

            if let Some(room) = game::rooms().get(room_data.name) {
                Self::request_transfer_for_ruins(transfer, &room);
                Self::request_transfer_for_tombstones(transfer, &room);
                Self::request_transfer_for_dropped_resources(transfer, &room);
            }

            Ok(())
        })
    }

    fn transfer_request_link_generator(room_entity: Entity, structure_data: Rc<RefCell<Option<StructureData>>>) -> TransferQueueGenerator {
        Box::new(move |system, transfer, _room_name| {
            let room_data = system.get_room_data(room_entity).ok_or("Expected room data")?;
            let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

            let mut structure_data = structure_data.maybe_access(
                |d| game::time() - d.last_updated >= 10 && has_visibility,
                || Self::create_structure_data(room_data),
            );
            let Some(structure_data) = structure_data.get() else {
                return Ok(());
            };

            Self::request_transfer_for_source_links(transfer, structure_data);
            Self::request_transfer_for_storage_links(transfer, structure_data);
            Self::request_transfer_for_controller_links(transfer, structure_data);

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

    fn summarize(&self) -> crate::visualization::SummaryContent {
        use crate::visualization::SummaryContent;
        let miners = self.source_container_miners.len() + self.source_link_miners.len();
        SummaryContent::Lines {
            header: "Local Supply".to_string(),
            items: vec![
                format!("Miners: {}", miners),
                format!("Harvesters: {}", self.harvesters.len()),
                format!("Minerals: {}", self.mineral_container_miners.len()),
            ],
        }
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

        //TODO: Split generators in to single flag.
        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            Self::transfer_request_haul_generator(self.room_data, self.structure_data.clone()),
        );

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL | TransferTypeFlags::LINK | TransferTypeFlags::USE,
            Self::transfer_request_link_generator(self.room_data, self.structure_data.clone()),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let creep_data = self.create_creep_data(system_data)?;

        if self.allow_spawning {
            self.spawn_creeps(system_data, mission_entity, &creep_data)?;
        }

        self.link_transfer(system_data)?;

        Ok(MissionResult::Running)
    }
}
