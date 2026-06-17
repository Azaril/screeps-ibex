//! Same-tile movement-conflict resolution — the engine `movement.js check` port. This is where
//! kiting and squad-cohesion fidelity come from: a "move if the tile is free" toy model would hide
//! exactly the failure class the sim exists to surface (a creep that "sat idle" is usually a
//! movement-conflict loss, not a decision bug).
//!
//! Engine fidelity (`src/processor/intents/movement.js`, ground truth `C:\code\screeps-engine`):
//! - **Eligibility** (`canMove`, lines 11-14): a creep moves only if it has a working MOVE part AND
//!   its fatigue was 0 at tick start.
//! - **Same-tile contention** (`check`, lines 104-150): when >1 creep targets a tile, the winner is
//!   chosen by, in order, `rate1` (mutual-swap → 100, else how many movers want the creep's *current*
//!   tile), `rate2` (being pulled), `rate3` (pulling), `rate4 = move_rate / weight`; losers stay.
//! - **Pull** (`canMove`'s `_pulled` branch + rate2/rate3): a creep dragged by an adjacent, moving
//!   puller follows into the puller's vacated tile and is eligible even with **no MOVE part / nonzero
//!   fatigue** — how no-MOVE / under-MOVE compositions stay mobile. See [`resolve_moves_with_pulls`].
//! - **Obstacle + chain-block** (`checkObstacleAtXY` line 16-39 + `removeFromMatrix` line 154-165):
//!   a mover is stripped if its destination is a wall or holds a creep that is NOT itself moving
//!   (engine `!objects[i._id]`, line 22); stripping a mover recursively strips any mover that wanted
//!   the stripped creep's now-unvacated current tile — so a blocked front stops the whole column
//!   (the cohesion mechanic).
//!
//! **Not modelled yet:** room-edge crossing (a step off the room is blocked here), roads (fatigue
//! stays plain/swamp). Tracked in `AGENTS.md`.

use crate::state::*;
use screeps::{Direction, Position, RoomCoordinate};
use std::collections::{HashMap, HashSet};

/// (dx, dy) for a direction. Screeps y increases downward, so `Top` is `-y`.
fn dir_delta(dir: Direction) -> (i32, i32) {
    match dir {
        Direction::Top => (0, -1),
        Direction::TopRight => (1, -1),
        Direction::Right => (1, 0),
        Direction::BottomRight => (1, 1),
        Direction::Bottom => (0, 1),
        Direction::BottomLeft => (-1, 1),
        Direction::Left => (-1, 0),
        Direction::TopLeft => (-1, -1),
    }
}

/// One step from `pos` in `dir`, or `None` if it would leave the room (edge crossing is a later slice).
pub fn step(pos: Position, dir: Direction) -> Option<Position> {
    let (dx, dy) = dir_delta(dir);
    let x = pos.x().u8() as i32 + dx;
    let y = pos.y().u8() as i32 + dy;
    if !(0..=49).contains(&x) || !(0..=49).contains(&y) {
        return None;
    }
    Some(Position::new(
        RoomCoordinate::new(x as u8).ok()?,
        RoomCoordinate::new(y as u8).ok()?,
        pos.room_name(),
    ))
}

/// Room-edge tile (fatigue resets to 0 on entering one, `movement.js:242`).
pub fn is_edge(x: u8, y: u8) -> bool {
    x == 0 || x == 49 || y == 0 || y == 49
}

struct Mover {
    id: CreepId,
    current: (u8, u8),
    dest: (u8, u8),
    dest_pos: Position,
    move_rate: u32,
    weight: u32,   // min 1
    pulled: bool, // being dragged (engine `_pulled`): eligible regardless of own MOVE/fatigue (rate2)
    pulling: bool, // dragging another (engine `_pull`, rate3)
}

fn xy(p: Position) -> (u8, u8) {
    (p.x().u8(), p.y().u8())
}

fn rate1(movers: &[Mover], want_count: &HashMap<(u8, u8), usize>, i: usize) -> u32 {
    let m = &movers[i];
    // Mutual swap: some mover is currently on m's destination and wants m's current tile.
    let swap = movers
        .iter()
        .any(|n| n.current == m.dest && n.dest == m.current);
    if swap {
        100
    } else {
        want_count.get(&m.current).copied().unwrap_or(0) as u32
    }
}

fn rate4(movers: &[Mover], i: usize) -> f64 {
    movers[i].move_rate as f64 / movers[i].weight as f64
}

/// Resolve move intents with no pulls (the common case). See [`resolve_moves_with_pulls`].
pub fn resolve_moves(
    world: &CombatWorld,
    moves: &HashMap<CreepId, Direction>,
) -> HashMap<CreepId, Position> {
    resolve_moves_with_pulls(world, moves, &HashMap::new())
}

/// Resolve move + pull intents for a tick (engine `movement.js check`). `pulls` maps a puller to the
/// creep it drags: a pulled creep follows the puller into its vacated tile and is eligible even with
/// **no MOVE part / nonzero fatigue** (engine `canMove`'s `_pulled` branch) — this is how no-MOVE /
/// under-MOVE combat compositions stay mobile. Returns movers → new position.
pub fn resolve_moves_with_pulls(
    world: &CombatWorld,
    moves: &HashMap<CreepId, Direction>,
    pulls: &HashMap<CreepId, CreepId>,
) -> HashMap<CreepId, Position> {
    let creep_by_id: HashMap<CreepId, &SimCreep> = world
        .creeps
        .iter()
        .filter(|c| c.is_alive())
        .map(|c| (c.id, c))
        .collect();

    // Valid pulls: puller + target alive, adjacent, puller has a move intent. `pulled_by` maps the
    // dragged creep → its puller and overrides the dragged creep's own move intent.
    let mut pulled_by: HashMap<CreepId, CreepId> = HashMap::new();
    for (&puller, &target) in pulls {
        if let (Some(p), Some(t)) = (creep_by_id.get(&puller), creep_by_id.get(&target)) {
            if moves.contains_key(&puller) && p.pos.get_range_to(t.pos) <= 1 {
                pulled_by.insert(target, puller);
            }
        }
    }
    let pullers: HashSet<CreepId> = pulled_by.values().copied().collect();

    let mut movers: Vec<Mover> = Vec::new();
    // Self-propelled movers: alive, eligible (fatigue 0 + MOVE part), not currently being pulled.
    for c in &world.creeps {
        if !c.is_alive() || pulled_by.contains_key(&c.id) {
            continue;
        }
        let dir = match moves.get(&c.id) {
            Some(&d) => d,
            None => continue,
        };
        if c.fatigue > 0 || !c.body.can_move() {
            continue;
        }
        let dest_pos = match step(c.pos, dir) {
            Some(d) => d,
            None => continue,
        };
        movers.push(Mover {
            id: c.id,
            current: xy(c.pos),
            dest: xy(dest_pos),
            dest_pos,
            move_rate: c.body.move_rate(),
            weight: c.body.fatigue_weight().max(1),
            pulled: false,
            pulling: pullers.contains(&c.id),
        });
    }
    // Pulled creeps follow their puller into its current tile (only if the puller is itself moving).
    let self_mover_ids: HashSet<CreepId> = movers.iter().map(|m| m.id).collect();
    for (&target, &puller) in &pulled_by {
        if !self_mover_ids.contains(&puller) {
            continue; // puller isn't moving → nothing to follow into
        }
        let t = creep_by_id[&target];
        let p = creep_by_id[&puller];
        movers.push(Mover {
            id: target,
            current: xy(t.pos),
            dest: xy(p.pos), // into the puller's vacated tile
            dest_pos: p.pos,
            move_rate: t.body.move_rate(),
            weight: t.body.fatigue_weight().max(1),
            pulled: true,
            pulling: false,
        });
    }
    if movers.is_empty() {
        return HashMap::new();
    }

    let mut want_count: HashMap<(u8, u8), usize> = HashMap::new();
    for m in &movers {
        *want_count.entry(m.dest).or_insert(0) += 1;
    }
    // dest tile -> contending mover indices.
    let mut matrix: HashMap<(u8, u8), Vec<usize>> = HashMap::new();
    for (i, m) in movers.iter().enumerate() {
        matrix.entry(m.dest).or_default().push(i);
    }
    // dest tile -> movers wanting it (for chain-block: a stayed creep blocks followers).
    let want_idx = matrix.clone();

    let mut moving = vec![true; movers.len()];

    // ── Contention: one winner per contested tile (rate1 then rate4); losers stay ────────────────
    for contenders in matrix.values() {
        if contenders.len() <= 1 {
            continue;
        }
        let mut best = contenders[0];
        for &i in contenders.iter().skip(1) {
            // Engine order: rate1 (swap/affected), rate2 (being pulled), rate3 (pulling), rate4.
            let key = |k: usize| {
                (
                    rate1(&movers, &want_count, k),
                    movers[k].pulled as u32,
                    movers[k].pulling as u32,
                )
            };
            let (a, b) = (key(i), key(best));
            let win = a > b || (a == b && rate4(&movers, i) > rate4(&movers, best));
            if win {
                best = i;
            }
        }
        for &i in contenders.iter() {
            if i != best {
                moving[i] = false;
            }
        }
    }

    // All living creeps by current tile (to detect non-moving blockers).
    let creep_at: HashMap<(u8, u8), CreepId> = world
        .creeps
        .iter()
        .filter(|c| c.is_alive())
        .map(|c| ((c.pos.x().u8(), c.pos.y().u8()), c.id))
        .collect();
    let mover_idx_of: HashMap<CreepId, usize> =
        movers.iter().enumerate().map(|(i, m)| (m.id, i)).collect();

    // ── Obstacle + chain-block (removeFromMatrix) ────────────────────────────────────────────────
    let mut stack: Vec<usize> = Vec::new();
    for (i, m) in movers.iter().enumerate() {
        if !moving[i] {
            stack.push(i); // a contention loser stays → may block followers
            continue;
        }
        let wall = world.terrain.is_wall(m.dest.0, m.dest.1);
        // Destination blocked iff a creep is there that won't vacate (non-mover, or a mover that
        // isn't moving). A mover at dest that IS moving vacates it → not a blocker.
        let occupied = match creep_at.get(&m.dest) {
            Some(cid) => match mover_idx_of.get(cid) {
                Some(&j) => !moving[j],
                None => true,
            },
            None => false,
        };
        if wall || occupied {
            moving[i] = false;
            stack.push(i);
        }
    }
    // Propagate: a creep that stays blocks every mover that wanted its current tile.
    while let Some(i) = stack.pop() {
        if let Some(followers) = want_idx.get(&movers[i].current) {
            for &j in followers {
                if moving[j] {
                    moving[j] = false;
                    stack.push(j);
                }
            }
        }
    }

    movers
        .iter()
        .enumerate()
        .filter(|(i, _)| moving[*i])
        .map(|(_, m)| (m.id, m.dest_pos))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::SimBody;
    use screeps::{Part, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(
            RoomCoordinate::new(x).unwrap(),
            RoomCoordinate::new(y).unwrap(),
            room,
        )
    }
    fn creep(id: CreepId, x: u8, y: u8, parts: &[(Part, u32)], fatigue: u32) -> SimCreep {
        let body: Vec<_> = parts
            .iter()
            .flat_map(|&(p, n)| std::iter::repeat_n(crate::body::BodyPartDef::new(p), n as usize))
            .collect();
        SimCreep {
            id,
            owner: 0,
            pos: pos(x, y),
            body: SimBody::new(body),
            fatigue,
        }
    }
    fn moves(pairs: &[(CreepId, Direction)]) -> HashMap<CreepId, Direction> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn simple_move_to_empty_tile() {
        let world = CombatWorld {
            creeps: vec![creep(1, 25, 25, &[(Part::Move, 1)], 0)],
            ..Default::default()
        };
        let r = resolve_moves(&world, &moves(&[(1, Direction::Right)]));
        assert_eq!(r.get(&1), Some(&pos(26, 25)));
    }

    #[test]
    fn wall_blocks_the_move() {
        let mut world = CombatWorld {
            creeps: vec![creep(1, 25, 25, &[(Part::Move, 1)], 0)],
            ..Default::default()
        };
        world.terrain.walls.insert((26, 25));
        assert!(resolve_moves(&world, &moves(&[(1, Direction::Right)])).is_empty());
    }

    #[test]
    fn fatigued_or_moveless_cannot_move() {
        let world = CombatWorld {
            creeps: vec![
                creep(1, 10, 10, &[(Part::Attack, 1), (Part::Move, 1)], 5), // fatigued
                creep(2, 20, 20, &[(Part::Attack, 1)], 0),                  // no MOVE part
            ],
            ..Default::default()
        };
        let r = resolve_moves(
            &world,
            &moves(&[(1, Direction::Right), (2, Direction::Right)]),
        );
        assert!(r.is_empty());
    }

    #[test]
    fn two_creeps_contest_one_tile_only_one_moves() {
        // Both want (25,25); higher move/weight ratio wins. C1 is all-MOVE (rate4 high), C2 carries
        // dead weight (lower rate4) → C1 wins, C2 stays.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 24, 25, &[(Part::Move, 2)], 0),
                creep(2, 26, 25, &[(Part::Attack, 4), (Part::Move, 1)], 0),
            ],
            ..Default::default()
        };
        let r = resolve_moves(
            &world,
            &moves(&[(1, Direction::Right), (2, Direction::Left)]),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r.get(&1), Some(&pos(25, 25)));
        assert!(!r.contains_key(&2));
    }

    #[test]
    fn adjacent_creeps_swap() {
        // C1 at (25,25)→(26,25); C2 at (26,25)→(25,25). Mutual swap: both move.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 25, 25, &[(Part::Move, 1)], 0),
                creep(2, 26, 25, &[(Part::Move, 1)], 0),
            ],
            ..Default::default()
        };
        let r = resolve_moves(
            &world,
            &moves(&[(1, Direction::Right), (2, Direction::Left)]),
        );
        assert_eq!(r.get(&1), Some(&pos(26, 25)));
        assert_eq!(r.get(&2), Some(&pos(25, 25)));
    }

    #[test]
    fn blocked_front_stops_the_column() {
        // A column: C1(24,25)→(25,25); C2(23,25)→(24,25). A wall at (25,25) blocks C1; C2 wanted
        // C1's tile, so the chain-block stops C2 too.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 24, 25, &[(Part::Move, 1)], 0),
                creep(2, 23, 25, &[(Part::Move, 1)], 0),
            ],
            ..Default::default()
        };
        world.terrain.walls.insert((25, 25));
        let r = resolve_moves(
            &world,
            &moves(&[(1, Direction::Right), (2, Direction::Right)]),
        );
        assert!(
            r.is_empty(),
            "blocked front must stop the follower (cohesion)"
        );
    }

    #[test]
    fn column_advances_when_front_is_clear() {
        // Same column, no wall: C1 moves into the empty tile, C2 follows into C1's vacated tile.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 24, 25, &[(Part::Move, 1)], 0),
                creep(2, 23, 25, &[(Part::Move, 1)], 0),
            ],
            ..Default::default()
        };
        let r = resolve_moves(
            &world,
            &moves(&[(1, Direction::Right), (2, Direction::Right)]),
        );
        assert_eq!(r.get(&1), Some(&pos(25, 25)));
        assert_eq!(r.get(&2), Some(&pos(24, 25)));
    }

    fn pulls(pairs: &[(CreepId, CreepId)]) -> HashMap<CreepId, CreepId> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn pull_drags_a_zero_move_creep() {
        // Puller (MOVE) at (25,25)→(26,25) drags a no-MOVE creep at (24,25) into its vacated tile.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 25, 25, &[(Part::Move, 1)], 0),
                creep(2, 24, 25, &[(Part::Attack, 5)], 0), // no MOVE part
            ],
            ..Default::default()
        };
        let r =
            resolve_moves_with_pulls(&world, &moves(&[(1, Direction::Right)]), &pulls(&[(1, 2)]));
        assert_eq!(r.get(&1), Some(&pos(26, 25)), "puller advances");
        assert_eq!(
            r.get(&2),
            Some(&pos(25, 25)),
            "pulled creep follows into vacated tile"
        );
    }

    #[test]
    fn zero_move_creep_cannot_move_unpulled() {
        // Same no-MOVE creep, given a direct move intent but NOT pulled → it cannot move.
        let world = CombatWorld {
            creeps: vec![creep(2, 24, 25, &[(Part::Attack, 5)], 0)],
            ..Default::default()
        };
        let r = resolve_moves(&world, &moves(&[(2, Direction::Right)]));
        assert!(r.is_empty(), "a no-MOVE creep is immobile without a pull");
    }

    #[test]
    fn pulled_creep_moves_despite_fatigue() {
        // The dragged creep has MOVE parts but nonzero fatigue (would normally be stuck). The
        // engine `_pulled` branch bypasses both fatigue and the MOVE requirement → it still follows.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 25, 25, &[(Part::Move, 1)], 0),
                creep(2, 24, 25, &[(Part::Move, 1), (Part::Attack, 4)], 8), // fatigued
            ],
            ..Default::default()
        };
        let r =
            resolve_moves_with_pulls(&world, &moves(&[(1, Direction::Right)]), &pulls(&[(1, 2)]));
        assert_eq!(r.get(&1), Some(&pos(26, 25)));
        assert_eq!(
            r.get(&2),
            Some(&pos(25, 25)),
            "fatigue does not stop a pulled creep"
        );
    }

    #[test]
    fn pull_does_nothing_when_puller_is_blocked() {
        // Puller blocked by a wall → it stays, so the pulled creep has nothing to follow into.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 25, 25, &[(Part::Move, 1)], 0),
                creep(2, 24, 25, &[(Part::Attack, 5)], 0),
            ],
            ..Default::default()
        };
        world.terrain.walls.insert((26, 25));
        let r =
            resolve_moves_with_pulls(&world, &moves(&[(1, Direction::Right)]), &pulls(&[(1, 2)]));
        assert!(r.is_empty(), "a blocked puller drags no one");
    }
}
