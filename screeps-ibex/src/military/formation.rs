use super::squad::*;
use screeps_combat_decision::composition::*;
use screeps::*;
use screeps_rover::*;
use specs::Entity;

// ADR 0031 D14: the dead hardcoded-2×2 cost overlay (`is_valid_quad_position`, `apply_quad_cost_overlay`)
// and its unused generalizations (`apply_formation_cost_overlay`, `apply_tower_avoidance_costs`) were
// removed here — the LIVE footprint overlay is rover `moving_maximum(cm, w, h)`, fed the COMPACT
// `box_footprint(N)` derived from the member count (see `advance_virtual_pos`).

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
/// A reachable STAND-OFF tile one step from a target structure toward the squad. A structure focus sits on
/// an IMPASSABLE tile, so driving the formation anchor onto it pathfinds to range 0, finds no path, and
/// reports `Blocked` — the squad then holds SHORT of weapon range and never fires (the invader-core "enters
/// the room but does nothing" bug, ADR 0026 §9). Standing off one tile toward the squad keeps a ranged (≤3)
/// or siege (1) formation in weapon range. (For a fully ramparted target this tile may itself be blocked;
/// breaching is the separate siege path — this fixes the open-target case, e.g. a level-0 invader core.)
pub fn standoff_one_tile(structure: Position, toward: Position) -> Position {
    let (sx, sy) = (structure.x().u8() as i32, structure.y().u8() as i32);
    let (tx, ty) = (toward.x().u8() as i32, toward.y().u8() as i32);
    let mut dx = (tx - sx).signum();
    let dy = (ty - sy).signum();
    if dx == 0 && dy == 0 {
        dx = 1; // degenerate (squad centroid on the structure tile) — pick an arbitrary neighbour
    }
    let nx = (sx + dx).clamp(1, (ROOM_SIZE - 2) as i32) as u8;
    let ny = (sy + dy).clamp(1, (ROOM_SIZE - 2) as i32) as u8;
    Position::new(
        RoomCoordinate::new(nx).expect("1..=48 is a valid room coordinate"),
        RoomCoordinate::new(ny).expect("1..=48 is a valid room coordinate"),
        structure.room_name(),
    )
}

/// Whether the squad's full roster has spawned AND every member has a body in the world — i.e. it is
/// `squad_ready_to_depart` (rally gate) + `should_hold_at_boundary` (boundary cohesion) — the pure P-OBJ
/// #23 gates, lifted to the shared `screeps_combat_decision::rally` kernel (K0 / ADR 0028) so the bot and
/// the offline lifecycle harness share ONE implementation. Re-exported here so existing call sites
/// (`squad_manager`, `advance_squad_virtual_position`) are unchanged.
pub use screeps_combat_decision::rally::{ready_to_depart_gate, should_hold_at_boundary, target_is_uncontested};

pub fn advance_squad_virtual_position(squad: &mut SquadContext, destination: Position) {
    // P-OBJ #23 invader no-engage ROOT CAUSE: count ONLY members with a resolved position. A still-
    // spawning member carries `position: None` (no body in the world yet) for the whole ~body*3-tick
    // spawn; including it inflated `living_count` AND failed every cohesion quorum, so `boundary_hold`
    // latched true and a lone in-room lead was frozen at the room edge — the squad never massed, the room
    // never became visible, the DTOs stayed empty, and `decide_squad` never found a focus to engage.
    let living_members: Vec<(usize, Option<Position>)> =
        squad.members.iter().filter(|m| m.position.is_some()).map(|m| (m.formation_slot, m.position)).collect();

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
    // Room-boundary cohesion gate (extracted to the pure `should_hold_at_boundary` for offline tests +
    // the P-OBJ #23 spawning-member fix). `living_members` is already positioned-only (filtered above), so
    // these are all `Some`; the helper filters again defensively.
    let member_positions: Vec<Option<Position>> = living_members.iter().map(|(_, pos)| *pos).collect();
    let boundary_hold = should_hold_at_boundary(&member_positions, virtual_pos, destination);

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
    // The cohesive footprint we WANT to route as. For a ≥3-member blob this is the COMPACT box that
    // holds all members (`box_footprint`, ADR 0031 D14 — N=4→2×2, 5-6→3×2, 7-8→3×3) even when the member
    // layout is temporarily collapsed to a line for a corridor; for a duo/solo it is just the current
    // layout's footprint. Probing with the box footprint every tick — derived from the member COUNT, not
    // the live (possibly-collapsed) layout — gives a stable "does the box fit here?" signal, so the
    // collapse/re-form below cannot oscillate. The rover overlay + single-file fallback handle any W×H.
    let member_count = squad.members.len();
    let tight_footprint = if member_count >= 3 {
        box_footprint(member_count)
    } else {
        squad_footprint(squad)
    };

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

/// Decide the member layout for a box blob given whether its tight (compact-box) footprint is currently
/// blocked. Returns `Some(new_layout)` when the layout should change this tick, or `None` to keep
/// the current one.
///
/// - **Collapse**: a `Box2x2` whose footprint is blocked drops to a single-file `Line` so members
///   thread the corridor.
/// - **Re-form**: a `Line` whose box footprint fits again snaps back to a compact box ([`box_formation`])
///   — the corridor → cohesive transition, taken the instant a group path exists.
///
/// Scoped to box blobs (≥3 members, ADR 0031 D14 — `formation_for` emits `Box2x2` for any count ≥3): every
/// intended `Line` composition is a 2-member duo, so a ≥3-member `Line` is always a collapsed blob and
/// re-forming it can never clobber an intended line. The offsets are count-driven (`line`/`box_formation`
/// take the member count), so this generalizes to 5-8 members with no per-shape table. Pure (no `game::*`)
/// so the transition is unit-testable without the game runtime.
fn corridor_layout_transition(shape: Option<FormationShape>, member_count: usize, tight_blocked: bool) -> Option<FormationLayout> {
    if member_count < 3 {
        return None;
    }
    match shape {
        Some(FormationShape::Box2x2) if tight_blocked => Some(FormationLayout::line(member_count)),
        Some(FormationShape::Line) if !tight_blocked => Some(FormationLayout::box_formation(member_count)),
        _ => None,
    }
}

const ROOM_SIZE: u8 = 50;

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
        assert_eq!(reformed.offsets, vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
    }

    /// ADR 0031 D14: the corridor collapse/re-form generalizes to any box blob (5-8 members), with
    /// count-driven offsets — a stuck N-box collapses to an N-long line and re-forms to the compact
    /// `box_formation(N)` (e.g. N=6 → 6 distinct offsets) the moment it fits again.
    #[test]
    fn box_blob_collapses_and_re_forms_for_5_to_8_members() {
        for n in [3usize, 5, 6, 8] {
            let collapsed = corridor_layout_transition(Some(FormationShape::Box2x2), n, true).expect("box collapses");
            assert_eq!(collapsed.shape, FormationShape::Line);
            assert_eq!(collapsed.offsets.len(), n, "line is N-long (n={n})");

            let reformed = corridor_layout_transition(Some(FormationShape::Line), n, false).expect("line re-forms");
            assert_eq!(reformed.shape, FormationShape::Box2x2);
            assert_eq!(reformed.offsets.len(), n, "box holds all N members (n={n})");
            assert_eq!(reformed.offsets.iter().collect::<std::collections::HashSet<_>>().len(), n, "distinct tiles (n={n})");
        }
    }

    /// Duos (and any <3-member squad) are never touched: an intended 2-member `Line` must not be
    /// "re-formed" into a box, and a 2-member squad has no box to collapse.
    #[test]
    fn duos_are_left_alone() {
        assert!(corridor_layout_transition(Some(FormationShape::Line), 2, false).is_none());
        assert!(corridor_layout_transition(Some(FormationShape::Line), 2, true).is_none());
        assert!(corridor_layout_transition(Some(FormationShape::None), 1, true).is_none());
    }

    /// ADR 0031 D14: the live footprint the anchor routes as is DERIVED from the member count, and matches
    /// the decision-crate `box_footprint(N)` single source of truth. For N=1..=8: member offsets are
    /// distinct + in-bounds + non-negative (anchor top-left), and the bounding box of `box_formation(N)`
    /// equals `box_footprint(N)` — so the rover overlay reserves exactly the footprint tiles for N members.
    #[test]
    fn box_footprint_matches_box_formation_bounding_box_for_1_to_8() {
        for n in 1..=8usize {
            let layout = FormationLayout::box_formation(n);
            assert_eq!(layout.offsets.len(), n, "one offset per member (n={n})");
            // Distinct + non-negative (anchor at top-left, fills right then down).
            let set: std::collections::HashSet<_> = layout.offsets.iter().collect();
            assert_eq!(set.len(), n, "distinct tiles (n={n})");
            assert!(layout.offsets.iter().all(|&(x, y)| x >= 0 && y >= 0), "non-negative offsets (n={n})");
            // Bounding box (since min is 0,0) = (max_x+1, max_y+1) must equal box_footprint(n).
            let w = layout.offsets.iter().map(|&(x, _)| x).max().unwrap() + 1;
            let h = layout.offsets.iter().map(|&(_, y)| y).max().unwrap() + 1;
            assert_eq!((w as u8, h as u8), box_footprint(n), "bounding box == box_footprint (n={n})");
        }
    }

    /// A structure focus must stand the anchor OFF the structure's own (impassable) tile — pathing onto it
    /// reports Blocked and the squad never reaches weapon range (the invader-core no-fire bug, ADR 0026 §9).
    #[test]
    fn standoff_one_tile_steps_off_the_structure_toward_the_squad() {
        let room: RoomName = "W1N1".parse().unwrap();
        let core = Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), room);
        // Squad centroid to the west (lower x) and north (lower y) of the core.
        let squad = Position::new(RoomCoordinate::new(20).unwrap(), RoomCoordinate::new(22).unwrap(), room);
        let s = standoff_one_tile(core, squad);
        assert_ne!(s, core, "never the structure's own (impassable) tile");
        assert_eq!(core.get_range_to(s), 1, "exactly one tile off — in weapon range (ranged <=3, dismantle 1)");
        assert_eq!((s.x().u8(), s.y().u8()), (24, 24), "stepped toward the squad (west + north)");
    }

    /// Degenerate: a centroid exactly on the structure tile still yields an adjacent (off-structure) tile,
    /// never range 0 (which would re-introduce the Blocked-on-the-structure hold).
    #[test]
    fn standoff_one_tile_handles_centroid_on_the_structure() {
        let room: RoomName = "W1N1".parse().unwrap();
        let core = Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), room);
        let s = standoff_one_tile(core, core);
        assert_eq!(core.get_range_to(s), 1, "still steps off the structure");
    }

    // (squad_ready_to_depart + should_hold_at_boundary tests live with the kernel now —
    // screeps_combat_decision::rally, K0 / ADR 0028.)
}
