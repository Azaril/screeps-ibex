//! Squad support for the sim (P2.M2 validation): a squad whose footprint-aware anchor
//! ([`rover::AnchorPath`](screeps_rover::AnchorPath)) advances toward an objective while its
//! members hold formation (`anchor + offset`) and fight via the seam. This lets the sim exercise
//! **squad-level movement + cohesion** — members stay in formation across obstacles instead of
//! scattering (the squad-scatter fix), measured with [`cohesion`](screeps_combat_decision::cohesion),
//! the same instrument H3 uses. The anchor advance is **cohesion-gated** (hold if members lag) and
//! routes the squad's W×H box around walls; a [`AnchorOutcome::Blocked`] anchor surfaces a path
//! failure for the owner to respond to.

use crate::pathing::{build_combat_matrix, resolve_move_direction};
use crate::{to_engine_action, SimView};
use screeps::{Position, RoomCoordinate};
use screeps_combat_decision::{cohesion, decide_combat, CombatIntent};
use screeps_combat_engine::{CombatWorld, CreepId, Intents, PlayerId};
use screeps_rover::{AnchorOutcome, AnchorPath, LocalPathfinder};

/// Members within this Chebyshev distance of their slot count as "in formation".
const COHESION_TOL: u32 = 1;
/// Advance the anchor only when at least this fraction of members are in formation.
const ADVANCE_QUORUM: f32 = 0.75;

/// `anchor + (dx,dy)`, clamped out if it leaves the room.
fn offset_pos(anchor: Position, (dx, dy): (i32, i32)) -> Option<Position> {
    let x = anchor.x().u8() as i32 + dx;
    let y = anchor.y().u8() as i32 + dy;
    if (0..50).contains(&x) && (0..50).contains(&y) {
        Some(Position::new(RoomCoordinate::new(x as u8).ok()?, RoomCoordinate::new(y as u8).ok()?, anchor.room_name()))
    } else {
        None
    }
}

/// A squad in the sim: an anchor mover + ordered members (member `i` holds `layout[i]`).
pub struct SimSquad {
    pub owner: PlayerId,
    /// Members in slot order (member `i` ↔ `layout[i]`).
    pub members: Vec<CreepId>,
    /// Formation slot offsets relative to the anchor.
    pub layout: Vec<(i32, i32)>,
    pub anchor: AnchorPath,
    pub objective: Position,
}

impl SimSquad {
    /// The squad's bounding-box footprint `(w,h)` from its layout — the size the anchor path must
    /// fit (so the block routes as a unit, never threading a gap narrower than itself).
    pub fn footprint(&self) -> (u8, u8) {
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (0i32, 0i32, 0i32, 0i32);
        for &(dx, dy) in &self.layout {
            min_x = min_x.min(dx);
            max_x = max_x.max(dx);
            min_y = min_y.min(dy);
            max_y = max_y.max(dy);
        }
        (((max_x - min_x + 1).max(1)) as u8, ((max_y - min_y + 1).max(1)) as u8)
    }

    /// Member positions (living members only), in slot order — for cohesion measurement.
    fn member_positions(&self, sim: &SimView) -> Vec<Position> {
        self.members
            .iter()
            .filter_map(|&id| sim.friend_index(id).map(|i| sim.friends()[i].pos))
            .collect()
    }

    /// Advance the squad one tick. Measures cohesion against the current anchor; advances the
    /// anchor toward the objective only if a quorum is in formation (else holds for stragglers);
    /// then moves each member toward its formation slot and emits seam combat. Returns the engine
    /// [`Intents`] for the squad's creeps plus the anchor [`AnchorOutcome`] (`Blocked` = the path
    /// failed; the owner should respond).
    pub fn step(&mut self, world: &CombatWorld) -> (Intents, AnchorOutcome) {
        let room = self.anchor.virtual_pos.room_name();
        let sim = SimView::from_world(world, self.owner, self.anchor.virtual_pos, room);

        // Cohesion gate: only advance the anchor when the squad is gathered (members near slots).
        let positions = self.member_positions(&sim);
        let sample = cohesion::measure(&positions, Some((self.anchor.virtual_pos, &self.layout)), COHESION_TOL);
        let mut outcome = AnchorOutcome::Advanced;
        if sample.in_formation_rate >= ADVANCE_QUORUM {
            let footprint = self.footprint();
            let mut pf = LocalPathfinder;
            outcome = self.anchor.advance(self.objective, footprint, &mut pf, &mut |r| build_combat_matrix(world, r, self.owner));
        }

        // Move members to their (possibly-advanced) formation slots; fight via the seam.
        let anchor = self.anchor.virtual_pos;
        let mut intents = Intents::new();
        for (slot, &member_id) in self.members.iter().enumerate() {
            let Some(fi) = sim.friend_index(member_id) else {
                continue;
            };
            let me_pos = sim.friends()[fi].pos;

            let actions: Vec<_> = decide_combat(&sim.view_for(fi))
                .iter()
                .filter_map(|ci| to_engine_action(ci, &sim))
                .collect();
            if !actions.is_empty() {
                intents.set(member_id, actions);
            }

            let offset = self.layout.get(slot).copied().unwrap_or((0, 0));
            if let Some(slot_pos) = offset_pos(anchor, offset) {
                let goal = CombatIntent::MoveTo { target: slot_pos, range: 0 };
                if let Some(dir) = resolve_move_direction(world, me_pos, self.owner, &goal) {
                    intents.set_move(member_id, dir);
                }
            }
        }
        (intents, outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{Part, RoomName};
    use screeps_combat_engine::{resolve_tick, SimBody, SimCreep};

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }
    fn creep(id: CreepId, x: u8, y: u8) -> SimCreep {
        SimCreep {
            id,
            owner: 0,
            pos: pos(x, y),
            // balanced body so it clears fatigue and moves every tick on plains.
            body: SimBody::unboosted(&[Part::Attack, Part::Move]),
            fatigue: 0,
        }
    }

    const QUAD: [(i32, i32); 4] = [(0, 0), (1, 0), (0, 1), (1, 1)];

    fn quad_squad(anchor: Position, objective: Position) -> SimSquad {
        SimSquad {
            owner: 0,
            members: vec![1, 2, 3, 4],
            layout: QUAD.to_vec(),
            anchor: AnchorPath::new(anchor, objective),
            objective,
        }
    }

    #[test]
    fn a_quad_crosses_an_open_room_staying_in_formation() {
        // Start a 2×2 quad formed at (5,25), objective (40,25). It should arrive cohesively.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 5, 25), creep(2, 6, 25), creep(3, 5, 26), creep(4, 6, 26)],
            ..Default::default()
        };
        let mut squad = quad_squad(pos(5, 25), pos(40, 25));
        let mut worst_in_formation = 1.0f32;
        for _ in 0..80 {
            let (intents, _) = squad.step(&world);
            resolve_tick(&mut world, &intents);
            let sim = SimView::from_world(&world, 0, squad.anchor.virtual_pos, room());
            let s = cohesion::measure(&squad.member_positions(&sim), Some((squad.anchor.virtual_pos, &QUAD)), 1);
            worst_in_formation = worst_in_formation.min(s.in_formation_rate);
            if squad.anchor.virtual_pos == pos(40, 25) {
                break;
            }
        }
        // The anchor reached the objective and the squad never fell apart.
        assert!(squad.anchor.virtual_pos.x().u8() >= 38, "squad advanced to the objective");
        assert!(worst_in_formation >= 0.75, "stayed cohesive throughout (worst {})", worst_in_formation);
    }

    #[test]
    fn a_quad_routes_its_footprint_around_a_wall() {
        // A wall band with a 3-wide gap; a 2×2 quad must route through the gap (fits) and not clip.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 5, 25), creep(2, 6, 25), creep(3, 5, 26), creep(4, 6, 26)],
            ..Default::default()
        };
        for y in 0..=49u8 {
            if !(24..=26).contains(&y) {
                world.terrain.walls.insert((20, y)); // wall column with a gap at y=24..=26
            }
        }
        let mut squad = quad_squad(pos(5, 25), pos(35, 25));
        let mut blocked = false;
        for _ in 0..120 {
            let (intents, outcome) = squad.step(&world);
            if outcome == AnchorOutcome::Blocked {
                blocked = true;
            }
            resolve_tick(&mut world, &intents);
            if squad.anchor.virtual_pos.x().u8() >= 33 {
                break;
            }
        }
        assert!(!blocked, "the 2×2 fits the 3-wide gap → never Blocked");
        assert!(squad.anchor.virtual_pos.x().u8() >= 33, "squad threaded the gap to the far side");
    }

    #[test]
    fn anchor_reports_blocked_when_the_footprint_cannot_pass() {
        // A 1-wide gap a 2×2 cannot thread → the anchor reports Blocked (holds), not a blind step.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 5, 25), creep(2, 6, 25), creep(3, 5, 26), creep(4, 6, 26)],
            ..Default::default()
        };
        for y in 0..=49u8 {
            if y != 25 {
                world.terrain.walls.insert((20, y)); // only a single-tile gap at y=25
            }
        }
        let mut squad = quad_squad(pos(15, 25), pos(35, 25));
        let mut saw_blocked = false;
        for _ in 0..30 {
            let (intents, outcome) = squad.step(&world);
            if outcome == AnchorOutcome::Blocked {
                saw_blocked = true;
            }
            resolve_tick(&mut world, &intents);
        }
        assert!(saw_blocked, "a 2×2 cannot pass a 1-wide gap → Blocked surfaced");
        assert!(squad.anchor.virtual_pos.x().u8() < 20, "anchor held on the near side, did not clip through");
    }
}
