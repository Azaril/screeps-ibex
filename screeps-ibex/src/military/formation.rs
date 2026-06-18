use super::composition::*;
use super::squad::*;
use screeps::*;
use screeps_rover::*;
use specs::Entity;

/// Offset positions for a 2x2 box formation relative to the anchor at (0,0).
/// Anchor is top-left; other members fill right, below, and diagonal.
const QUAD_OFFSETS: [(i32, i32); 4] = [(0, 0), (1, 0), (0, 1), (1, 1)];

/// Offset positions for a line formation (front member first, others behind).
const LINE_OFFSETS: [(i32, i32); 4] = [(0, 0), (0, 1), (0, 2), (0, 3)];

/// Check if a 2x2 footprint at (x, y) is valid (all four tiles are within room bounds).
pub fn is_valid_quad_position(x: u8, y: u8) -> bool {
    // All four tiles of the 2x2 must be within walkable room bounds (1..48).
    (1..=47).contains(&x) && (1..=47).contains(&y)
}

/// Apply a 2x2 cost matrix overlay for quad formation movement.
/// Marks tiles where the 2x2 footprint would overlap walls or unwalkable terrain
/// as impassable (cost 255).
///
/// This should be applied to the cost matrix before pathfinding for the quad anchor.
pub fn apply_quad_cost_overlay(cost_matrix: &mut LocalCostMatrix, room_name: RoomName) {
    let terrain = game::map::get_room_terrain(room_name);

    if let Some(terrain) = terrain {
        // For each potential anchor position, check if the 2x2 footprint is valid.
        for x in 0u8..50 {
            for y in 0u8..50 {
                // Check if any of the 4 tiles in the 2x2 footprint is a wall.
                let mut blocked = false;

                for &(dx, dy) in &QUAD_OFFSETS {
                    let fx = x as i32 + dx;
                    let fy = y as i32 + dy;

                    if !(0..50).contains(&fx) || !(0..50).contains(&fy) {
                        blocked = true;
                        break;
                    }

                    if terrain.get(fx as u8, fy as u8) == Terrain::Wall {
                        blocked = true;
                        break;
                    }
                }

                if blocked {
                    if let Ok(xy) = RoomXY::checked_new(x, y) {
                        cost_matrix.set(xy, 255);
                    }
                }
            }
        }
    }
}

/// Apply a formation-aware cost matrix overlay for any formation shape.
/// Marks tiles where the formation footprint would overlap walls as impassable.
/// Works for any formation shape, not just 2x2.
pub fn apply_formation_cost_overlay(cost_matrix: &mut LocalCostMatrix, room_name: RoomName, layout: &FormationLayout) {
    if layout.offsets.len() <= 1 {
        return; // No overlay needed for single-member formations.
    }

    let terrain = game::map::get_room_terrain(room_name);

    if let Some(terrain) = terrain {
        for x in 0u8..50 {
            for y in 0u8..50 {
                let mut blocked = false;

                for &(dx, dy) in &layout.offsets {
                    let fx = x as i32 + dx;
                    let fy = y as i32 + dy;

                    if !(0..50).contains(&fx) || !(0..50).contains(&fy) {
                        blocked = true;
                        break;
                    }

                    if terrain.get(fx as u8, fy as u8) == Terrain::Wall {
                        blocked = true;
                        break;
                    }
                }

                if blocked {
                    if let Ok(xy) = RoomXY::checked_new(x, y) {
                        cost_matrix.set(xy, 255);
                    }
                }
            }
        }
    }
}

/// Apply hostile tower range costs to a cost matrix.
/// Tiles within tower range get increased cost to encourage pathfinding around them.
pub fn apply_tower_avoidance_costs(cost_matrix: &mut LocalCostMatrix, tower_positions: &[Position], room_name: RoomName) {
    for tower_pos in tower_positions {
        if tower_pos.room_name() != room_name {
            continue;
        }

        let tx = tower_pos.x().u8();
        let ty = tower_pos.y().u8();

        // Apply costs in concentric rings around the tower.
        for x in 0u8..50 {
            for y in 0u8..50 {
                let dx = (x as i32 - tx as i32).unsigned_abs();
                let dy = (y as i32 - ty as i32).unsigned_abs();
                let range = dx.max(dy);

                let additional_cost: u8 = if range <= 5 {
                    20 // Max damage range -- very expensive.
                } else if range <= 10 {
                    10 // Medium damage range.
                } else if range <= 20 {
                    5 // Low damage range.
                } else {
                    0
                };

                if additional_cost > 0 {
                    if let Ok(xy) = RoomXY::checked_new(x, y) {
                        let current = cost_matrix.get(xy);
                        if current < 255 {
                            cost_matrix.set(xy, current.saturating_add(additional_cost));
                        }
                    }
                }
            }
        }
    }
}

// ─── Virtual anchor movement (new) ─────────────────────────────────────────

/// Compute the target tile for a squad member given the virtual position
/// and the formation layout.
pub fn virtual_anchor_target(virtual_pos: Position, layout: &FormationLayout, formation_slot: usize) -> Option<Position> {
    let (dx, dy) = layout.get_offset(formation_slot);
    let new_x = virtual_pos.x().u8() as i32 + dx;
    let new_y = virtual_pos.y().u8() as i32 + dy;

    if !(0..50).contains(&new_x) || !(0..50).contains(&new_y) {
        return None;
    }

    Some(Position::new(
        RoomCoordinate::new(new_x as u8).ok()?,
        RoomCoordinate::new(new_y as u8).ok()?,
        virtual_pos.room_name(),
    ))
}

/// Issue movement requests for all squad members using the virtual anchor approach.
/// Every creep independently issues MoveTo toward their formation offset relative
/// to the virtual position. No Follow intents are used.
///
/// This is a convenience function that combines `advance_squad_virtual_position`
/// (strategic advancement) with per-member movement commands. When the mission
/// and job layers are split (mission advances, job moves), use
/// `advance_squad_virtual_position` and `virtual_anchor_target` separately.
pub fn issue_virtual_anchor_movement(squad: &mut SquadContext, destination: Position, movement: &mut MovementData<Entity>) {
    // Advance the virtual position (cohesion checks, mode transitions).
    advance_squad_virtual_position(squad, destination);

    // Read the resulting virtual position.
    let virtual_pos = squad.squad_path.as_ref().map(|p| p.anchor.virtual_pos).unwrap_or(destination);

    let layout = squad.layout.clone();

    // Issue MoveTo for each living member toward their formation offset.
    for member in squad.members.iter() {
        let target_tile = if let Some(ref layout) = layout {
            virtual_anchor_target(virtual_pos, layout, member.formation_slot).unwrap_or(virtual_pos)
        } else {
            destination
        };

        movement
            .move_to(member.entity, target_tile)
            .range(0)
            .priority(MovementPriority::High);
    }
}

/// Advance the squad's virtual position toward the destination, handling
/// formation cohesion checks and mode transitions. Call this once per squad
/// per tick from the mission layer. Individual creeps then read the resulting
/// `virtual_pos` from `SquadContext` and issue their own `move_to` toward
/// their formation offset.
///
/// `destination` is the strategic target the squad is moving toward (e.g.
/// the focus target position or room center).
pub fn advance_squad_virtual_position(squad: &mut SquadContext, destination: Position) {
    let living_members: Vec<(usize, Option<Position>)> = squad.members.iter().map(|m| (m.formation_slot, m.position)).collect();

    if living_members.is_empty() {
        return;
    }

    let layout = match &squad.layout {
        Some(l) => l.clone(),
        None => {
            // No layout -- just advance directly.
            init_squad_path_if_needed(squad, &living_members, destination);
            advance_virtual_pos(squad, destination);
            return;
        }
    };

    // Initialize squad path if needed.
    init_squad_path_if_needed(squad, &living_members, destination);

    // Update destination if changed (the anchor re-paths on a destination change).
    if let Some(path) = squad.squad_path.as_mut() {
        path.anchor.destination = destination;
    }

    let virtual_pos = squad.squad_path.as_ref().map(|p| p.anchor.virtual_pos).unwrap_or(destination);

    // Check formation cohesion and decide whether to advance the virtual position.
    let living_count = living_members.len();
    let in_formation_count = living_members
        .iter()
        .filter(|(slot, pos)| {
            if let Some(target) = virtual_anchor_target(virtual_pos, &layout, *slot) {
                match squad.formation_mode {
                    FormationMode::Strict => pos.map(|p| p.get_range_to(target) == 0).unwrap_or(false),
                    FormationMode::Loose => pos.map(|p| p.get_range_to(target) <= 1).unwrap_or(false),
                }
            } else {
                false
            }
        })
        .count();

    let all_in_formation = in_formation_count == living_count;

    // ── Room boundary cohesion: hold at room edges until squad is gathered ──
    //
    // When the virtual position is about to cross a room boundary (destination
    // is in a different room), require most members to be in the same room as
    // the virtual position before advancing across. This prevents faster
    // creeps from trickling into the next room while slower ones lag behind.
    let at_room_boundary = virtual_pos.room_name() != destination.room_name();
    let boundary_hold = if at_room_boundary && living_count > 1 {
        let vp_room = virtual_pos.room_name();
        let members_in_vp_room = living_members
            .iter()
            .filter(|(_, pos)| pos.map(|p| p.room_name() == vp_room).unwrap_or(false))
            .count();
        let members_already_crossed = living_members
            .iter()
            .filter(|(_, pos)| pos.map(|p| p.room_name() == destination.room_name()).unwrap_or(false))
            .count();

        // Allow crossing when:
        // - All members are in the virtual_pos room (full cohesion), OR
        // - At least 75% are in either the vp room or destination room AND
        //   the majority are near the boundary (within 8 tiles of the edge), OR
        // - The hold has lasted too long (STRICT_HOLD_MAX_TICKS) to avoid deadlock.
        let gathered_count = members_in_vp_room + members_already_crossed;
        let quorum_met = gathered_count as f32 >= living_count as f32 * STRICT_QUORUM_RATIO;

        // Count members near the relevant room edge.
        let members_near_edge = living_members
            .iter()
            .filter(|(_, pos)| {
                pos.map(|p| {
                    if p.room_name() != vp_room {
                        // Already in destination room -- they've crossed.
                        return true;
                    }
                    is_near_room_edge_toward(p, destination)
                })
                .unwrap_or(false)
            })
            .count();
        let near_edge_quorum = members_near_edge as f32 >= living_count as f32 * STRICT_QUORUM_RATIO;

        !(quorum_met && near_edge_quorum)
    } else {
        false
    };

    let should_advance = if boundary_hold {
        // Hold at the room boundary -- don't advance the virtual pos.
        // Still allow strict_hold_ticks to increment so we eventually
        // force through via the deadlock timeout.
        squad.strict_hold_ticks += 1;
        if squad.strict_hold_ticks >= STRICT_HOLD_MAX_TICKS * 2 {
            // Extended timeout: force advance to avoid permanent deadlock.
            squad.strict_hold_ticks = 0;
            true
        } else {
            false
        }
    } else {
        match squad.formation_mode {
            FormationMode::Strict => {
                if all_in_formation {
                    squad.strict_hold_ticks = 0;
                    true
                } else {
                    squad.strict_hold_ticks += 1;

                    if squad.strict_hold_ticks >= STRICT_HOLD_MAX_TICKS {
                        squad.formation_mode = FormationMode::Loose;
                        squad.strict_hold_ticks = 0;
                        true
                    } else {
                        squad.strict_hold_ticks >= STRICT_QUORUM_TICKS
                            && in_formation_count as f32 >= living_count as f32 * STRICT_QUORUM_RATIO
                    }
                }
            }
            FormationMode::Loose => {
                if squad.desired_formation_mode == FormationMode::Strict && all_in_formation {
                    squad.formation_mode = FormationMode::Strict;
                }

                // Enforce formation in loose mode too: wait for members to
                // be within range ≤ 1 of their formation slot (combat range).
                // Use a shorter hold timeout since this is already the
                // relaxed fallback mode.
                let loose_in_formation = living_members
                    .iter()
                    .filter(|(slot, pos)| {
                        if let Some(target) = virtual_anchor_target(virtual_pos, &layout, *slot) {
                            pos.map(|p| p.get_range_to(target) <= 1).unwrap_or(false)
                        } else {
                            false
                        }
                    })
                    .count();

                if loose_in_formation == living_count {
                    squad.strict_hold_ticks = 0;
                    true
                } else {
                    squad.strict_hold_ticks += 1;
                    // Force advance after a shorter timeout to avoid permanent blocks
                    // when pathfinding can't reach the exact formation tile.
                    squad.strict_hold_ticks >= STRICT_QUORUM_TICKS && loose_in_formation as f32 >= living_count as f32 * STRICT_QUORUM_RATIO
                }
            }
        }
    };

    if should_advance {
        advance_virtual_pos(squad, destination);
    }
}

/// Initialize the squad path if it doesn't exist yet.
fn init_squad_path_if_needed(squad: &mut SquadContext, living_members: &[(usize, Option<Position>)], destination: Position) {
    if squad.squad_path.is_none() {
        let start_pos = living_members.iter().find_map(|(_, pos)| *pos).unwrap_or(destination);

        squad.squad_path = Some(SquadPath {
            anchor: AnchorPath::new(start_pos, destination),
            room_route: Vec::new(),
        });
    }
}

/// The squad's bounding-box footprint `(w, h)` from its layout offsets — the size the anchor path
/// must fit so the box routes as a unit. Defaults to 1×1 (no layout).
fn squad_footprint(squad: &SquadContext) -> (u8, u8) {
    let offsets: &[(i32, i32)] = match &squad.layout {
        Some(l) => &l.offsets,
        None => return (1, 1),
    };
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (0i32, 0i32, 0i32, 0i32);
    for &(dx, dy) in offsets {
        min_x = min_x.min(dx);
        max_x = max_x.max(dx);
        min_y = min_y.min(dy);
        max_y = max_y.max(dy);
    }
    (((max_x - min_x + 1).max(1)) as u8, ((max_y - min_y + 1).max(1)) as u8)
}

/// Advance the virtual anchor one step toward the destination along a **cached, footprint-aware
/// path** (P2.M2) — the `rover::AnchorPath` mechanism, driven by the live server `PathFinder`
/// (`ScreepsPathfinder`) over the room cost matrix. The anchor pathfinds once and follows the cache,
/// re-pathing only on staleness; on a `Blocked` outcome it **holds** (anchor.stuck_ticks rises) for
/// the manager to respond to — it never degrades to a straight-line step into the obstacle.
///
/// The `room_callback` bakes **terrain walls** into the matrix so the footprint transform covers
/// them (the server PathFinder applies terrain per-tile, which would otherwise dodge the
/// footprint expansion). Cost-matrix/source/pathfinder are built ad-hoc (they read `game::*`
/// lazily). Validate behavior on the private server before relying on it live.
fn advance_virtual_pos(squad: &mut SquadContext, destination: Position) {
    // The cohesive footprint we WANT to route as. For a full quad this is the 2×2 box even when
    // the member layout is temporarily collapsed to a line for a corridor; for everything else it
    // is just the current layout's footprint. Probing with the tight footprint every tick gives a
    // stable "does the box fit here?" signal independent of the current member layout, so the
    // collapse/re-form below cannot oscillate.
    let layout_footprint = squad_footprint(squad);
    let is_quad = squad.members.len() >= 4;
    let tight_footprint = if is_quad { (2, 2) } else { layout_footprint };

    // Advance the anchor with the tight footprint; thread single-file when the box can't fit.
    // `tight_blocked` is the corridor signal that drives the member-layout collapse below.
    let tight_blocked = {
        let path = match squad.squad_path.as_mut() {
            Some(p) => p,
            None => return,
        };

        let mut cache = CostMatrixCache::default();
        let mut cms = CostMatrixSystem::new(&mut cache, Box::new(screeps_rover::screeps_impl::ScreepsCostMatrixDataSource));
        let opts = CostMatrixOptions::default();
        let mut pf = screeps_rover::screeps_impl::ScreepsPathfinder;
        let mut room_cb = |r: RoomName| {
            let mut matrix = cms.build_local_cost_matrix(r, &opts).ok()?;
            if let Some(terrain) = game::map::get_room_terrain(r) {
                for x in 0..ROOM_SIZE {
                    for y in 0..ROOM_SIZE {
                        if terrain.get(x, y) == Terrain::Wall {
                            if let Ok(xy) = RoomXY::checked_new(x, y) {
                                matrix.set(xy, u8::MAX);
                            }
                        }
                    }
                }
            }
            Some(matrix)
        };

        let outcome = path.anchor.advance(destination, tight_footprint, &mut pf, &mut room_cb);
        if outcome == AnchorOutcome::Blocked && tight_footprint != (1, 1) {
            // Corridor relax (P2.M3): the tight box can't fit → thread single-file (width-1).
            let _ = path.anchor.advance(destination, (1, 1), &mut pf, &mut room_cb);
            true
        } else {
            // A still-`Blocked` width-1 anchor holds (stuck_ticks rises) for the manager to respond to.
            false
        }
    };

    // Member-layout corridor switch (P2.M3): collapse a stuck box to single-file so members thread
    // the corridor behind the width-1 anchor, then re-form to the tight box the moment it fits
    // again — the transition back to a cohesive squad as soon as a group path exists.
    if let Some(new_layout) = corridor_layout_transition(squad.layout.as_ref().map(|l| l.shape), squad.members.len(), tight_blocked) {
        squad.layout = Some(new_layout);
        squad.compact_formation_slots();
    }
}

/// Decide the member layout for a quad given whether its tight (2×2 box) footprint is currently
/// blocked. Returns `Some(new_layout)` when the layout should change this tick, or `None` to keep
/// the current one.
///
/// - **Collapse**: a `Box2x2` whose footprint is blocked drops to a single-file `Line` so members
///   thread the corridor.
/// - **Re-form**: a `Line` whose box footprint fits again snaps back to `Box2x2` — the corridor →
///   cohesive transition, taken the instant a group path exists.
///
/// Scoped to full quads (≥4 members): every intended `Line` composition is a 2-member duo, so a
/// 4-member `Line` is always a collapsed quad and re-forming it can never clobber an intended line.
/// This is pure (no `game::*`) so the transition is unit-testable without the game runtime.
fn corridor_layout_transition(shape: Option<FormationShape>, member_count: usize, tight_blocked: bool) -> Option<FormationLayout> {
    if member_count < 4 {
        return None;
    }
    match shape {
        Some(FormationShape::Box2x2) if tight_blocked => Some(FormationLayout::line(member_count)),
        Some(FormationShape::Line) if !tight_blocked => Some(FormationLayout::box_2x2()),
        _ => None,
    }
}

const ROOM_SIZE: u8 = 50;

/// Check if a position is near the room edge leading toward a destination in
/// another room. "Near" means within 8 tiles of the relevant border.
fn is_near_room_edge_toward(pos: Position, destination: Position) -> bool {
    let (cur_wx, cur_wy) = pos.world_coords();
    let (dst_wx, dst_wy) = destination.world_coords();
    let pos_room = pos.room_name();
    let dst_room = destination.room_name();

    if pos_room == dst_room {
        return true; // Already in the destination room.
    }

    let x = pos.x().u8();
    let y = pos.y().u8();
    let near_threshold = 8;

    // Check which direction we need to go based on world coordinates.
    let room_dx = (dst_wx - cur_wx).signum();
    let room_dy = (dst_wy - cur_wy).signum();

    let near_x_edge = if room_dx > 0 {
        x >= 49 - near_threshold
    } else if room_dx < 0 {
        x <= near_threshold
    } else {
        true // Same x-axis; no x-boundary to cross.
    };

    let near_y_edge = if room_dy > 0 {
        y >= 49 - near_threshold
    } else if room_dy < 0 {
        y <= near_threshold
    } else {
        true // Same y-axis; no y-boundary to cross.
    };

    near_x_edge && near_y_edge
}

/// Issue flee movement for all squad members using virtual anchor approach.
/// Each member independently flees from hostile positions.
pub fn issue_virtual_anchor_flee(
    squad: &SquadContext,
    hostile_positions: &[Position],
    flee_range: u32,
    movement: &mut MovementData<Entity>,
) {
    let targets: Vec<FleeTarget> = hostile_positions.iter().map(|&pos| FleeTarget { pos, range: flee_range }).collect();

    if targets.is_empty() {
        return;
    }

    for member in squad.members.iter() {
        movement.flee(member.entity, targets.clone()).range(flee_range);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stuck quad box collapses to single-file, then snaps back to a box the moment the corridor
    /// opens — the loose→tight transition the movement overhaul is meant to make ASAP.
    #[test]
    fn quad_collapses_in_a_corridor_and_re_forms_when_clear() {
        // Box, tight footprint fits → no change.
        assert!(corridor_layout_transition(Some(FormationShape::Box2x2), 4, false).is_none());

        // Box, tight footprint blocked → collapse to a 4-long single-file line.
        let collapsed = corridor_layout_transition(Some(FormationShape::Box2x2), 4, true).expect("should collapse");
        assert_eq!(collapsed.shape, FormationShape::Line);
        assert_eq!(collapsed.offsets.len(), 4);

        // Line, still blocked → stay collapsed.
        assert!(corridor_layout_transition(Some(FormationShape::Line), 4, true).is_none());

        // Line, box fits again → re-form to the tight box immediately.
        let reformed = corridor_layout_transition(Some(FormationShape::Line), 4, false).expect("should re-form");
        assert_eq!(reformed.shape, FormationShape::Box2x2);
        assert_eq!(reformed.offsets, QUAD_OFFSETS.to_vec());
    }

    /// Duos (and any <4-member squad) are never touched: an intended 2-member `Line` must not be
    /// "re-formed" into a box, and a 2-member squad has no box to collapse.
    #[test]
    fn duos_are_left_alone() {
        assert!(corridor_layout_transition(Some(FormationShape::Line), 2, false).is_none());
        assert!(corridor_layout_transition(Some(FormationShape::Line), 2, true).is_none());
        assert!(corridor_layout_transition(Some(FormationShape::Box2x2), 3, true).is_none());
        assert!(corridor_layout_transition(Some(FormationShape::None), 1, true).is_none());
    }
}
