use crate::remoteobjectid::*;
use crate::room::data::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use std::cell::*;
use std::collections::HashMap;
use std::rc::*;

pub type MineralExtractorPair = (RemoteObjectId<Mineral>, RemoteObjectId<StructureExtractor>);

/// Range around the controller within which a container is classified as a
/// controller (upgrade) container. This IS the planner's placement range —
/// re-exported from screeps-foreman so classification and placement can
/// never drift apart (the live 2-vs-3 mismatch bug class is
/// unrepresentable). Classifying with a smaller radius mis-buckets a
/// planner-placed container as a generic storage container: its deposit
/// requests are then registered at `TransferPriority::None`, which never
/// pairs with the storage's `TransferPriority::None` withdraw, so no energy
/// is hauled to the controller and upgraders idle.
pub use screeps_foreman::constants::CONTROLLER_CONTAINER_MAX_RANGE as CONTROLLER_CONTAINER_RANGE;

/// Same contract for the controller link (foreman places it within this
/// range; we classify any link within it as controller-feeding).
use screeps_foreman::constants::CONTROLLER_LINK_MAX_RANGE;

/// Whether a container at `container_pos` serves the controller at
/// `controller_pos` (see [`CONTROLLER_CONTAINER_RANGE`]).
fn is_controller_container(container_pos: screeps::Position, controller_pos: screeps::Position) -> bool {
    container_pos.in_range_to(controller_pos, CONTROLLER_CONTAINER_RANGE)
}

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
            let nearby_link = links
                .iter()
                .find(|link| link.pos().in_range_to(controller.pos(), CONTROLLER_LINK_MAX_RANGE));

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
                .find(|container| is_controller_container(container.pos(), controller.pos()));

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
///
/// Ops-capped (P1.B1 / IBEX-035, ADR 0004 step 1): this runs spawns ×
/// targets searches per structure-data refresh; spawn and targets share
/// a room, so 1000 ops is generous (engine default is 2000). A capped
/// incomplete search reports `u32::MAX` exactly like an unreachable
/// target — callers already treat that as "very far".
fn compute_nearest_spawn_distances(
    spawns: &[RemoteObjectId<StructureSpawn>],
    source_positions: impl Iterator<Item = screeps::Position>,
    mineral_positions: impl Iterator<Item = screeps::Position>,
) -> HashMap<screeps::Position, u32> {
    const NEAREST_SPAWN_MAX_OPS: u32 = 1000;

    let mut distances = HashMap::new();

    if spawns.is_empty() {
        return distances;
    }

    let target_positions: Vec<_> = source_positions.chain(mineral_positions).collect();

    for target_pos in target_positions {
        let min_distance = spawns
            .iter()
            .map(|spawn_id| {
                // P1.B4: drawn from the mission ops pool; a zero grant
                // degrades to the capped-incomplete u32::MAX semantic.
                let ops = crate::pathbudget::take(NEAREST_SPAWN_MAX_OPS);
                if ops == 0 {
                    return u32::MAX;
                }
                let options = pathfinder::SearchOptions::default()
                    .plain_cost(2)
                    .swamp_cost(10)
                    .max_ops(ops);
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

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::RoomCoordinate;

    // Pin: the controller-container classification radius must cover the room
    // planner's whole upgrade area (range 3 from the controller -- foreman's
    // ControllerInfraLayer / the game's upgrade range). At the old radius of 2
    // a planner-placed container at range 3 fell into the generic storage-
    // container bucket, whose TransferPriority::None deposit never pairs with
    // the storage's TransferPriority::None withdraw -- observed live as a
    // controller container stuck at 0 energy beside a 250k+ storage while
    // upgraders idled (W7N4: container (36,9), controller (39,12)).

    fn pos(x: u8, y: u8) -> screeps::Position {
        screeps::Position::new(
            RoomCoordinate::new(x).expect("valid coordinate"),
            RoomCoordinate::new(y).expect("valid coordinate"),
            "W7N4".parse().expect("valid room name"),
        )
    }

    #[test]
    fn classifies_planner_placed_container_at_upgrade_area_edge() {
        // The live repro geometry: range 3 (Chebyshev) from the controller.
        assert!(is_controller_container(pos(36, 9), pos(39, 12)));
    }

    #[test]
    fn classifies_adjacent_and_near_containers() {
        assert!(is_controller_container(pos(38, 12), pos(39, 12)));
        assert!(is_controller_container(pos(37, 11), pos(39, 12)));
    }

    #[test]
    fn rejects_containers_outside_the_upgrade_area() {
        // Range 4: not reachable by an upgrader working the controller.
        assert!(!is_controller_container(pos(35, 12), pos(39, 12)));
        assert!(!is_controller_container(pos(35, 8), pos(39, 12)));
    }
}
