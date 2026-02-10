use super::data::*;
use super::missionsystem::*;
use crate::room::roomplansystem::*;
use crate::serialize::*;
use screeps::*;
use screeps_foreman::plan::{BuildStep, ExecutionFilter};
use screeps_foreman::terrain::NEIGHBORS_8;
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
struct ConstructionFilter<'a> {
    room: &'a Room,
    room_level: u8,
    min_rcl_for_walls: u8,
}

impl<'a> ConstructionFilter<'a> {
    fn new(room: &'a Room, room_level: u8) -> Self {
        ConstructionFilter {
            room,
            room_level,
            min_rcl_for_walls: 4,
        }
    }
}

impl<'a> ExecutionFilter for ConstructionFilter<'a> {
    fn should_place(&self, step: &BuildStep) -> bool {
        // Defer walls/ramparts until the room reaches min_rcl_for_walls.
        if (step.structure_type == StructureType::Wall
            || step.structure_type == StructureType::Rampart)
            && self.room_level < self.min_rcl_for_walls
        {
            return false;
        }

        // Defer roads that don't have any adjacent built structure yet.
        if step.structure_type == StructureType::Road
            && !has_adjacent_built_structure(step.location, self.room)
        {
            return false;
        }

        true
    }
}

/// Check if a location has any adjacent built structure (or road) that
/// justifies placing a road here.
///
/// Returns `true` if any neighbor has a non-wall structure (including
/// roads). This allows road networks to grow outward from structures:
/// the first road tile is adjacent to a building, subsequent tiles are
/// adjacent to that road, and so on across ticks.
fn has_adjacent_built_structure(
    loc: screeps_foreman::location::Location,
    room: &Room,
) -> bool {
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

                        const MAX_CONSTRUCTION_SITES: i32 = 10;

                        let max_placement = MAX_CONSTRUCTION_SITES - (construction_sites.len() as i32);

                        if max_placement > 0 {
                            let filter = ConstructionFilter::new(&room, room_level);
                            let ops = plan.get_build_operations(room_level, max_placement as u32, &filter);
                            screeps_foreman::plan::execute_operations(&room, &ops);
                        }
                    }

                    if crate::features::features().construction.cleanup {
                        let structures = room_data.get_structures().ok_or("Expected structures")?;
                        let snapshot = screeps_foreman::plan::snapshot_structures(structures.all());
                        let ops = plan.get_cleanup_operations(&snapshot, room_level);
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
