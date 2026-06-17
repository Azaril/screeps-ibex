use super::data::*;
use super::missionsystem::*;
use crate::room::roomplansystem::*;
use crate::serialize::*;
use screeps::*;
use screeps_common::Location as PlanLocation;
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
/// - Roads are deferred until at least one adjacent road or structure
///   exists (built, under construction, or approved earlier in this
///   batch). This lets an entire road chain be placed in a single
///   execution cycle rather than growing one tile per cycle.
/// - The total number of in-flight construction sites is capped at
///   [`MAX_CONSTRUCTION_SITES`].
struct ConstructionFilter<'a> {
    room: &'a Room,
    room_level: u8,
    min_rcl_for_walls: u8,
    /// Locations approved for placement earlier in this batch. Used so
    /// that road adjacency checks can see sites we have already decided
    /// to place (but that don't exist in the game world yet).
    placed_this_batch: Vec<PlanLocation>,
}

impl<'a> ConstructionFilter<'a> {
    fn new(room: &'a Room, room_level: u8) -> Self {
        ConstructionFilter {
            room,
            room_level,
            min_rcl_for_walls: 4,
            placed_this_batch: Vec::new(),
        }
    }
}

impl<'a> ExecutionFilter for ConstructionFilter<'a> {
    fn should_place(&self, step: &BuildStep) -> bool {
        // Skip if the structure already exists or already has a construction
        // site at this location.
        //
        // NOTE: the in-flight site CAP is enforced at execution time
        // (`execute_operations(.., max_creates)`), charged on SUCCESS — NOT
        // here at queue time. A queue-time cap let failing ops (RCL gate,
        // InvalidTarget) burn the budget before they failed, starving the
        // valid ops behind them and stalling construction entirely.
        if structure_or_site_exists(step.location, step.structure_type, self.room) {
            return false;
        }

        // Defer walls/ramparts until the room reaches min_rcl_for_walls.
        if (step.structure_type == StructureType::Wall || step.structure_type == StructureType::Rampart)
            && self.room_level < self.min_rcl_for_walls
        {
            return false;
        }

        // Defer roads that don't have any adjacent road or structure yet.
        // Checks built structures, construction sites, and sites approved
        // earlier in this batch so an entire road chain can be placed in
        // one cycle.
        if step.structure_type == StructureType::Road && !has_adjacent_structure_or_site(step.location, self.room, &self.placed_this_batch)
        {
            return false;
        }

        true
    }

    fn added_placement(&mut self, step: &BuildStep) {
        self.placed_this_batch.push(step.location);
    }
}

/// Check if a location has any adjacent structure, construction site, or
/// batch-placed site that justifies placing a road here.
///
/// Returns `true` if any of the 8 neighbors has:
/// - A built non-wall structure (including roads), OR
/// - A construction site (any type), OR
/// - A site approved earlier in this execution batch.
///
/// This allows road networks to be placed outward from the hub in a
/// single execution cycle: the first road tile is adjacent to a built
/// structure, subsequent tiles are adjacent to the road site placed
/// moments earlier in the same batch.
fn has_adjacent_structure_or_site(loc: PlanLocation, room: &Room, placed_this_batch: &[PlanLocation]) -> bool {
    let room_name = room.name();
    for neighbor in loc.neighbors() {
        // Check if a site was approved earlier in this batch at this neighbor.
        if placed_this_batch.contains(&neighbor) {
            return true;
        }

        let pos = RoomPosition::new(neighbor.x(), neighbor.y(), room_name);

        // Check for built structures (excluding natural walls).
        let structures = room.look_for_at(look::STRUCTURES, &pos);
        for structure in &structures {
            if structure.structure_type() != StructureType::Wall {
                return true;
            }
        }

        // Check for construction sites (any type counts — a road next to
        // an extension under construction should still be placed).
        let sites = room.look_for_at(look::CONSTRUCTION_SITES, &pos);
        if !sites.is_empty() {
            return true;
        }
    }
    false
}

/// Check if a structure of the given type already exists (built or as a
/// construction site) at the given location.
///
/// Used to skip no-op placements so they don't consume the construction
/// site budget.
fn structure_or_site_exists(loc: PlanLocation, structure_type: StructureType, room: &Room) -> bool {
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

    fn get_room(&self) -> Option<Entity> {
        Some(self.room_data)
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
                    if system_data.features.construction.execute {
                        let construction_sites = room_data.get_construction_sites().ok_or("Expected construction sites")?;
                        let existing_sites = construction_sites.len();
                        // Success-charged budget: place up to (cap - current) NEW
                        // sites this cycle, skipping (not counting) failures.
                        let max_new = (system_data.features.construction.max_construction_sites - existing_sites as i32).max(0) as u32;
                        let mut filter = ConstructionFilter::new(&room, room_level);
                        let ops = plan.get_build_operations(room_level, &mut filter);
                        let create_ops = ops
                            .iter()
                            .filter(|o| matches!(o, screeps_foreman::plan::PlanOperation::CreateSite { .. }))
                            .count();
                        let created = screeps_foreman::plan::execute_operations(&room, &ops, Some(max_new));
                        // Diagnostic: distinguishes "no build ops generated"
                        // (no plan / everything filtered: RCL gate, site cap,
                        // already-built) from "ops generated but placement
                        // failed" (see the per-failure warn in execute_operations).
                        log::info!(
                            "Construction {} (RCL {}): {} create-ops, {} sites created, {} sites already in room (cap {})",
                            room_data.name,
                            room_level,
                            create_ops,
                            created,
                            existing_sites,
                            system_data.features.construction.max_construction_sites
                        );
                    }

                    if system_data.features.construction.cleanup {
                        let structures = room_data.get_structures().ok_or_else(|| {
                            let msg = format!("Expected structures - Room: {}", room_data.name);
                            log::warn!("{} at {}:{}", msg, file!(), line!());
                            msg
                        })?;
                        let snapshot = screeps_foreman::plan::snapshot_structures(structures.all());
                        let mut removal_filter = RemovalFilter::new(&room);
                        let ops = plan.get_cleanup_operations(&snapshot, room_level, &mut removal_filter);
                        screeps_foreman::plan::execute_operations(&room, &ops, None);
                    }
                }

                false
            } else {
                // No usable plan (Failed with no last-known-good). Recovery is
                // unconditional (S3) -- a plan-less owned room must re-plan so it
                // regains construction + authoritative spawn approaches; this is
                // deliberately NOT gated by `allow_replan` (the backoff in
                // roomplansystem still prevents thrashing).
                if game::time().is_multiple_of(50) {
                    log::info!(
                        "Construction {}: no usable plan yet — requesting (re)plan, placing nothing this cycle",
                        room_data.name
                    );
                }
                true
            }
        } else {
            true
        };

        if request_plan || system_data.features.construction.force_plan {
            system_data.room_plan_queue.request(RoomPlanRequest::new(self.room_data, 1.0));
        }

        Ok(MissionResult::Running)
    }
}
