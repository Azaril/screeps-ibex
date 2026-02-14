use super::data::*;
use super::missionsystem::*;
use crate::room::roomplansystem::*;
use crate::serialize::*;
use screeps::*;
use screeps_common::NEIGHBORS_8;
use screeps_foreman::plan::{BuildStep, CleanupFilter, ExecutionFilter, ExistingStructure};
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Game-aware execution filter for plan construction.
///
/// Implements [`ExecutionFilter`] with policy decisions that depend on
/// live game state:
/// - Walls/ramparts are deferred until the room reaches a minimum RCL.
/// - Roads are deferred until at least one adjacent non-road structure exists.
/// - The total number of in-flight construction sites is capped at
///   [`MAX_CONSTRUCTION_SITES`].
struct ConstructionFilter<'a> {
    room: &'a Room,
    room_level: u8,
    min_rcl_for_walls: u8,
    /// Number of construction sites remaining before hitting the cap.
    remaining_sites: i32,
}

impl<'a> ConstructionFilter<'a> {
    fn new(room: &'a Room, room_level: u8, current_sites: usize) -> Self {
        let max_sites = crate::features::features().construction.max_construction_sites;

        ConstructionFilter {
            room,
            room_level,
            min_rcl_for_walls: 4,
            remaining_sites: max_sites - current_sites as i32,
        }
    }
}

impl<'a> ExecutionFilter for ConstructionFilter<'a> {
    fn should_place(&self, step: &BuildStep) -> bool {
        // Skip if the structure already exists or already has a construction
        // site at this location — placing would be a no-op but would consume
        // a slot in the remaining_sites budget.
        if structure_or_site_exists(step.location, step.structure_type, self.room) {
            return false;
        }

        if self.remaining_sites <= 0 {
            return false;
        }

        // Defer walls/ramparts until the room reaches min_rcl_for_walls.
        if (step.structure_type == StructureType::Wall || step.structure_type == StructureType::Rampart)
            && self.room_level < self.min_rcl_for_walls
        {
            return false;
        }

        // Defer roads that don't have any adjacent built structure yet.
        if step.structure_type == StructureType::Road && !has_adjacent_built_structure(step.location, self.room) {
            return false;
        }

        true
    }

    fn added_placement(&mut self, _step: &BuildStep) {
        self.remaining_sites -= 1;
    }
}

/// Check if a location has any adjacent built structure (or road) that
/// justifies placing a road here.
///
/// Returns `true` if any neighbor has a non-wall structure (including
/// roads). This allows road networks to grow outward from structures:
/// the first road tile is adjacent to a building, subsequent tiles are
/// adjacent to that road, and so on across ticks.
fn has_adjacent_built_structure(loc: screeps_common::Location, room: &Room) -> bool {
    let room_name = room.name();
    for &(dx, dy) in &NEIGHBORS_8 {
        let nx = loc.x() as i16 + dx as i16;
        let ny = loc.y() as i16 + dy as i16;
        if !(0..50).contains(&nx) || !(0..50).contains(&ny) {
            continue;
        }
        let pos = RoomPosition::new(nx as u8, ny as u8, room_name);
        let structures = room.look_for_at(look::STRUCTURES, &pos);
        for structure in &structures {
            let st = structure.structure_type();
            if st != StructureType::Wall {
                return true;
            }
        }
    }
    false
}

/// Check if a structure of the given type already exists (built or as a
/// construction site) at the given location.
///
/// Used to skip no-op placements so they don't consume the construction
/// site budget.
fn structure_or_site_exists(loc: screeps_common::Location, structure_type: StructureType, room: &Room) -> bool {
    let pos = RoomPosition::new(loc.x(), loc.y(), room.name());

    let structures = room.look_for_at(look::STRUCTURES, &pos);
    if structures.iter().any(|s| s.structure_type() == structure_type) {
        return true;
    }

    let sites = room.look_for_at(look::CONSTRUCTION_SITES, &pos);
    if sites.iter().any(|s| s.structure_type() == structure_type) {
        return true;
    }

    false
}

/// Game-aware cleanup filter for plan removal.
///
/// Implements [`CleanupFilter`] with policy decisions that depend on
/// live game state:
/// - Spawns are only removed if at least one other spawn will remain
///   in the room after the removal, ensuring the room is never left
///   without a spawn.
struct RemovalFilter {
    /// Number of spawns remaining in the room. Starts at the current
    /// total and is decremented each time a spawn removal is committed.
    remaining_spawns: u32,
}

impl RemovalFilter {
    fn new(room: &Room) -> Self {
        let remaining_spawns = room.find(find::MY_SPAWNS, None).len() as u32;

        RemovalFilter { remaining_spawns }
    }
}

impl CleanupFilter for RemovalFilter {
    fn should_remove(&self, structure: &ExistingStructure) -> bool {
        if structure.structure_type == StructureType::Spawn {
            self.remaining_spawns > 1
        } else {
            true
        }
    }

    fn added_removal(&mut self, structure: &ExistingStructure) {
        if structure.structure_type == StructureType::Spawn {
            self.remaining_spawns -= 1;
        }
    }
}

#[derive(ConvertSaveload)]
pub struct ConstructionMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ConstructionMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ConstructionMission::new(owner, room_data);

        builder
            .with(MissionData::Construction(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> ConstructionMission {
        ConstructionMission {
            owner: owner.into(),
            room_data,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ConstructionMission {
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
        "Construction".to_string()
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text("Construction".to_string())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;
        let room_level = room.controller().map(|c| c.level()).unwrap_or(0);

        let request_plan = if let Some(room_plan_data) = system_data.room_plan_data.get(self.room_data) {
            if let Some(plan) = room_plan_data.plan() {
                if game::time().is_multiple_of(50) {
                    if crate::features::features().construction.execute {
                        let construction_sites = room_data.get_construction_sites().ok_or("Expected construction sites")?;
                        let mut filter = ConstructionFilter::new(&room, room_level, construction_sites.len());
                        let ops = plan.get_build_operations(room_level, &mut filter);
                        screeps_foreman::plan::execute_operations(&room, &ops);
                    }

                    if crate::features::features().construction.cleanup {
                        let structures = room_data.get_structures().ok_or("Expected structures")?;
                        let snapshot = screeps_foreman::plan::snapshot_structures(structures.all());
                        let mut removal_filter = RemovalFilter::new(&room);
                        let ops = plan.get_cleanup_operations(&snapshot, room_level, &mut removal_filter);
                        screeps_foreman::plan::execute_operations(&room, &ops);
                    }
                }

                false
            } else {
                crate::features::features().construction.allow_replan
            }
        } else {
            true
        };

        if request_plan || crate::features::features().construction.force_plan {
            system_data.room_plan_queue.request(RoomPlanRequest::new(self.room_data, 1.0));
        }

        Ok(MissionResult::Running)
    }
}
