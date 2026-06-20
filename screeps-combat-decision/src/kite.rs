//! Pure per-tile pricing for squad kite/flee positioning (P2.G3-tail, ADR 0008 §4.1 / §5).
//!
//! This is the **combat pricing** the pathfinding system consumes — NOT a search. The search is
//! `screeps-rover`'s pure `LocalPathfinder` (`search_scored`); the squad runs **one** bounded
//! search pricing each reached tile with [`score_tile`], so "stay cohesive" and "find a safe,
//! higher-expected-value position" are **one** scoring function (the cohesion term lives in the
//! score, not a separate per-creep clamp — the operator's "don't be disorganized with squads").
//!
//! `score_tile` returns a **cost (lower = better)** so it drops straight into the min-scored search
//! with no sign juggling. Pure over `screeps::Position` — no `game::*`, no serialization. The weights
//! are tunable params (the ADR 0008a EXP-* loop tunes them on the sim).

use screeps::local::{LocalCostMatrix, RoomXY};
use screeps::{Position, RoomName};
use screeps_rover::{LocalPathfinder, ReachSource, ReachabilityMap};

/// Op budget for the per-squad kite search — a local ~window flood, not a full-room path. Bounded
/// so it costs ~one cheap search per squad per kiting tick (and degrades gracefully on exhaustion).
pub const MAX_KITE_OPS: u32 = 400;

/// How a hostile threatens a tile — drives the safety "danger reach".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreatKind {
    /// Melee-only (must be kept beyond range 1; we keep beyond `reach` to be safe).
    MeleeOnly,
    /// Has ranged attack (dangerous out to ~range 3).
    Ranged,
}

/// A hostile as the kite scorer sees it.
#[derive(Clone, Copy, Debug)]
pub struct KiteThreat {
    pub pos: Position,
    pub kind: ThreatKind,
    /// Danger reach (tiles): stand beyond this. Melee kiters keep ~3 from a melee-only chaser;
    /// ranged threats reach ~3.
    pub reach: u32,
    /// Fatigue cadence: ticks this threat spends to step one tile on plain terrain (`>=1`; lower =
    /// faster). `None` ⇒ immobile (no working MOVE) → not a chaser, so it seeds no reachability wave
    /// (ADR 0019 Guard 5), though it still contributes present danger via `reach`.
    pub step_ticks: Option<u32>,
}

/// A hostile tower (its damage falls off with range — see the engine's
/// `tower_attack_damage_at_range`, the single source of truth for the tower curve).
#[derive(Clone, Copy, Debug)]
pub struct KiteTower {
    pub pos: Position,
}

/// Tunable weights for [`score_tile`]. Default ordering: **SAFETY ≫ COHESION > VALUE > openness**
/// (the operator priority — don't die, then stay with the squad, then keep the focus shootable).
#[derive(Clone, Copy, Debug)]
pub struct KiteScoreParams {
    pub w_safety: f32,
    pub w_cohesion: f32,
    pub w_value: f32,
    pub w_openness: f32,
    /// Weight of the **future-threat** term (ADR 0019 Stage 2): penalize tiles a mobile chaser will
    /// reach SOON (low ticks-to-reach), so the kiter favors positions with durable standoff instead
    /// of ones safe only this tick. Scaled per-tile by `max(0, FUTURE_HORIZON - ticks_to_reach)`.
    pub w_future: f32,
    /// Cohesion radius K: beyond this distance from the centroid the penalty steepens (×3/tile).
    pub max_cohesion_radius: u32,
}

impl Default for KiteScoreParams {
    fn default() -> Self {
        Self {
            w_safety: 1000.0,
            w_cohesion: 10.0,
            w_value: 3.0,
            w_openness: 1.0,
            // Below cohesion (don't scatter) but above value/openness: durable standoff matters more
            // than keeping the focus shootable, less than staying with the squad. Tuned by EXP-*.
            w_future: 5.0,
            max_cohesion_radius: 2,
        }
    }
}

/// How many ticks ahead the future-threat term looks: a tile a chaser reaches in `>= FUTURE_HORIZON`
/// ticks is "far enough" and gets no future penalty; nearer arrivals scale up linearly.
pub const FUTURE_HORIZON: u32 = 5;

/// The squad's kite-scoring context — its centroid (cohesion anchor), the threats/towers to avoid,
/// and the shared focus the value term keeps shootable.
pub struct SquadKiteView<'a> {
    pub centroid: Position,
    pub threats: &'a [KiteThreat],
    pub towers: &'a [KiteTower],
    /// Shared focus position; the value term keeps it within shooting range 3.
    pub focus: Option<Position>,
    pub params: KiteScoreParams,
}

/// Cost of the squad standing its block on `tile` — **LOWER is better** (composes directly with
/// rover's min-scored search). Sums five weighted penalties:
/// - **SAFETY:** inside a threat's `reach` (worse the deeper) + tower DPS at this range;
/// - **FUTURE:** how soon a mobile chaser reaches this tile (`reach` optional map; ADR 0019 Stage 2);
/// - **COHESION:** Chebyshev distance from the centroid, steepening ×3/tile past `max_cohesion_radius`;
/// - **VALUE:** the focus being beyond shooting range 3;
/// - **OPENNESS:** fewer walkable neighbours (0–8, supplied from the cost matrix) → nearer a dead-end.
///
/// `reach` is the shared per-room reachability map (built once by [`plan_kite_anchor`]); `None` ⇒ the
/// future term is omitted (byte-identical to the pre-Stage-2 four-term score).
pub fn score_tile(view: &SquadKiteView, tile: Position, walkable_neighbors: u8, reach: Option<&ReachabilityMap>) -> i64 {
    let p = &view.params;

    // SAFETY — being within a threat's danger reach (deeper = worse), plus tower DPS here.
    let mut safety = 0.0f32;
    for t in view.threats {
        let r = tile.get_range_to(t.pos);
        if r <= t.reach {
            safety += (t.reach + 1 - r) as f32;
        }
    }
    for tw in view.towers {
        let r = tile.get_range_to(tw.pos);
        // Normalize to ~1.0 (min tower hit) .. 4.0 (point-blank). Tower curve delegated to the engine
        // (the single source of truth; the local duplicate was deleted in ADR 0019 Stage 1 after a
        // bit-identity proof over all in-room ranges).
        safety += screeps_combat_engine::damage::tower_attack_damage_at_range(r) as f32 / 150.0;
    }

    // FUTURE — a tile a mobile chaser reaches soon offers no durable standoff; penalize low
    // ticks-to-reach (linearly within FUTURE_HORIZON). Avoids "safe this tick, caught the next".
    let future = match reach {
        Some(map) => {
            let ttr = map.ticks_xy(tile.x().u8(), tile.y().u8());
            if ttr < FUTURE_HORIZON {
                (FUTURE_HORIZON - ttr) as f32
            } else {
                0.0
            }
        }
        None => 0.0,
    };

    // COHESION — distance from the centroid, steepening past K so the block doesn't string out.
    let d = tile.get_range_to(view.centroid);
    let cohesion = if d <= p.max_cohesion_radius {
        d as f32
    } else {
        p.max_cohesion_radius as f32 + (d - p.max_cohesion_radius) as f32 * 3.0
    };

    // VALUE — keep the focus within shooting range 3.
    let value = match view.focus {
        Some(f) => {
            let r = tile.get_range_to(f);
            if r <= 3 {
                0.0
            } else {
                (r - 3) as f32
            }
        }
        None => 0.0,
    };

    // OPENNESS — penalize dead-ends (few walkable neighbours).
    let openness = (8 - walkable_neighbors.min(8)) as f32;

    (p.w_safety * safety + p.w_future * future + p.w_cohesion * cohesion + p.w_value * value + p.w_openness * openness)
        .round() as i64
}

/// A planned kite/flee goal for the whole squad — the single tile every in-cohesion member targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KitePlan {
    pub goal: Position,
}

/// Count the walkable (not-impassable) neighbours of `tile` in the cost matrix (0–8) — the openness
/// input to [`score_tile`] (fewer exits ⇒ nearer a dead-end).
fn walkable_neighbors(cm: &LocalCostMatrix, tile: Position) -> u8 {
    let (x, y) = (tile.x().u8() as i32, tile.y().u8() as i32);
    let mut n = 0u8;
    for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
        let (nx, ny) = (x + dx, y + dy);
        if (0..50).contains(&nx) && (0..50).contains(&ny) {
            if let Ok(xy) = RoomXY::checked_new(nx as u8, ny as u8) {
                if cm.get(xy) != u8::MAX {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Plan ONE kite/flee goal for the whole squad: a single bounded `LocalPathfinder::search_scored`
/// from the centroid, pricing each reached tile with [`score_tile`] (safety + future-threat +
/// cohesion + value + openness). `None` ⇒ holding the centroid is already optimal (members
/// hold/shoot). This is the squad's ONE bounded search per kiting tick (members reuse the goal via
/// their own move request) — combat supplies the pricing, rover owns the search (no one-off rule).
///
/// First builds the shared per-room **reachability map** (ADR 0019 Stage 2) — one multi-source flood
/// seeded by the mobile chasers (`step_ticks.is_some()`) — so the future-threat term can prefer tiles
/// with durable standoff. Both the flood and the goal search run over the *same* matrix, live and sim.
///
/// `room_callback` supplies the room's movement cost matrix (terrain walls baked in): the **same**
/// `LocalPathfinder` runs live and in the sim — only the matrix source differs (the live
/// `CostMatrixSystem` vs the sim's synthetic). Kiting is single-room by nature (cross-room travel is
/// the separate `MoveToRoom` phase), so the headless local search is the right tool for both.
pub fn plan_kite_anchor(
    view: &SquadKiteView,
    room_callback: &mut dyn FnMut(RoomName) -> Option<LocalCostMatrix>,
    max_ops: u32,
) -> Option<KitePlan> {
    let room = view.centroid.room_name();
    let matrix = room_callback(room)?;
    // Reachability seeds = the mobile chasers only (ADR 0019 Guard 5: an immobile threat seeds no
    // wave). Built once, shared across every priced tile in the search below.
    let sources: Vec<ReachSource> = view
        .threats
        .iter()
        .filter_map(|t| t.step_ticks.map(|st| ReachSource { pos: t.pos, step_ticks: st }))
        .collect();
    let mut reach_cb = |_r: RoomName| Some(matrix.clone());
    let reach = (!sources.is_empty()).then(|| LocalPathfinder.reachability_from(&sources, room, &mut reach_cb, max_ops));
    let cost = |tile: Position| -> i64 { score_tile(view, tile, walkable_neighbors(&matrix, tile), reach.as_ref()) };
    // Feed the search the already-fetched matrix (so the openness lookup + the search agree).
    let mut cb = |_r: RoomName| Some(matrix.clone());
    let result = LocalPathfinder.search_scored(view.centroid, &mut cb, max_ops, 1, &cost);
    result.path.last().copied().map(|goal| KitePlan { goal })
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{RoomCoordinate, RoomName};

    /// ADR 0019 Stage 1 — kiting now delegates the tower curve to the engine (the local duplicate was
    /// deleted after a bit-identity proof over all in-room ranges 0..=49). This pins the values the
    /// kite safety term depends on, so a future engine-curve change that would shift kiting is caught.
    #[test]
    fn kite_uses_the_engine_tower_curve() {
        use screeps_combat_engine::damage::tower_attack_damage_at_range as t;
        assert_eq!(t(0), 600, "point-blank = optimal");
        assert_eq!(t(5), 600, "optimal range edge");
        assert_eq!(t(10), 450, "mid falloff");
        assert_eq!(t(20), 150, "min-damage floor");
        assert_eq!(t(49), 150, "beyond falloff stays at the floor");
    }

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }
    fn view<'a>(centroid: Position, threats: &'a [KiteThreat], towers: &'a [KiteTower], focus: Option<Position>) -> SquadKiteView<'a> {
        SquadKiteView { centroid, threats, towers, focus, params: KiteScoreParams::default() }
    }
    fn melee(x: u8, y: u8, reach: u32) -> KiteThreat {
        KiteThreat { pos: pos(x, y), kind: ThreatKind::MeleeOnly, reach, step_ticks: Some(1) }
    }


    #[test]
    fn safety_penalizes_being_inside_a_threats_reach() {
        let c = pos(25, 25);
        let threats = [melee(25, 25, 3)];
        let v = view(c, &threats, &[], None);
        // Outside reach (range 4) vs inside (range 1): inside is worse.
        let outside = score_tile(&v, pos(29, 25), 8, None);
        let inside = score_tile(&v, pos(26, 25), 8, None);
        assert!(inside > outside, "inside reach must cost more: in={inside} out={outside}");
    }

    #[test]
    fn cohesion_penalizes_distance_and_steepens_past_k() {
        let c = pos(25, 25);
        let v = view(c, &[], &[], None);
        let d1 = score_tile(&v, pos(26, 25), 8, None); // d=1 (<=K)
        let d2 = score_tile(&v, pos(27, 25), 8, None); // d=2 (==K)
        let d3 = score_tile(&v, pos(28, 25), 8, None); // d=3 (>K, steepened)
        let d4 = score_tile(&v, pos(29, 25), 8, None); // d=4 (>K)
        assert!(d1 < d2 && d2 < d3 && d3 < d4, "monotonic in distance");
        // The marginal cost past K (d2→d3) exceeds the marginal cost within K (d1→d2).
        assert!((d3 - d2) > (d2 - d1), "cohesion penalty steepens past K");
    }

    #[test]
    fn value_keeps_the_focus_in_shooting_range() {
        let c = pos(25, 25);
        let focus = pos(25, 30);
        let v = view(c, &[], &[], Some(focus));
        // Both equidistant from the centroid (d=3); A keeps the focus in range 3, B does not.
        let in_range = score_tile(&v, pos(25, 28), 8, None); // range to focus = 2
        let out_range = score_tile(&v, pos(25, 22), 8, None); // range to focus = 8
        assert!(in_range < out_range, "keeping the focus shootable is preferred: in={in_range} out={out_range}");
    }

    #[test]
    fn openness_avoids_dead_ends() {
        let c = pos(25, 25);
        let v = view(c, &[], &[], None);
        let open = score_tile(&v, pos(26, 25), 8, None); // 8 walkable neighbours
        let pocket = score_tile(&v, pos(26, 25), 1, None); // 1 walkable neighbour (dead-end)
        assert!(pocket > open, "a dead-end pocket costs more");
    }

    #[test]
    fn safety_dominates_cohesion() {
        let c = pos(25, 25);
        let threats = [melee(25, 25, 3)];
        let v = view(c, &threats, &[], None);
        // A: safe (range 4 from the threat) but far from the centroid (d=5).
        let safe_far = score_tile(&v, pos(30, 25), 8, None);
        // B: in the threat's reach (range 1) but right next to the centroid (d=1).
        let danger_close = score_tile(&v, pos(26, 25), 8, None);
        assert!(safe_far < danger_close, "safety outweighs cohesion: safe_far={safe_far} danger_close={danger_close}");
    }

    // ── future-threat term (ADR 0019 Stage 2 reachability folding) ──
    #[test]
    fn future_term_prefers_tiles_a_chaser_reaches_later() {
        // Two tiles equidistant from the centroid and outside any present reach, but one is closer in
        // TICKS to a fast chaser than the other. The future term makes the soon-reached tile cost more
        // (durable standoff is preferred), with no present-threat/tower/focus terms to confound it.
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let chaser = pos(20, 25);
        let room: RoomName = "W1N1".parse().unwrap();
        let reach = LocalPathfinder.reachability_from(&[ReachSource { pos: chaser, step_ticks: 1 }], room, &mut cb, 2000);
        let c = pos(25, 25);
        let v = view(c, &[], &[], None);
        let near_in_time = score_tile(&v, pos(24, 25), 8, Some(&reach)); // 4 tiles → ttr 4 (< horizon)
        let far_in_time = score_tile(&v, pos(26, 25), 8, Some(&reach)); // 6 tiles → ttr 6 (>= horizon)
        assert!(
            near_in_time > far_in_time,
            "the tile the chaser reaches sooner costs more: near={near_in_time} far={far_in_time}"
        );
        // With no reachability map the two are equal (future term omitted → byte-identical to pre-Stage-2).
        assert_eq!(score_tile(&v, pos(24, 25), 8, None), score_tile(&v, pos(26, 25), 8, None));
    }

    // ── plan_kite_anchor (one bounded search per squad) ─────────────────
    #[test]
    fn plan_kite_anchor_flees_to_a_safe_tile_near_the_centroid() {
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let centroid = pos(25, 25);
        // A melee threat adjacent to the centroid → holding is unsafe; kite out of reach.
        let threats = [melee(24, 25, 3)];
        let v = view(centroid, &threats, &[], None);
        let plan = plan_kite_anchor(&v, &mut cb, MAX_KITE_OPS).expect("a safer tile than the centroid exists");
        assert!(plan.goal.get_range_to(pos(24, 25)) > 3, "escapes the melee reach: {:?}", plan.goal);
        assert!(plan.goal.get_range_to(centroid) <= 6, "stays near the squad (cohesion in the score): {:?}", plan.goal);
    }

    #[test]
    fn plan_kite_anchor_holds_when_already_safe() {
        let mut cb = |_r| Some(LocalCostMatrix::new());
        // No threats/towers/focus → the centroid (cohesion 0) is the global min → hold (no goal).
        let v = view(pos(25, 25), &[], &[], None);
        assert!(plan_kite_anchor(&v, &mut cb, MAX_KITE_OPS).is_none(), "nothing to flee → hold");
    }
}
