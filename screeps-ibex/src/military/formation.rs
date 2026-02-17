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
pub fn issue_virtual_anchor_movement(
    squad: &mut SquadContext,
    destination: Position,
    movement: &mut MovementData<Entity>,
) {
    // Advance the virtual position (cohesion checks, mode transitions).
    advance_squad_virtual_position(squad, destination);

    // Read the resulting virtual position.
    let virtual_pos = squad
        .squad_path
        .as_ref()
        .map(|p| p.virtual_pos)
        .unwrap_or(destination);

    let layout = squad.layout.clone();

    // Issue MoveTo for each living member toward their formation offset.
    for member in squad.members.iter() {
        let target_tile = if let Some(ref layout) = layout {
            virtual_anchor_target(virtual_pos, layout, member.formation_slot)
                .unwrap_or(virtual_pos)
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
    let living_members: Vec<(usize, Option<Position>)> = squad
        .members
        .iter()
        .map(|m| (m.formation_slot, m.position))
        .collect();

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

    // Update destination if changed.
    if let Some(path) = squad.squad_path.as_mut() {
        path.destination = destination;
    }

    let virtual_pos = squad
        .squad_path
        .as_ref()
        .map(|p| p.virtual_pos)
        .unwrap_or(destination);

    // Check formation cohesion and decide whether to advance the virtual position.
    let living_count = living_members.len();
    let in_formation_count = living_members
        .iter()
        .filter(|(slot, pos)| {
            if let Some(target) = virtual_anchor_target(virtual_pos, &layout, *slot) {
                pos.map(|p| p.get_range_to(target) <= 1).unwrap_or(false)
            } else {
                false
            }
        })
        .count();

    let all_in_formation = in_formation_count == living_count;

    let should_advance = match squad.formation_mode {
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
            true
        }
    };

    if should_advance {
        advance_virtual_pos(squad, destination);
    }
}

/// Initialize the squad path if it doesn't exist yet.
fn init_squad_path_if_needed(
    squad: &mut SquadContext,
    living_members: &[(usize, Option<Position>)],
    destination: Position,
) {
    if squad.squad_path.is_none() {
        let start_pos = living_members
            .iter()
            .find_map(|(_, pos)| *pos)
            .unwrap_or(destination);

        squad.squad_path = Some(SquadPath {
            destination,
            room_route: Vec::new(),
            virtual_pos: start_pos,
            stuck_ticks: 0,
        });
    }
}

/// Advance the virtual position one step toward the destination.
fn advance_virtual_pos(squad: &mut SquadContext, destination: Position) {
    let path = match squad.squad_path.as_mut() {
        Some(p) => p,
        None => return,
    };

    let current = path.virtual_pos;

    // If already at destination, nothing to do.
    if current == destination {
        path.stuck_ticks = 0;
        return;
    }

    // Simple one-step advance toward destination.
    let dx = (destination.x().u8() as i32 - current.x().u8() as i32).signum();
    let dy = (destination.y().u8() as i32 - current.y().u8() as i32).signum();

    let new_x = (current.x().u8() as i32 + dx).clamp(0, 49) as u8;
    let new_y = (current.y().u8() as i32 + dy).clamp(0, 49) as u8;

    if let (Ok(rx), Ok(ry)) = (RoomCoordinate::new(new_x), RoomCoordinate::new(new_y)) {
        let new_pos = Position::new(rx, ry, current.room_name());

        if new_pos == current {
            path.stuck_ticks += 1;
        } else {
            path.virtual_pos = new_pos;
            path.stuck_ticks = 0;
        }
    } else {
        path.stuck_ticks += 1;
    }
}

/// Issue flee movement for all squad members using virtual anchor approach.
/// Each member independently flees from hostile positions.
pub fn issue_virtual_anchor_flee(
    squad: &SquadContext,
    hostile_positions: &[Position],
    flee_range: u32,
    movement: &mut MovementData<Entity>,
) {
    let targets: Vec<FleeTarget> = hostile_positions
        .iter()
        .map(|&pos| FleeTarget {
            pos,
            range: flee_range,
        })
        .collect();

    if targets.is_empty() {
        return;
    }

    for member in squad.members.iter() {
        movement
            .flee(member.entity, targets.clone())
            .range(flee_range);
    }
}
