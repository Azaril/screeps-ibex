//! Single-room weighted grid search, owned by the pathfinding system.
//!
//! Engine `PathFinder` cost matrices are u8 (1..=254, 255 = impassable):
//! they cannot express searches whose tile weights span orders of magnitude
//! — e.g. breach planning, where entering a tile costs its blockers' hit
//! pool (hundreds to millions). Searches of that class are implemented HERE
//! and consumed by feature modules as pricing policy — never as bespoke
//! algorithms in feature code (operator directive 2026-06-13; EP-2.6 one
//! implementation per concern). Engine-walkable navigation stays on
//! `PathfinderService` and the rover movement system.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

pub const ROOM_DIM: usize = 50;

/// Min-cost path across one room from `start` to within `goal_range`
/// (Chebyshev) of `goal`, moving in 8 directions. `enter_cost` prices
/// stepping ONTO a tile (`None` = impassable); the `start` tile itself is
/// never priced. Returns the path tiles in walk order, EXCLUDING `start`
/// (empty when `start` already satisfies the goal), or `None` when no route
/// exists.
///
/// Bounded at ROOM_DIM² nodes with no JS calls — cheap enough to run
/// uncharged, but callers doing repeated searches should cache results
/// (see `BreachPlanCache`). Deterministic per EP-6.13: the heap orders by
/// (cost, tile index), so equal-cost routes always resolve the same way.
pub fn room_grid_dijkstra(
    enter_cost: &dyn Fn(u8, u8) -> Option<u64>,
    start: (u8, u8),
    goal: (u8, u8),
    goal_range: u8,
) -> Option<Vec<(u8, u8)>> {
    let index = |x: u8, y: u8| y as usize * ROOM_DIM + x as usize;
    let coords = |i: usize| ((i % ROOM_DIM) as u8, (i / ROOM_DIM) as u8);
    let satisfies_goal = |x: u8, y: u8| x.abs_diff(goal.0) <= goal_range && y.abs_diff(goal.1) <= goal_range;

    if satisfies_goal(start.0, start.1) {
        return Some(Vec::new());
    }

    let mut dist = vec![u64::MAX; ROOM_DIM * ROOM_DIM];
    let mut prev = vec![usize::MAX; ROOM_DIM * ROOM_DIM];
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();

    let start_index = index(start.0, start.1);
    dist[start_index] = 0;
    heap.push(Reverse((0, start_index)));

    let mut found: Option<usize> = None;

    while let Some(Reverse((cost, node))) = heap.pop() {
        if cost > dist[node] {
            continue;
        }

        let (x, y) = coords(node);

        if satisfies_goal(x, y) {
            found = Some(node);
            break;
        }

        for dx in -1i32..=1 {
            for dy in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }

                let nx = x as i32 + dx;
                let ny = y as i32 + dy;

                if !(0..ROOM_DIM as i32).contains(&nx) || !(0..ROOM_DIM as i32).contains(&ny) {
                    continue;
                }

                let (nx, ny) = (nx as u8, ny as u8);

                let Some(step) = enter_cost(nx, ny) else {
                    continue;
                };

                let neighbor = index(nx, ny);
                let next_cost = cost.saturating_add(step);

                if next_cost < dist[neighbor] {
                    dist[neighbor] = next_cost;
                    prev[neighbor] = node;
                    heap.push(Reverse((next_cost, neighbor)));
                }
            }
        }
    }

    let mut node = found?;
    let mut path = Vec::new();

    while node != start_index {
        path.push(coords(node));
        node = prev[node];
    }

    path.reverse();

    Some(path)
}

/// True if a creep can walk from any room-edge tile to within range 1 of
/// `start`, over `passable` tiles in 8 directions (a flood fill). `start`
/// itself is NOT required passable — it may be a structure tile such as a
/// controller — the flood seeds from its passable neighbours. Used for
/// reachability gating (e.g. "is the controller reachable without
/// dismantling, given current structures?"). Bounded at ROOM_DIM² nodes, no
/// JS calls.
pub fn reaches_room_edge(passable: &dyn Fn(u8, u8) -> bool, start: (u8, u8)) -> bool {
    let index = |x: u8, y: u8| y as usize * ROOM_DIM + x as usize;
    let is_edge = |x: u8, y: u8| x == 0 || y == 0 || x as usize == ROOM_DIM - 1 || y as usize == ROOM_DIM - 1;

    let mut visited = vec![false; ROOM_DIM * ROOM_DIM];
    let mut queue: VecDeque<(u8, u8)> = VecDeque::new();

    let push_passable_neighbours = |x: u8, y: u8, visited: &mut Vec<bool>, queue: &mut VecDeque<(u8, u8)>| {
        for dx in -1i32..=1 {
            for dy in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if !(0..ROOM_DIM as i32).contains(&nx) || !(0..ROOM_DIM as i32).contains(&ny) {
                    continue;
                }
                let (nx, ny) = (nx as u8, ny as u8);
                if !visited[index(nx, ny)] && passable(nx, ny) {
                    visited[index(nx, ny)] = true;
                    queue.push_back((nx, ny));
                }
            }
        }
    };

    // Seed from the start's passable neighbours (the start tile itself is not
    // walkable — e.g. the controller sits on it).
    push_passable_neighbours(start.0, start.1, &mut visited, &mut queue);

    while let Some((x, y)) = queue.pop_front() {
        if is_edge(x, y) {
            return true;
        }
        push_passable_neighbours(x, y, &mut visited, &mut queue);
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: fn(u8, u8) -> Option<u64> = |_, _| Some(1);

    #[test]
    fn open_grid_path_length_is_chebyshev_distance() {
        // 8-directional movement: the optimal step count between two points
        // is their Chebyshev distance.
        let path = room_grid_dijkstra(&OPEN, (5, 25), (45, 20), 1).expect("path should exist");
        assert_eq!(path.len() as u32, 40 - 1); // distance 40, goal_range 1
    }

    #[test]
    fn start_inside_goal_range_returns_empty_path() {
        assert_eq!(room_grid_dijkstra(&OPEN, (10, 10), (11, 11), 1), Some(Vec::new()));
        assert_eq!(room_grid_dijkstra(&OPEN, (10, 10), (10, 13), 3), Some(Vec::new()));
    }

    #[test]
    fn sealed_goal_returns_none() {
        // Goal ringed by impassable tiles at range 2; goal_range 1 targets
        // sit inside the ring.
        let cost = |x: u8, y: u8| -> Option<u64> {
            if x.abs_diff(25) == 2 && y.abs_diff(25) <= 2 || y.abs_diff(25) == 2 && x.abs_diff(25) <= 2 {
                None
            } else {
                Some(1)
            }
        };
        assert_eq!(room_grid_dijkstra(&cost, (5, 25), (25, 25), 1), None);
    }

    #[test]
    fn expensive_tiles_are_detoured_when_cheaper_routes_exist() {
        // A high-cost band across column 25 except one cheap tile at y=5:
        // the path must route through (25, 5) despite the longer walk.
        let cost = |x: u8, y: u8| -> Option<u64> {
            if x == 25 {
                if y == 5 {
                    Some(10)
                } else {
                    Some(1_000_000)
                }
            } else {
                Some(1)
            }
        };

        let path = room_grid_dijkstra(&cost, (5, 25), (45, 25), 1).expect("path should exist");
        assert!(path.contains(&(25, 5)), "path must use the cheap crossing: {:?}", path);
        assert_eq!(path.iter().filter(|(x, _)| *x == 25).count(), 1, "exactly one crossing of the band");
    }

    #[test]
    fn open_room_controller_reaches_edge() {
        // Interior start, everything passable → reachable.
        assert!(reaches_room_edge(&|_, _| true, (25, 25)));
    }

    #[test]
    fn fully_walled_controller_does_not_reach_edge() {
        // Ring of impassable tiles around the controller at range 1: its
        // passable neighbours are all blocked, so nothing is even seeded.
        let blocked: std::collections::HashSet<(u8, u8)> = [(24, 24), (25, 24), (26, 24), (24, 25), (26, 25), (24, 26), (25, 26), (26, 26)]
            .into_iter()
            .collect();
        let passable = |x: u8, y: u8| !blocked.contains(&(x, y));
        assert!(!reaches_room_edge(&passable, (25, 25)));
    }

    #[test]
    fn single_gap_in_wall_reaches_edge() {
        // Box around the controller with one gap at (25,24): the flood escapes
        // through it to the edge.
        let blocked: std::collections::HashSet<(u8, u8)> = [(24, 24), (26, 24), (24, 25), (26, 25), (24, 26), (25, 26), (26, 26)]
            .into_iter()
            .collect();
        // (25,24) deliberately left open.
        let passable = |x: u8, y: u8| !blocked.contains(&(x, y));
        assert!(reaches_room_edge(&passable, (25, 25)));
    }

    #[test]
    fn controller_on_edge_reaches_edge() {
        // A controller adjacent to the room border is trivially reachable: a
        // passable neighbour lies on the edge.
        assert!(reaches_room_edge(&|_, _| true, (1, 0)));
    }
}
