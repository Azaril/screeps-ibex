use crate::remoteobjectid::*;
use crate::room::data::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use std::cell::*;
use std::collections::HashMap;
use std::rc::*;

pub type MineralExtractorPair = (RemoteObjectId<Mineral>, RemoteObjectId<StructureExtractor>);

#[derive(Clone, Serialize, Deserialize)]
pub struct StructureData {
    pub last_updated: u32,
    pub sources_to_containers: HashMap<RemoteObjectId<Source>, Vec<RemoteObjectId<StructureContainer>>>,
    pub sources_to_links: HashMap<RemoteObjectId<Source>, Vec<RemoteObjectId<StructureLink>>>,
    pub storage_links: Vec<RemoteObjectId<StructureLink>>,
    pub mineral_extractors_to_containers: HashMap<MineralExtractorPair, Vec<RemoteObjectId<StructureContainer>>>,
    pub controllers_to_containers: HashMap<RemoteObjectId<StructureController>, Vec<RemoteObjectId<StructureContainer>>>,
    pub controller_links: Vec<RemoteObjectId<StructureLink>>,
    pub containers: Vec<RemoteObjectId<StructureContainer>>,
    pub spawns: Vec<RemoteObjectId<StructureSpawn>>,
    pub extensions: Vec<RemoteObjectId<StructureExtension>>,
    pub storage: Vec<RemoteObjectId<StructureStorage>>,
    /// Precomputed pathfinding distance from the nearest spawn to each
    /// source/mineral position. Populated in `create_structure_data` using
    /// `pathfinder::search` so the per-tick lead time calculation is pure
    /// arithmetic.
    pub nearest_spawn_distances: HashMap<screeps::Position, u32>,
}

/// World resource that caches `StructureData` per room. Each room gets a
/// single `Rc<RefCell<Option<StructureData>>>` that is shared across all
/// missions operating in that room. The cache is lazily populated on demand
/// and refreshed when stale (every 10+ ticks with visibility).
#[derive(Default)]
pub struct SupplyStructureCache {
    rooms: HashMap<RoomName, Rc<RefCell<Option<StructureData>>>>,
}

impl SupplyStructureCache {
    pub fn new() -> Self {
        Self { rooms: HashMap::new() }
    }

    /// Get (or create) the shared `Rc<RefCell<Option<StructureData>>>` for a
    /// room. The returned `Rc` can be captured by generator closures and
    /// shared across missions.
    pub fn get_room(&mut self, room_name: RoomName) -> Rc<RefCell<Option<StructureData>>> {
        self.rooms.entry(room_name).or_insert_with(|| Rc::new(RefCell::new(None))).clone()
    }
}

pub fn create_structure_data(room_data: &RoomData) -> Option<StructureData> {
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

    let spawn_remote_ids: Vec<_> = spawns.iter().map(|s| s.remote_id()).collect();

    // Precompute pathfinding distances from nearest spawn to each source and
    // mineral position. Uses `pathfinder::search` with road-aware costs so the
    // path follows roads where available.
    let nearest_spawn_distances =
        compute_nearest_spawn_distances(&spawn_remote_ids, sources.iter().map(|s| s.pos()), minerals.iter().map(|m| m.pos()));

    Some(StructureData {
        last_updated: game::time(),
        sources_to_containers,
        sources_to_links,
        storage_links,
        mineral_extractors_to_containers,
        controllers_to_containers,
        controller_links,
        containers: containers.iter().map(|s| s.remote_id()).collect(),
        spawns: spawn_remote_ids,
        extensions: extensions.iter().map(|e| e.remote_id()).collect(),
        storage: storages.iter().map(|s| s.remote_id()).collect(),
        nearest_spawn_distances,
    })
}

/// Compute the pathfinding distance from the nearest spawn to each target
/// position. Uses `pathfinder::search` with `plain_cost=2`, `swamp_cost=10`
/// so that roads (cost 1) are preferred, matching real creep movement.
fn compute_nearest_spawn_distances(
    spawns: &[RemoteObjectId<StructureSpawn>],
    source_positions: impl Iterator<Item = screeps::Position>,
    mineral_positions: impl Iterator<Item = screeps::Position>,
) -> HashMap<screeps::Position, u32> {
    let mut distances = HashMap::new();

    if spawns.is_empty() {
        return distances;
    }

    let target_positions: Vec<_> = source_positions.chain(mineral_positions).collect();

    for target_pos in target_positions {
        let min_distance = spawns
            .iter()
            .map(|spawn_id| {
                let options = pathfinder::SearchOptions::default().plain_cost(2).swamp_cost(10);
                let result = pathfinder::search(spawn_id.pos(), target_pos, 1, Some(options));
                if result.incomplete() {
                    u32::MAX
                } else {
                    result.path().len() as u32
                }
            })
            .min()
            .unwrap_or(0);

        distances.insert(target_pos, min_distance);
    }

    distances
}
