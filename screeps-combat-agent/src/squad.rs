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
use screeps::{Part, Position, RoomCoordinate};
use screeps_combat_decision::{
    cohesion, decide_combat, decide_movement, decide_squad_with_pathing,
    kite::{SquadTacticParams, MAX_KITE_OPS},
    CombatIntent, CreepOrders, FocusTarget, SquadMemberView, SquadOrderState, SquadStateDto, SquadView,
};
use screeps_combat_engine::{CombatWorld, CreepId, Intents, PlayerId};
use screeps_rover::{AnchorOutcome, AnchorPath, LocalPathfinder};

/// Members within this Chebyshev distance of their slot count as "in formation".
const COHESION_TOL: u32 = 1;
/// Advance the anchor only when at least this fraction of members are in formation.
const ADVANCE_QUORUM: f32 = 0.75;
/// Loose-mode (blob / corridor) cohesion radius — members within this of the anchor are gathered.
const LOOSE_RADIUS: u32 = 3;

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
    /// Persisted corridor/loose state: once the box can't fit (a corridor), the squad relaxes to
    /// single-file and stays loose (gated on centroid, not box formation) until it re-gathers into
    /// the box on open terrain. A blob (N>4) is always loose regardless.
    pub loose: bool,
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
        let anchor_pos = self.anchor.virtual_pos;
        let n = positions.len().max(1) as f32;

        // Mode (P2.M3): a blob (N>4) is always **loose** (centroid cohesion, single-tile footprint).
        // A small squad tries to move as a **box**; if the box can't fit (a corridor) it relaxes to
        // single-file (`self.loose`) and is gated on centroid proximity (a strung-out line is never
        // "in box formation"). The instant the box footprint can advance again — group pathfinding
        // works — it clears `self.loose`, transitioning back to a tight box as soon as possible; the
        // members then re-gather into their slots.
        let blob = self.members.len() > 4;
        let box_rate = cohesion::measure(&positions, Some((anchor_pos, &self.layout)), COHESION_TOL).in_formation_rate;
        let near_anchor = positions.iter().filter(|p| p.get_range_to(anchor_pos) <= LOOSE_RADIUS).count() as f32 / n;

        // Gate: a strung-out (loose) squad or a blob advances on centroid proximity (so it can keep
        // threading / re-gathering); a formed box advances only when actually in box formation.
        let cohesive = if blob || self.loose {
            near_anchor >= ADVANCE_QUORUM
        } else {
            box_rate >= ADVANCE_QUORUM
        };

        let mut pf = LocalPathfinder;
        let mut outcome = AnchorOutcome::Advanced;
        if cohesive {
            if blob {
                outcome = self.anchor.advance(self.objective, (1, 1), &mut pf, &mut |r| build_combat_matrix(world, r, self.owner));
            } else {
                // Always attempt to move as a box. Blocked ⇒ a corridor: relax to single-file and
                // mark loose. Not blocked ⇒ the box fits (open terrain): clear loose to re-form.
                outcome = self.anchor.advance(self.objective, self.footprint(), &mut pf, &mut |r| build_combat_matrix(world, r, self.owner));
                if outcome == AnchorOutcome::Blocked {
                    self.loose = true;
                    outcome = self.anchor.advance(self.objective, (1, 1), &mut pf, &mut |r| build_combat_matrix(world, r, self.owner));
                } else {
                    self.loose = false;
                }
            }
        }
        let loose = blob || self.loose;

        // Move members: box → exact slot; loose (blob / corridor) → clump near the anchor (they
        // queue single-file through a 1-wide corridor). Fight via the seam regardless.
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

            // Member target by mode: a corridor (loose, not a blob) tight-follows the anchor
            // single-file (its box slots are walls); a blob spreads to its slots loosely (range 1);
            // the box holds exact slots (range 0).
            let goal = if loose && !blob {
                Some(CombatIntent::MoveTo { target: anchor, range: 1 })
            } else {
                let offset = self.layout.get(slot).copied().unwrap_or((0, 0));
                let range = if loose { 1 } else { 0 };
                offset_pos(anchor, offset).map(|slot_pos| CombatIntent::MoveTo { target: slot_pos, range })
            };
            if let Some(g) = goal {
                if let Some(dir) = resolve_move_direction(world, me_pos, self.owner, &g) {
                    intents.set_move(member_id, dir);
                }
            }
        }
        (intents, outcome)
    }
}

/// A **manager-fielded** squad in the sim (P2.G3-tail Step 8): anchorless, driven by the pure
/// `decide_squad_with_pathing` (shared focus + heal assignment + the cohesive, pathfinding-scored
/// kite goal) and the per-creep `decide_movement` — exactly the live `SquadManager` + `SquadCombatJob`
/// path (no anchor mover). This is the self-play vehicle that validates cohesive kiting + focus-fire
/// + heal against the engine (no fork: the SAME decision code runs live).
pub struct ManagedSimSquad {
    pub owner: PlayerId,
    /// Members in slot order (the decision indexes the *living* subset of these each tick).
    pub members: Vec<CreepId>,
    /// Where the squad is fighting (the centroid fallback + the room).
    pub objective: Position,
    pub retreat_threshold: f32,
    state: SquadOrderState,
    /// Position-scoring weights (ADR 0019 Stage 4 tuning seam). Defaults to the shipped presets; the
    /// EXP sweep sets custom vectors via [`Self::with_tactics`] to tune them empirically.
    tactics: SquadTacticParams,
}

impl ManagedSimSquad {
    pub fn new(owner: PlayerId, members: Vec<CreepId>, objective: Position) -> Self {
        Self { owner, members, objective, retreat_threshold: 0.3, state: SquadOrderState::Forming, tactics: SquadTacticParams::default() }
    }

    /// Override the position-scoring weights (the EXP-* sweep loop, ADR 0019 Stage 4).
    pub fn with_tactics(mut self, tactics: SquadTacticParams) -> Self {
        self.tactics = tactics;
        self
    }

    /// Advance one tick: build the `SquadView` from living members, run `decide_squad_with_pathing`
    /// (the squad's ONE bounded kite search), then run the per-creep `decide_combat` + `decide_movement`
    /// with the shared directive, returning the engine [`Intents`].
    pub fn step(&mut self, world: &CombatWorld) -> Intents {
        let room = self.objective.room_name();
        let sim = SimView::from_world(world, self.owner, self.objective, room);

        // Living members in slot order — `member_views` and the decision index by THIS list.
        let living: Vec<(CreepId, usize)> = self.members.iter().filter_map(|&id| sim.friend_index(id).map(|fi| (id, fi))).collect();
        if living.is_empty() {
            return Intents::new();
        }
        let member_views: Vec<SquadMemberView> = living
            .iter()
            .map(|&(_, fi)| {
                let f = &sim.friends()[fi];
                SquadMemberView {
                    hits: f.hits,
                    hits_max: f.hits_max,
                    heal_power: f.working_parts(Part::Heal) as u32,
                    pos: Some(f.pos),
                    has_ranged: f.has_working(Part::RangedAttack),
                    // Per-tick attack output for the engage DMG reward (ADR 0019 focus_damage richness).
                    melee_power: f.working_parts(Part::Attack) as u32 * screeps_combat_engine::constants::ATTACK_POWER,
                    ranged_power: f.working_parts(Part::RangedAttack) as u32 * screeps_combat_engine::constants::RANGED_ATTACK_POWER,
                    damage_taken_last_tick: 0,
                }
            })
            .collect();

        let view = SquadView {
            members: &member_views,
            hostiles: sim.hostiles(),
            structures: sim.structures(),
            retreat_threshold: self.retreat_threshold,
            current_state: self.state,
        };
        let decision = decide_squad_with_pathing(&view, None, self.tactics, &mut |r| build_combat_matrix(world, r, self.owner), MAX_KITE_OPS);
        self.state = decision.state;

        let squad_dto = SquadStateDto {
            center: decision.center.unwrap_or(self.objective),
            room,
            movement: decision.movement,
            cohesion_radius: decision.cohesion_radius,
        };

        let mut intents = Intents::new();
        for (idx, &(member_id, fi)) in living.iter().enumerate() {
            let heal_target = decision.heal_assignments.iter().find(|a| a.healer_idx == idx).and_then(|a| {
                let &(_, tfi) = living.get(a.target_idx)?;
                let tf = &sim.friends()[tfi];
                Some(FocusTarget { pos: tf.pos, id: tf.id })
            });
            let orders = CreepOrders { focus: decision.focus, heal_target };
            let view_i = sim.view_for_with(fi, &squad_dto, orders);

            let actions: Vec<_> = decide_combat(&view_i).iter().filter_map(|ci| to_engine_action(ci, &sim)).collect();
            if !actions.is_empty() {
                intents.set(member_id, actions);
            }
            let me_pos = sim.friends()[fi].pos;
            for mv in decide_movement(&view_i) {
                if let Some(dir) = resolve_move_direction(world, me_pos, self.owner, &mv) {
                    intents.set_move(member_id, dir);
                    break;
                }
            }
        }
        intents
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::RoomName;
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
            loose: false,
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
    fn a_quad_threads_a_one_wide_corridor_single_file() {
        // A 1-wide gap a 2×2 box can't fit → M3 relaxes to single-file (footprint 1×1, members
        // clump) and threads it, re-forming on the far side.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 5, 25), creep(2, 6, 25), creep(3, 5, 26), creep(4, 6, 26)],
            ..Default::default()
        };
        for y in 0..=49u8 {
            if y != 25 {
                world.terrain.walls.insert((20, y)); // single-tile gap at y=25
            }
        }
        let mut squad = quad_squad(pos(15, 25), pos(35, 25));
        for _ in 0..150 {
            let (intents, _) = squad.step(&world);
            resolve_tick(&mut world, &intents);
            if squad.anchor.virtual_pos.x().u8() >= 33 {
                break;
            }
        }
        assert!(squad.anchor.virtual_pos.x().u8() >= 33, "relaxed to single-file and threaded the 1-wide corridor");
    }

    #[test]
    fn re_forms_a_tight_box_after_a_corridor() {
        // Thread a 1-wide corridor (forces loose/single-file), then verify the squad transitions
        // back to a TIGHT box as soon as the box footprint can path again on the open far side.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 5, 25), creep(2, 6, 25), creep(3, 5, 26), creep(4, 6, 26)],
            ..Default::default()
        };
        for y in 0..=49u8 {
            if y != 25 {
                world.terrain.walls.insert((20, y)); // single-tile gap at y=25
            }
        }
        let mut squad = quad_squad(pos(15, 25), pos(45, 25));
        let mut went_loose = false;
        for _ in 0..300 {
            let (intents, _) = squad.step(&world);
            resolve_tick(&mut world, &intents);
            went_loose |= squad.loose; // must have relaxed to pass the corridor
            if squad.anchor.virtual_pos.x().u8() >= 40 {
                break;
            }
        }
        // Let the members finish re-gathering into the box on the open side.
        for _ in 0..20 {
            let (intents, _) = squad.step(&world);
            resolve_tick(&mut world, &intents);
        }
        assert!(went_loose, "the squad relaxed to single-file in the corridor");
        assert!(!squad.loose, "re-formed: back in tight box mode once group pathfinding worked again");
        let sim = SimView::from_world(&world, 0, squad.anchor.virtual_pos, room());
        let s = cohesion::measure(&squad.member_positions(&sim), Some((squad.anchor.virtual_pos, &QUAD)), 1);
        assert!(s.in_formation_rate >= 0.75, "members re-gathered into the box (in-formation {})", s.in_formation_rate);
        assert!(s.max_pairwise <= 3, "tight again (diameter {})", s.max_pairwise);
    }

    #[test]
    fn reports_blocked_when_fully_sealed() {
        // No gap at all → even the single-file relax fails → Blocked, anchor holds on the near side.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 5, 25), creep(2, 6, 25), creep(3, 5, 26), creep(4, 6, 26)],
            ..Default::default()
        };
        for y in 0..=49u8 {
            world.terrain.walls.insert((20, y)); // fully sealed
        }
        let mut squad = quad_squad(pos(15, 25), pos(35, 25));
        let mut saw_blocked = false;
        for _ in 0..30 {
            let (intents, outcome) = squad.step(&world);
            saw_blocked |= outcome == AnchorOutcome::Blocked;
            resolve_tick(&mut world, &intents);
        }
        assert!(saw_blocked, "fully sealed → Blocked surfaced (even single-file can't pass)");
        assert!(squad.anchor.virtual_pos.x().u8() < 20, "anchor held on the near side, never clipped through");
    }

    // ── EXP-SQUAD-KITE-1: managed cohesive kiting + focus-fire + survival (P2.G3-tail Step 8) ──
    #[test]
    fn exp_squad_kite_1_managed_duo_kites_cohesively_and_focus_fires() {
        // A high-HP melee keeper + a ranged attacker + a healer, driven by the manager path
        // (decide_squad_with_pathing → per-creep decide_movement). The squad should advance to its
        // pathfinding-scored kite goal (shooting range, clear of the keeper's melee reach), stay
        // cohesive (ONE shared goal → the block doesn't separate), and chip the keeper while surviving.
        let keeper_body: Vec<Part> = std::iter::repeat(Part::Attack)
            .take(5)
            .chain(std::iter::repeat(Part::Move).take(5))
            .chain(std::iter::repeat(Part::Tough).take(10))
            .collect();
        let keeper = SimCreep { id: 99, owner: 1, pos: pos(25, 25), body: SimBody::unboosted(&keeper_body), fatigue: 0 };
        let ra_body = [Part::RangedAttack, Part::RangedAttack, Part::RangedAttack, Part::RangedAttack, Part::RangedAttack, Part::Move, Part::Move, Part::Move, Part::Move, Part::Move];
        let attacker = SimCreep { id: 1, owner: 0, pos: pos(20, 25), body: SimBody::unboosted(&ra_body), fatigue: 0 };
        let heal_body = [Part::Heal, Part::Heal, Part::Heal, Part::Move, Part::Move, Part::Move];
        let healer = SimCreep { id: 2, owner: 0, pos: pos(20, 26), body: SimBody::unboosted(&heal_body), fatigue: 0 };

        let mut world = CombatWorld { creeps: vec![keeper, attacker, healer], ..Default::default() };
        let keeper_hits_0 = world.creeps.iter().find(|c| c.id == 99).unwrap().body.hits;

        let mut squad = ManagedSimSquad::new(0, vec![1, 2], pos(25, 25));
        let mut worst_pairwise = 0u32;
        for _ in 0..50 {
            let intents = squad.step(&world);
            resolve_tick(&mut world, &intents);
            let positions: Vec<Position> = world.creeps.iter().filter(|c| c.owner == 0 && c.is_alive()).map(|c| c.pos).collect();
            if positions.len() >= 2 {
                worst_pairwise = worst_pairwise.max(cohesion::measure(&positions, None, 0).max_pairwise);
            }
        }

        let keeper_hits_1 = world.creeps.iter().find(|c| c.id == 99).map(|c| if c.is_alive() { c.body.hits } else { 0 }).unwrap_or(0);
        let duo_alive = world.creeps.iter().filter(|c| c.owner == 0 && c.is_alive()).count();

        assert!(keeper_hits_1 < keeper_hits_0, "the squad focus-fired the keeper ({keeper_hits_0} -> {keeper_hits_1})");
        assert_eq!(duo_alive, 2, "the duo kited to shooting range + survived (took no melee)");
        assert!(worst_pairwise <= 4, "the duo stayed cohesive throughout (worst pairwise {worst_pairwise})");
    }

    #[test]
    fn a_blob_of_five_advances_loosely() {
        // N>4 → loose-centroid mode: the blob advances to the objective staying near the anchor.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 5, 25), creep(2, 6, 25), creep(3, 5, 26), creep(4, 6, 26), creep(5, 5, 24)],
            ..Default::default()
        };
        let mut squad = SimSquad {
            owner: 0,
            members: vec![1, 2, 3, 4, 5],
            layout: QUAD.to_vec(), // ignored in loose mode (N>4)
            anchor: AnchorPath::new(pos(5, 25), pos(30, 25)),
            objective: pos(30, 25),
            loose: false,
        };
        for _ in 0..90 {
            let (intents, _) = squad.step(&world);
            resolve_tick(&mut world, &intents);
            if squad.anchor.virtual_pos.x().u8() >= 28 {
                break;
            }
        }
        assert!(squad.anchor.virtual_pos.x().u8() >= 28, "the 5-blob advanced to the objective");
        let sim = SimView::from_world(&world, 0, squad.anchor.virtual_pos, room());
        let near = squad.member_positions(&sim).iter().filter(|p| p.get_range_to(squad.anchor.virtual_pos) <= LOOSE_RADIUS).count();
        assert!(near >= 4, "blob stayed loosely gathered near the anchor ({} of 5 within {})", near, LOOSE_RADIUS);
    }
}
