use super::squad::*;
use screeps::*;
use screeps_rover::*;
use specs::Entity;

/// Offset positions for a 2x2 box formation relative to the leader at (0,0).
/// Leader is top-left; other members fill right, below, and diagonal.
const QUAD_OFFSETS: [(i32, i32); 4] = [(0, 0), (1, 0), (0, 1), (1, 1)];

/// Offset positions for a line formation (leader in front, others behind).
const LINE_OFFSETS: [(i32, i32); 4] = [(0, 0), (0, 1), (0, 2), (0, 3)];

/// Calculate the target position for a squad member based on formation type
/// and the leader's position.
///
/// Returns `None` if the member index is out of range or position is invalid.
pub fn formation_position(
    leader_pos: Position,
    formation: FormationType,
    member_index: usize,
) -> Option<Position> {
    let offsets = match formation {
        FormationType::None => return Some(leader_pos),
        FormationType::Line => &LINE_OFFSETS[..],
        FormationType::Box2x2 => &QUAD_OFFSETS[..],
    };

    if member_index >= offsets.len() {
        return None;
    }

    let (dx, dy) = offsets[member_index];
    let new_x = leader_pos.x().u8() as i32 + dx;
    let new_y = leader_pos.y().u8() as i32 + dy;

    // Ensure position is within room bounds (1..48 for walkable tiles).
    if !(1..=48).contains(&new_x) || !(1..=48).contains(&new_y) {
        return None;
    }

    Some(Position::new(
        RoomCoordinate::new(new_x as u8).ok()?,
        RoomCoordinate::new(new_y as u8).ok()?,
        leader_pos.room_name(),
    ))
}

/// Check if a 2x2 footprint at (x, y) is valid (all four tiles are within room bounds).
pub fn is_valid_quad_position(x: u8, y: u8) -> bool {
    // All four tiles of the 2x2 must be within walkable room bounds (1..48).
    (1..=47).contains(&x) && (1..=47).contains(&y)
}

/// Apply a 2x2 cost matrix overlay for quad formation movement.
/// Marks tiles where the 2x2 footprint would overlap walls or unwalkable terrain
/// as impassable (cost 255).
///
/// This should be applied to the cost matrix before pathfinding for the quad leader.
pub fn apply_quad_cost_overlay(
    cost_matrix: &mut LocalCostMatrix,
    room_name: RoomName,
) {
    let terrain = game::map::get_room_terrain(room_name);

    if let Some(terrain) = terrain {
        // For each potential leader position, check if the 2x2 footprint is valid.
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

/// Apply hostile tower range costs to a cost matrix.
/// Tiles within tower range get increased cost to encourage pathfinding around them.
pub fn apply_tower_avoidance_costs(
    cost_matrix: &mut LocalCostMatrix,
    tower_positions: &[Position],
    room_name: RoomName,
) {
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

/// Issue movement requests for all squad members based on formation and squad state.
///
/// Movement strategy by squad type:
///
/// - **Solo**: simple `MoveTo` toward target.
/// - **Duo**: leader `MoveTo`, follower `Follow` with desired range 1.
///   The follower independently pathfinds to stay close to the leader.
///   Pull is only used when the follower is fatigued (heavy TOUGH bodies).
/// - **Quad**: leader `MoveTo` (with 2×2-aware cost matrix applied externally),
///   followers use `Follow` with a `desired_offset` so each member targets a
///   unique tile in the 2×2 footprint.  The offset is computed relative to
///   the leader's *resolved* next position (topological sort guarantees the
///   leader resolves first).  When the desired tile is blocked or out of
///   bounds the movement system falls back to the nearest tile within range,
///   so the quad can still squeeze through narrow corridors.
pub fn issue_squad_movement(
    squad: &SquadContext,
    target_pos: Position,
    movement: &mut MovementData<Entity>,
) {
    let living_members: Vec<_> = squad.members.iter().filter(|m| m.alive).collect();

    if living_members.is_empty() {
        return;
    }

    match squad.squad_type {
        SquadType::Solo => {
            if let Some(member) = living_members.first() {
                movement
                    .move_to(member.entity, target_pos)
                    .range(1)
                    .priority(MovementPriority::High);
            }
        }
        SquadType::Duo => {
            if let Some(leader) = living_members.first() {
                // Leader pathfinds to the target.
                movement
                    .move_to(leader.entity, target_pos)
                    .range(1)
                    .priority(MovementPriority::High);

                // Follower uses Follow to stay within range 1 of the leader.
                // The movement system decides the best adjacent tile; no rigid
                // offset is imposed so the pair can navigate narrow corridors.
                for follower in living_members.iter().skip(1) {
                    movement
                        .follow(follower.entity, leader.entity)
                        .range(1)
                        .priority(MovementPriority::High);
                }
            }
        }
        SquadType::Quad => {
            if let Some(leader) = living_members.first() {
                // Leader pathfinds to the target. The caller should apply a
                // 2×2-aware cost matrix so the leader only picks paths where
                // the full quad footprint fits.
                movement
                    .move_to(leader.entity, target_pos)
                    .range(1)
                    .priority(MovementPriority::High);

                // Followers use Follow with a desired_offset so each one
                // targets a unique tile in the 2×2 formation:
                //   leader (0,0)  |  follower 1 (1,0)
                //   follower 2 (0,1) | follower 3 (1,1)
                //
                // The offset is applied to the leader's resolved destination,
                // so the formation shape is maintained as the group moves.
                // When the offset tile is blocked the movement system falls
                // back to any tile within range 1, allowing the quad to
                // deform temporarily in tight spaces.
                for (i, follower) in living_members.iter().skip(1).enumerate() {
                    // QUAD_OFFSETS[0] is the leader; followers use indices 1..3.
                    let (dx, dy) = if i + 1 < QUAD_OFFSETS.len() {
                        QUAD_OFFSETS[i + 1]
                    } else {
                        // More members than formation slots -- just follow.
                        (0, 0)
                    };

                    movement
                        .follow(follower.entity, leader.entity)
                        .range(1)
                        .desired_offset(dx, dy)
                        .priority(MovementPriority::High);
                }
            }
        }
    }
}

/// Issue flee movement for all squad members away from hostile positions.
///
/// The leader flees and all followers follow the leader (rather than each
/// member fleeing independently, which would scatter the squad).
/// For quads, followers use desired offsets to maintain the 2×2 shape
/// during retreat.
pub fn issue_squad_flee(
    squad: &SquadContext,
    hostile_positions: &[Position],
    flee_range: u32,
    movement: &mut MovementData<Entity>,
) {
    let targets: Vec<FleeTarget> = hostile_positions
        .iter()
        .map(|&pos| FleeTarget { pos, range: flee_range })
        .collect();

    let living_members: Vec<_> = squad.members.iter().filter(|m| m.alive).collect();

    if living_members.is_empty() {
        return;
    }

    // Leader flees from hostiles.
    if let Some(leader) = living_members.first() {
        movement
            .flee(leader.entity, targets)
            .range(flee_range);

        // Followers follow the leader rather than fleeing independently.
        // This keeps the squad together during retreat instead of scattering.
        let is_quad = matches!(squad.squad_type, SquadType::Quad);

        for (i, follower) in living_members.iter().skip(1).enumerate() {
            if is_quad {
                if let Some(&(dx, dy)) = QUAD_OFFSETS.get(i + 1) {
                    movement
                        .follow(follower.entity, leader.entity)
                        .range(1)
                        .desired_offset(dx, dy)
                        .priority(MovementPriority::High);
                } else {
                    movement
                        .follow(follower.entity, leader.entity)
                        .range(1)
                        .priority(MovementPriority::High);
                }
            } else {
                movement
                    .follow(follower.entity, leader.entity)
                    .range(1)
                    .priority(MovementPriority::High);
            }
        }
    }
}
