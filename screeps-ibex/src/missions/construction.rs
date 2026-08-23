use super::data::*;
use super::missionsystem::*;
use crate::room::roomplansystem::*;
use crate::serialize::*;
use crate::spawnsystem::site_blocks_spawn;
use screeps::*;
use screeps_common::Location as PlanLocation;
use screeps_foreman::plan::{BuildStep, CleanupFilter, ExecutionFilter, ExistingStructure};
use screeps_foreman::terrain::FastRoomTerrain;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

thread_local! {
    /// REC-070 warn-once latch (EP-1.1c: a function-local once-per-VM log latch —
    /// LOGGING ONLY, never control flow; the sealing DEFERRAL fires every pass
    /// regardless). Rooms whose plan already logged a "would seal the last spawn birth
    /// tile" defect, so the warn is emitted at most once per room per VM instead of every
    /// 50-tick construction pass (EP-3.5: repeating warnings get a once latch). Rebuilt
    /// empty on a VM reset, which re-surfaces a persistent plan defect exactly once — the
    /// intended cadence.
    static SPAWN_SEAL_WARNED_ROOMS: std::cell::RefCell<HashSet<RoomName>> = std::cell::RefCell::new(HashSet::new());
}

/// Emit the REC-070 plan-defect warn at most once per room per VM (see
/// [`SPAWN_SEAL_WARNED_ROOMS`]).
fn warn_spawn_seal_once(room: RoomName, structure_type: StructureType, xy: (u8, u8)) {
    let first = SPAWN_SEAL_WARNED_ROOMS.with(|w| w.borrow_mut().insert(room));
    if first {
        log::warn!(
            "Construction {}: deferring {:?} site at ({},{}) — it would seal a spawn's last free birth tile (same-tick spawn-start race / single-approach geometry, REC-050). Warn-once per VM (REC-070); if seen the placement is being deferred every pass — the plan wants an obstacle on a spawn's only exit.",
            room,
            structure_type,
            xy.0,
            xy.1
        );
    }
}

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
    /// Tiles adjacent to a spawn that is mid-spawn this tick. An obstacle-type
    /// site placed here would seal the in-flight creep's birth exit and wedge
    /// the spawn permanently (see [`Self::new`]); such sites are deferred until
    /// the spawn is idle.
    spawning_exit_tiles: HashSet<(u8, u8)>,
    /// Per-spawn free-birth-tile budgets (REC-050, see [`Self::new`] and
    /// [`placement_seals_spawn`]) for EVERY my-spawn, idle ones included.
    /// Consumed by [`Self::added_placement`] as the batch approves obstacle
    /// sites, so a batch can never collectively seal what no single placement
    /// would.
    spawn_birth_tiles: Vec<SpawnBirthTiles>,
}

/// Free birth tiles around one spawn, tracked through a placement batch
/// (REC-050). Split into the two tiers `safe_spawn_directions`
/// (spawnsystem.rs) actually draws from: planner-approved approaches (Tier 1)
/// and interior fallback neighbours (Tier 2). Tier 3 (unconstrained) is
/// wedge-immune — the engine re-evaluates every tile at birth — so it needs
/// no budget.
struct SpawnBirthTiles {
    /// Planner-approved approach tiles adjacent to this spawn that are
    /// currently free. While one is free, the spawn's direction set is the
    /// approaches (Tier 1), which plan-driven sites never target — so
    /// interior placements are safe.
    free_approaches: HashSet<(u8, u8)>,
    /// Free interior (non-border) neighbours — the Tier-2 fallback direction
    /// set used when no approach is adjacent/free (off-plan spawns, young
    /// rooms). Tier 2 additionally skips dead-end pockets, which this budget
    /// deliberately ignores: counting a pocket as an escape slightly
    /// under-defers in that corner, accepted since the planner's
    /// ReachabilityLayer keeps a real approach existing.
    free_interior: HashSet<(u8, u8)>,
}

/// REC-050 kernel: whether placing an obstacle-type site at `tile` would seal
/// this spawn's LAST viable birth tile. `spawnCreep`'s `directions` are built
/// at spawn START from tick-start data — a site created this tick is not
/// visible in game state until the NEXT tick — and are checked only at BIRTH,
/// so a batch that seals every tile the direction set can use wedges the
/// newborn permanently (+1 spawnTime/tick forever; unlike a camping creep, a
/// site never moves). Deferring ONLY the last-tile placement (instead of every
/// spawn neighbour whenever the room might spawn) keeps spawn-adjacent
/// extensions buildable in busy colonies, whose spawn queues are rarely empty.
fn placement_seals_spawn(tile: (u8, u8), spawn: &SpawnBirthTiles) -> bool {
    if spawn.free_approaches.contains(&tile) {
        // Never consume the last free planner approach: the spawn-start
        // direction set (Tier 1) still SEES it as free and would be fully
        // sealed at birth. Plans never place obstacles on their own approach
        // tiles, so this firing at all is a plan defect (logged loudly).
        return spawn.free_approaches.len() == 1;
    }
    if spawn.free_approaches.is_empty() && spawn.free_interior.contains(&tile) {
        // No free approach ⇒ the direction set is the Tier-2 interior
        // fallback; keep at least one member free through the batch.
        return spawn.free_interior.len() == 1;
    }
    false
}

/// Whether a creep can NOT be born onto this tile: terrain wall, an obstacle
/// structure, or an obstacle-type construction site. Structures reuse
/// `site_blocks_spawn` on the structure type — identical to spawnsystem's
/// `structure_blocks_spawn` except built ramparts, which `site_blocks_spawn`
/// treats as standable unconditionally (a HOSTILE rampart adjacent to our own
/// spawn is not a configuration worth a second predicate). Creeps are
/// deliberately ignored: they move, a site does not.
///
/// REC-070 — ACCEPTED RESIDUAL: `safe_spawn_directions` (spawnsystem.rs) IS
/// creep-aware when it picks a birth direction, but this birth-tile budget is
/// not, so a creep CAMPING a spawn's sole free approach while an interior
/// obstacle placement lands the same tick is not modelled here — the budget
/// counts the camped approach as free and may let the interior placement seal
/// the last direction the (creep-aware) spawn-start set would actually offer.
/// The residual is narrow (a creep must sit on the last free approach on the one
/// tick a sealing site is approved) and self-heals (creeps move; the site is
/// deferred next pass once the approach clears, or the spawn-start set already
/// re-routed around the creep). Treating a creep-occupied approach as non-free
/// here would be strictly safer but would over-defer spawn-adjacent extensions
/// whenever a creep merely passes a spawn — a worse trade in busy colonies.
fn tile_blocks_birth(room: &Room, terrain: &FastRoomTerrain, x: u8, y: u8) -> bool {
    if terrain.is_wall(x, y) {
        return true;
    }
    let pos = RoomPosition::new(x, y, room.name());
    if room
        .look_for_at(look::STRUCTURES, &pos)
        .iter()
        .any(|s| site_blocks_spawn(s.structure_type()))
    {
        return true;
    }
    room.look_for_at(look::CONSTRUCTION_SITES, &pos)
        .iter()
        .any(|s| site_blocks_spawn(s.structure_type()))
}

impl<'a> ConstructionFilter<'a> {
    fn new(room: &'a Room, room_level: u8, spawn_approaches: &[PlanLocation]) -> Self {
        // Collect the exit tiles of every spawn that is mid-spawn this tick.
        // `spawnCreep`'s directional constraint is applied only at BIRTH, so a
        // tile that is free when a spawn STARTS (and therefore passed the
        // site-aware direction check) can be sealed mid-spawn by a freshly placed
        // obstacle site — and a blocked exit slips spawnTime +1/tick forever,
        // wedging the spawn permanently. Deferring an obstacle site on these
        // tiles until the spawn is idle closes the "construction site placed while
        // a spawn is pending" half of the RCL-up deadlock. It is harmless: such a
        // tile is a non-approach neighbour the plan wants an extension on, so a
        // one-cycle delay only postpones it until the spawn next goes idle.
        //
        // REC-050 extends this to the SAME-TICK START race: `RunMissionSystem`
        // (this filter) runs BEFORE `SpawnQueueSystem` (game_loop.rs), so a
        // spawn that is IDLE right now can start spawning later this very tick
        // with a direction set built from tick-start data that cannot see the
        // sites this batch approves. The mid-spawn set above cannot cover it, so
        // every my-spawn additionally gets a free-birth-tile budget and
        // `should_place` defers any placement that would consume a spawn's last
        // free tile (see `placement_seals_spawn`). This is unconditional rather
        // than gated on "spawn queue non-empty": the queue is only partially
        // populated while missions are still running, and an obstacle on a
        // spawn's only exit is just as wedging whenever it NEXT spawns.
        let mut spawning_exit_tiles = HashSet::new();
        let mut spawn_birth_tiles = Vec::new();
        let terrain = FastRoomTerrain::new(room.get_terrain().get_raw_buffer().to_vec());
        for spawn in room.find(find::MY_SPAWNS, None) {
            let p = spawn.pos();
            let loc = PlanLocation::from_xy(p.x().u8(), p.y().u8());
            if spawn.spawning().is_some() {
                for n in loc.neighbors() {
                    spawning_exit_tiles.insert((n.x(), n.y()));
                }
            }

            let mut free_approaches = HashSet::new();
            let mut free_interior = HashSet::new();
            for n in loc.neighbors() {
                let (nx, ny) = (n.x(), n.y());
                if tile_blocks_birth(room, &terrain, nx, ny) {
                    continue;
                }
                if spawn_approaches.iter().any(|a| a.x() == nx && a.y() == ny) {
                    free_approaches.insert((nx, ny));
                } else if nx > 0 && ny > 0 && nx < 49 && ny < 49 {
                    // Border tiles are excluded like Tier 2 does — a direction
                    // set never offers them, so they are not escapes.
                    free_interior.insert((nx, ny));
                }
            }
            spawn_birth_tiles.push(SpawnBirthTiles {
                free_approaches,
                free_interior,
            });
        }

        ConstructionFilter {
            room,
            room_level,
            min_rcl_for_walls: 4,
            placed_this_batch: Vec::new(),
            spawning_exit_tiles,
            spawn_birth_tiles,
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

        // Defer an obstacle-type site that would seal the exit of a spawn that is
        // mid-spawn this tick — placing it would wedge the spawn permanently (the
        // directional constraint is applied only at birth). See `new`.
        if site_blocks_spawn(step.structure_type) {
            let xy = (step.location.x(), step.location.y());
            if self.spawning_exit_tiles.contains(&xy) {
                return false;
            }
            // REC-050: also defer a placement that would seal ANY spawn's last
            // free birth tile — the mid-spawn set above cannot see a spawn that
            // STARTS spawning later this same tick with a direction set built
            // from tick-start data (see `new`). Loud (EP-3.1) but warn-ONCE per
            // room per VM (REC-070 / EP-3.5): the defer itself fires every pass;
            // only the warn is latched so a persistent single-approach plan
            // defect doesn't spam the log every 50-tick construction cycle.
            if self.spawn_birth_tiles.iter().any(|s| placement_seals_spawn(xy, s)) {
                warn_spawn_seal_once(self.room.name(), step.structure_type, xy);
                return false;
            }
        }

        true
    }

    fn added_placement(&mut self, step: &BuildStep) {
        self.placed_this_batch.push(step.location);
        // REC-050: an approved obstacle placement consumes its tile from every
        // spawn's free-birth budget, so a LATER placement in the same batch
        // cannot collectively seal a direction set that no single placement
        // would (the spawn-start direction set sees NONE of this batch).
        if site_blocks_spawn(step.structure_type) {
            let xy = (step.location.x(), step.location.y());
            for spawn in self.spawn_birth_tiles.iter_mut() {
                spawn.free_approaches.remove(&xy);
                spawn.free_interior.remove(&xy);
            }
        }
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
                        let mut filter = ConstructionFilter::new(&room, room_level, &plan.spawn_approaches);
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
                // deliberately NOT gated by any discretionary-replan flag (the backoff in
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles(approaches: &[(u8, u8)], interior: &[(u8, u8)]) -> SpawnBirthTiles {
        SpawnBirthTiles {
            free_approaches: approaches.iter().copied().collect(),
            free_interior: interior.iter().copied().collect(),
        }
    }

    /// Pin (REC-050 single-approach geometry): with no free planner approach,
    /// an obstacle placement on the spawn's ONLY free interior birth tile is
    /// deferred — the spawn-start direction set (built from tick-start data,
    /// blind to sites created this tick) would be fully sealed at birth and
    /// the spawn wedges permanently. A placement anywhere else never defers.
    #[test]
    fn sealing_the_last_free_birth_tile_is_deferred() {
        let single = tiles(&[], &[(25, 24)]);
        assert!(placement_seals_spawn((25, 24), &single));
        assert!(!placement_seals_spawn((30, 30), &single), "an unrelated tile never defers");

        // With a second free tile the direction set survives the placement.
        let double = tiles(&[], &[(25, 24), (26, 25)]);
        assert!(!placement_seals_spawn((25, 24), &double));
        assert!(!placement_seals_spawn((26, 25), &double));
    }

    /// Pin (REC-050): a batch can seal collectively what no single placement
    /// would. After the first of two free tiles is consumed
    /// (`added_placement`), the second placement IS the last tile and must
    /// defer — the spawn-start direction set is blind to BOTH just-approved
    /// sites and would be fully sealed at birth.
    #[test]
    fn batch_placements_consume_the_birth_budget() {
        let mut budget = tiles(&[], &[(25, 24), (26, 25)]);
        assert!(!placement_seals_spawn((25, 24), &budget), "first of two is allowed");
        budget.free_interior.remove(&(25, 24)); // what added_placement does
        assert!(placement_seals_spawn((26, 25), &budget), "second (now last) must defer");
    }

    /// Pin (REC-050): a free planner approach makes interior placements safe —
    /// the spawn-start direction set is then Tier 1 (the approaches), which
    /// plan-driven sites never target. The LAST free approach itself is still
    /// protected (defence-in-depth against a defective plan that places an
    /// obstacle on its own approach tile).
    #[test]
    fn free_approach_exempts_interior_but_is_itself_protected() {
        let budget = tiles(&[(25, 26)], &[(25, 24)]);
        assert!(
            !placement_seals_spawn((25, 24), &budget),
            "interior tile is safe while an approach is free (Tier-1 direction set)"
        );
        assert!(
            placement_seals_spawn((25, 26), &budget),
            "the sole free approach must never be sealed"
        );

        // Two free approaches: consuming one is fine.
        let two = tiles(&[(25, 26), (24, 25)], &[]);
        assert!(!placement_seals_spawn((25, 26), &two));
        assert!(!placement_seals_spawn((24, 25), &two));
    }

    /// Pin (REC-070 / EP-3.5): the plan-defect warn is latched once per room per
    /// VM. `warn_spawn_seal_once` returns `true` only the FIRST time a room is
    /// seen, so the every-50-tick construction pass logs the defect once instead
    /// of spamming. Distinct rooms each warn once; a repeat for a warned room is
    /// suppressed. (The deferral itself — `placement_seals_spawn` above — is
    /// unlatched and fires every pass; only the log is deduped.)
    #[test]
    fn plan_defect_warn_is_latched_once_per_room() {
        let a: RoomName = "W1N1".parse().unwrap();
        let b: RoomName = "W2N2".parse().unwrap();
        // Fresh latch for this test (thread-local; other tests may have touched it).
        SPAWN_SEAL_WARNED_ROOMS.with(|w| w.borrow_mut().clear());

        let first_a = SPAWN_SEAL_WARNED_ROOMS.with(|w| w.borrow_mut().insert(a));
        let repeat_a = SPAWN_SEAL_WARNED_ROOMS.with(|w| w.borrow_mut().insert(a));
        let first_b = SPAWN_SEAL_WARNED_ROOMS.with(|w| w.borrow_mut().insert(b));

        assert!(first_a, "first sighting of a room warns");
        assert!(!repeat_a, "a repeat sighting of the same room is suppressed");
        assert!(first_b, "a distinct room warns once of its own");
    }
}
