//! The deterministic combat tick — the **two-phase accumulate-then-apply** resolution that is the
//! heart of the engine port (`processor.js`). One `resolve_tick` runs: **Phase B** — creep combat
//! actions + tower fire accumulate into per-target damage/heal pools (from tick-START positions);
//! **Phase C** — same-tile movement resolution (the [`crate::movement`] module); **Phase D** —
//! apply movement + fatigue, then net **damage-then-heal** per object and run the death check.
//! Attacks use start positions, so a creep cannot dodge a hit by moving. Drives EXP-FOUND-1 /
//! EXP-FOCUS-1 (kill inequality, focus-fire) and EXP-KITE-1 (range-3 kiting at MOVE parity).
//!
//! Engine fidelity (ground truth `C:\code\screeps-engine`):
//! - **Two phases:** all damage/heal accumulate (engine "intent phase", `processor.js:227-322`)
//!   before any object applies them at its own tick (`creeps/tick.js:118-136`). Because every
//!   pool is complete before application, the apply order is irrelevant (no chained-death drift).
//! - **Intent priority/exclusion** (`creeps/intents.js:3-31`): rangedAttack is dropped when
//!   rangedMassAttack is queued; melee attack is dropped when a heal/rangedHeal is queued.
//! - **Melee attack-back** (`_damage.js:14-19,86-91`): a melee target with ATTACK parts deals its
//!   attack power back to the attacker (rampart-exempt; ramparts arrive with the structures slice).
//! - **Two-phase netting** (`creeps/tick.js:118-128`): `hits -= damage` then `hits += heal`, so
//!   same-tick heal can rescue an otherwise-lethal hit — computed signed, never clamped mid-net.
//! - **Safe mode** (`*.js` per-intent guard): a hostile's combat against the safe-mode owner's
//!   objects is zeroed (the owner's own combat is not).
//!
//! Structures (ramparts/walls/spawn) are attack/dismantle targets with rampart RMA-shielding;
//! towers heal/repair. **Not yet modelled:** pull-based movement (rate2/rate3), tower-as-target,
//! `CombatRecording`, NPC AI, power creeps, multi-room. Tracked in `AGENTS.md`.

use crate::constants::TOWER_ENERGY_COST;
use crate::damage::{
    ranged_mass_attack_damage, tower_attack_damage_at_range, tower_heal_at_range,
    tower_repair_at_range,
};
use crate::state::*;
use screeps::{Direction, Position};
use std::collections::{HashMap, HashSet};

/// A creep combat action for one tick. Movement is separate (the `moves` field on [`Intents`]);
/// these are the offensive / heal / dismantle actions that accumulate into the pools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombatAction {
    Attack(CreepId),
    RangedAttack(CreepId),
    RangedMassAttack,
    Heal(CreepId),
    RangedHeal(CreepId),
    /// Dismantle a structure (WORK parts, range 1).
    Dismantle(StructureId),
    /// Melee-attack a structure (ATTACK parts, range 1).
    AttackStructure(StructureId),
    /// Ranged-attack a structure (RANGED_ATTACK parts, range 3).
    RangedAttackStructure(StructureId),
}

/// A tower's action for one tick (towers fire once, costing [`TOWER_ENERGY_COST`] energy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TowerAction {
    Attack(CreepId),
    /// Heal a friendly creep (range falloff 400→100).
    Heal(CreepId),
    /// Repair a friendly structure (range falloff 800→200).
    Repair(StructureId),
}

/// All actors' intents for a tick. Creep actions are keyed by creep id; tower actions by the
/// tower's index in [`CombatWorld::towers`]. (The decision of *what* to do is the agent's job,
/// H2; this resolver only executes a given set under the engine's priority rules.)
#[derive(Clone, Debug, Default)]
pub struct Intents {
    pub creeps: HashMap<CreepId, Vec<CombatAction>>,
    pub towers: HashMap<usize, TowerAction>,
    /// Per-creep move direction this tick (resolved in phase C). The *decision* (which way) is the
    /// agent's job (H2 / the rover pathfinder); this resolver only executes a given direction.
    pub moves: HashMap<CreepId, Direction>,
}

impl Intents {
    pub fn new() -> Self {
        Self::default()
    }
    /// Set a creep's combat actions for the tick.
    pub fn set(&mut self, creep: CreepId, actions: Vec<CombatAction>) -> &mut Self {
        self.creeps.insert(creep, actions);
        self
    }
    pub fn set_tower(&mut self, tower_idx: usize, action: TowerAction) -> &mut Self {
        self.towers.insert(tower_idx, action);
        self
    }
    /// Set a creep's move direction for the tick.
    pub fn set_move(&mut self, creep: CreepId, dir: Direction) -> &mut Self {
        self.moves.insert(creep, dir);
        self
    }
}

/// What happened to one creep this tick (for introspection / the future `CombatRecording`).
#[derive(Clone, Copy, Debug, Default)]
pub struct CreepOutcome {
    pub raw_damage: u32,
    pub effective_damage: u32,
    pub heal: u32,
    pub died: bool,
}

/// The per-tick report.
#[derive(Clone, Debug, Default)]
pub struct TickReport {
    pub tick: u32,
    pub outcomes: HashMap<CreepId, CreepOutcome>,
    pub deaths: Vec<CreepId>,
    pub destroyed_structures: Vec<StructureId>,
}

/// Apply the engine intent priority/exclusion table to a creep's raw action list
/// (`creeps/intents.js:3-31`, combat subset): rangedAttack dropped when rangedMassAttack present;
/// melee attack dropped when any heal action present.
fn filtered_actions(actions: &[CombatAction]) -> Vec<CombatAction> {
    let has_rma = actions
        .iter()
        .any(|a| matches!(a, CombatAction::RangedMassAttack));
    // The 'attack' intent (melee, creep or structure) is dropped when a heal or dismantle is queued.
    let drops_attack = actions.iter().any(|a| {
        matches!(
            a,
            CombatAction::Heal(_) | CombatAction::RangedHeal(_) | CombatAction::Dismantle(_)
        )
    });
    actions
        .iter()
        .copied()
        .filter(|a| match a {
            CombatAction::RangedAttack(_) | CombatAction::RangedAttackStructure(_) if has_rma => {
                false
            }
            CombatAction::Attack(_) | CombatAction::AttackStructure(_) if drops_attack => false,
            _ => true,
        })
        .collect()
}

/// Immutable per-creep snapshot taken before accumulation, so phase B can read attacker/target
/// powers + positions without borrowing `world.creeps` (phase D mutates it).
struct Snap {
    id: CreepId,
    owner: PlayerId,
    pos: Position,
    alive: bool,
    attack: u32,
    ranged: u32,
    heal: u32,
    ranged_heal: u32,
    dismantle: u32,
}

/// Immutable per-structure snapshot for phase B target lookups (`world.structures` is mutated in
/// phase D). Only living structures are included.
struct StructSnap {
    id: StructureId,
    kind: StructureKind,
    owner: Option<PlayerId>,
    pos: Position,
}

/// Resolve one combat tick in place. Returns a [`TickReport`]. Dead creeps are removed from
/// `world.creeps` at the end of the tick.
pub fn resolve_tick(world: &mut CombatWorld, intents: &Intents) -> TickReport {
    let snaps: Vec<Snap> = world
        .creeps
        .iter()
        .map(|c| Snap {
            id: c.id,
            owner: c.owner,
            pos: c.pos,
            alive: c.is_alive(),
            attack: c.body.attack_power(),
            ranged: c.body.ranged_attack_power(),
            heal: c.body.heal_power(),
            ranged_heal: c.body.ranged_heal_power(),
            dismantle: c.body.dismantle_power(),
        })
        .collect();
    let by_id: HashMap<CreepId, usize> = snaps.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    let snap = |id: CreepId| by_id.get(&id).map(|&i| &snaps[i]);

    // Living-structure snapshot for target lookups, and the set of rampart tiles (RMA skip +
    // attack-back suppression).
    let struct_snap: Vec<StructSnap> = world
        .structures
        .iter()
        .filter(|s| s.is_alive())
        .map(|s| StructSnap {
            id: s.id,
            kind: s.kind,
            owner: s.owner,
            pos: s.pos,
        })
        .collect();
    let struct_by_id: HashMap<StructureId, usize> = struct_snap
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id, i))
        .collect();
    let sstruct = |id: StructureId| struct_by_id.get(&id).map(|&i| &struct_snap[i]);
    let rampart_tiles: HashSet<(u8, u8)> = struct_snap
        .iter()
        .filter(|s| s.kind == StructureKind::Rampart)
        .map(|s| (s.pos.x().u8(), s.pos.y().u8()))
        .collect();

    let safe_owner = world.safe_mode_owner;
    // A hostile's combat against the safe-mode owner's object is zeroed.
    let zeroed = |attacker_owner: PlayerId, target_owner: PlayerId| -> bool {
        matches!(safe_owner, Some(o) if attacker_owner != o && target_owner == o)
    };
    // Structure variant: an unowned wall is never safe-mode-protected.
    let zeroed_s = |attacker_owner: PlayerId, target_owner: Option<PlayerId>| -> bool {
        matches!((safe_owner, target_owner), (Some(o), Some(t)) if attacker_owner != o && t == o)
    };
    let on_rampart = |p: Position| rampart_tiles.contains(&(p.x().u8(), p.y().u8()));

    let mut dmg: HashMap<CreepId, u32> = HashMap::new();
    let mut heal: HashMap<CreepId, u32> = HashMap::new();
    let mut struct_dmg: HashMap<StructureId, u32> = HashMap::new();
    let mut struct_heal: HashMap<StructureId, u32> = HashMap::new();
    let add = |map: &mut HashMap<u32, u32>, id: u32, amt: u32| {
        if amt > 0 {
            *map.entry(id).or_insert(0) += amt;
        }
    };

    // ── Phase B: accumulate damage + heal into per-target pools ──────────────
    for atk in &snaps {
        if !atk.alive {
            continue;
        }
        let actions = match intents.creeps.get(&atk.id) {
            Some(a) => filtered_actions(a),
            None => continue,
        };
        for action in actions {
            match action {
                CombatAction::Attack(tid) => {
                    if let Some(t) = snap(tid) {
                        if t.alive
                            && atk.pos.get_range_to(t.pos) <= 1
                            && !zeroed(atk.owner, t.owner)
                        {
                            add(&mut dmg, tid, atk.attack);
                            // Melee attack-back: the target's ATTACK parts hit the attacker —
                            // unless the attacker stands on a rampart (engine `_damage.js:17`).
                            if t.attack > 0 && !on_rampart(atk.pos) && !zeroed(t.owner, atk.owner) {
                                add(&mut dmg, atk.id, t.attack);
                            }
                        }
                    }
                }
                CombatAction::RangedAttack(tid) => {
                    if let Some(t) = snap(tid) {
                        if t.alive
                            && atk.pos.get_range_to(t.pos) <= 3
                            && !zeroed(atk.owner, t.owner)
                        {
                            add(&mut dmg, tid, atk.ranged);
                        }
                    }
                }
                CombatAction::RangedMassAttack => {
                    // Hostile creeps in range 3, skipping any standing on a rampart (engine
                    // `rangedMassAttack.js:38`).
                    for t in &snaps {
                        if t.alive && t.owner != atk.owner && !zeroed(atk.owner, t.owner) {
                            let r = atk.pos.get_range_to(t.pos);
                            if r <= 3 && !on_rampart(t.pos) {
                                add(&mut dmg, t.id, ranged_mass_attack_damage(atk.ranged, r));
                            }
                        }
                    }
                    // Hostile structures in range 3 (ramparts can be hit; other structures on a
                    // rampart tile are skipped).
                    for s in &struct_snap {
                        if s.owner == Some(atk.owner) || zeroed_s(atk.owner, s.owner) {
                            continue;
                        }
                        let r = atk.pos.get_range_to(s.pos);
                        let shielded = s.kind != StructureKind::Rampart && on_rampart(s.pos);
                        if r <= 3 && !shielded {
                            add(
                                &mut struct_dmg,
                                s.id,
                                ranged_mass_attack_damage(atk.ranged, r),
                            );
                        }
                    }
                }
                CombatAction::Heal(tid) => {
                    if let Some(t) = snap(tid) {
                        if t.alive && atk.pos.get_range_to(t.pos) <= 1 {
                            add(&mut heal, tid, atk.heal);
                        }
                    }
                }
                CombatAction::RangedHeal(tid) => {
                    if let Some(t) = snap(tid) {
                        if t.alive && atk.pos.get_range_to(t.pos) <= 3 {
                            add(&mut heal, tid, atk.ranged_heal);
                        }
                    }
                }
                CombatAction::Dismantle(sid) => {
                    if let Some(s) = sstruct(sid) {
                        if atk.pos.get_range_to(s.pos) <= 1 && !zeroed_s(atk.owner, s.owner) {
                            add(&mut struct_dmg, sid, atk.dismantle);
                        }
                    }
                }
                CombatAction::AttackStructure(sid) => {
                    if let Some(s) = sstruct(sid) {
                        if atk.pos.get_range_to(s.pos) <= 1 && !zeroed_s(atk.owner, s.owner) {
                            add(&mut struct_dmg, sid, atk.attack);
                        }
                    }
                }
                CombatAction::RangedAttackStructure(sid) => {
                    if let Some(s) = sstruct(sid) {
                        if atk.pos.get_range_to(s.pos) <= 3 && !zeroed_s(atk.owner, s.owner) {
                            add(&mut struct_dmg, sid, atk.ranged);
                        }
                    }
                }
            }
        }
    }

    // Towers act (cost energy, range falloff). A hostile tower's attack on the safe-mode owner is
    // zeroed; heal/repair target friendlies.
    for (&idx, action) in &intents.towers {
        let (tower_owner, tower_pos, can_fire) = match world.towers.get(idx) {
            Some(tw) => (tw.owner, tw.pos, tw.energy >= TOWER_ENERGY_COST),
            None => continue,
        };
        if !can_fire {
            continue;
        }
        let fired = match *action {
            TowerAction::Attack(tid) => match snap(tid) {
                Some(t) if t.alive && !zeroed(tower_owner, t.owner) => {
                    add(
                        &mut dmg,
                        tid,
                        tower_attack_damage_at_range(tower_pos.get_range_to(t.pos)),
                    );
                    true
                }
                _ => false,
            },
            TowerAction::Heal(tid) => match snap(tid) {
                Some(t) if t.alive => {
                    add(
                        &mut heal,
                        tid,
                        tower_heal_at_range(tower_pos.get_range_to(t.pos)),
                    );
                    true
                }
                _ => false,
            },
            TowerAction::Repair(sid) => match sstruct(sid) {
                Some(s) => {
                    add(
                        &mut struct_heal,
                        sid,
                        tower_repair_at_range(tower_pos.get_range_to(s.pos)),
                    );
                    true
                }
                None => false,
            },
        };
        if fired {
            world.towers[idx].energy -= TOWER_ENERGY_COST;
        }
    }

    // ── Phase C: resolve movement (engine movement.check), using tick-START positions ────────
    // Attacks above were pooled from start positions, so a creep cannot dodge a hit by moving.
    let new_positions = crate::movement::resolve_moves(world, &intents.moves);

    // ── Phase D: apply movement + fatigue, then net damage-then-heal, deaths ─────────────────
    let mut report = TickReport {
        tick: world.tick,
        ..Default::default()
    };
    let terrain = &world.terrain;
    for c in world.creeps.iter_mut() {
        // Movement application (engine movement.execute, before damage): move, then add move
        // fatigue (0 on a room-edge tile), then regen (-2 × MOVE parts).
        if let Some(&np) = new_positions.get(&c.id) {
            c.pos = np;
            let (x, y) = (np.x().u8(), np.y().u8());
            let move_fatigue = if crate::movement::is_edge(x, y) {
                0
            } else {
                c.body.fatigue_weight() * terrain.fatigue_rate(x, y)
            };
            c.fatigue += move_fatigue;
        }
        c.fatigue = c.fatigue.saturating_sub(c.body.fatigue_clear());

        let raw = dmg.get(&c.id).copied().unwrap_or(0);
        let healed = heal.get(&c.id).copied().unwrap_or(0);
        let effective = c.body.damage_after_tough(raw);
        // Signed net so a same-tick heal can rescue from otherwise-lethal damage.
        let net = c.body.hits as i64 - effective as i64 + healed as i64;
        let mut outcome = CreepOutcome {
            raw_damage: raw,
            effective_damage: effective,
            heal: healed,
            died: false,
        };
        if net <= 0 {
            c.body.hits = 0;
            outcome.died = true;
            report.deaths.push(c.id);
        } else {
            c.body.hits = (net as u32).min(c.body.hits_max());
        }
        report.outcomes.insert(c.id, outcome);
    }

    world.creeps.retain(|c| c.is_alive());

    // Structures: no TOUGH/boost; net dismantle/attack damage against tower repair, destroyed at 0.
    for s in world.structures.iter_mut() {
        let d = struct_dmg.get(&s.id).copied().unwrap_or(0);
        let h = struct_heal.get(&s.id).copied().unwrap_or(0);
        let net = s.hits as i64 - d as i64 + h as i64;
        if net <= 0 {
            s.hits = 0;
            report.destroyed_structures.push(s.id);
        } else {
            s.hits = (net as u32).min(s.hits_max);
        }
    }
    world.structures.retain(|s| s.is_alive());

    world.tick += 1;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::{BodyPartDef, SimBody};
    use screeps::{Direction, Part, Position, RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(
            RoomCoordinate::new(x).unwrap(),
            RoomCoordinate::new(y).unwrap(),
            room,
        )
    }

    fn creep(id: CreepId, owner: PlayerId, x: u8, y: u8, parts: &[(Part, u32)]) -> SimCreep {
        let body: Vec<BodyPartDef> = parts
            .iter()
            .flat_map(|&(p, n)| std::iter::repeat(BodyPartDef::new(p)).take(n as usize))
            .collect();
        SimCreep {
            id,
            owner,
            pos: pos(x, y),
            body: SimBody::new(body),
            fatigue: 0,
        }
    }

    /// Run `intents_per_tick` repeatedly, return the tick on which `watch` first dies, or None.
    fn ticks_to_death(
        world: &mut CombatWorld,
        build: impl Fn(&CombatWorld) -> Intents,
        watch: CreepId,
        max: u32,
    ) -> Option<u32> {
        for _ in 0..max {
            let intents = build(world);
            let report = resolve_tick(world, &intents);
            if report.deaths.contains(&watch) {
                return Some(report.tick);
            }
        }
        None
    }

    #[test]
    fn kill_inequality_attacker_beats_healer() {
        // A = 15 ATTACK (450 dps) vs B = 14 HEAL self-healing (168/tick). D > Hb ⇒ B dies.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 0, 25, 25, &[(Part::Attack, 15)]),
                creep(2, 1, 25, 26, &[(Part::Heal, 14)]),
            ],
            ..Default::default()
        };
        let died = ticks_to_death(
            &mut world,
            |_| {
                let mut i = Intents::new();
                i.set(1, vec![CombatAction::Attack(2)]);
                i.set(2, vec![CombatAction::Heal(2)]);
                i
            },
            2,
            50,
        );
        assert!(
            died.is_some(),
            "attacker out-DPSing the heal must kill the healer"
        );
    }

    #[test]
    fn kill_inequality_heal_outpaces_attacker() {
        // A = 5 ATTACK (150 dps) vs B = 14 HEAL (168/tick). D < Hb ⇒ B never dies.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 0, 25, 25, &[(Part::Attack, 5)]),
                creep(2, 1, 25, 26, &[(Part::Heal, 14)]),
            ],
            ..Default::default()
        };
        let died = ticks_to_death(
            &mut world,
            |_| {
                let mut i = Intents::new();
                i.set(1, vec![CombatAction::Attack(2)]);
                i.set(2, vec![CombatAction::Heal(2)]);
                i
            },
            2,
            100,
        );
        assert!(
            died.is_none(),
            "a healer out-pacing incoming DPS must survive (heal nets positive)"
        );
    }

    #[test]
    fn focus_fire_two_attackers_break_a_healer_one_cannot() {
        // Target T = 10 MOVE (1000 hits, no self-heal), healer H = 20 HEAL (240/tick) adjacent.
        // One 5-ATTACK attacker (150 < 240) can't break it; two (300 > 240) can.
        let make = |attackers: usize| CombatWorld {
            creeps: {
                let mut v = vec![
                    creep(99, 1, 25, 25, &[(Part::Move, 10)]), // target
                    creep(50, 1, 25, 26, &[(Part::Heal, 20)]), // its healer
                ];
                for k in 0..attackers {
                    v.push(creep(k as u32, 0, 24, 25 + k as u8, &[(Part::Attack, 5)]));
                }
                v
            },
            ..Default::default()
        };
        let build = |attackers: usize| {
            move |_w: &CombatWorld| {
                let mut i = Intents::new();
                i.set(50, vec![CombatAction::Heal(99)]);
                for k in 0..attackers {
                    i.set(k as u32, vec![CombatAction::Attack(99)]);
                }
                i
            }
        };
        let mut one = make(1);
        assert!(
            ticks_to_death(&mut one, build(1), 99, 60).is_none(),
            "1 attacker < heal ⇒ target lives"
        );
        let mut two = make(2);
        assert!(
            ticks_to_death(&mut two, build(2), 99, 60).is_some(),
            "2 attackers > heal ⇒ target dies (focus-fire)"
        );
    }

    #[test]
    fn tower_drain_self_heal_survives_and_burns_energy() {
        // Hostile drain creep at the room edge (range ~20 from a centred tower → 150/tick) with
        // 13 HEAL (156/tick) self-heals through it; the tower bleeds 10 energy/shot.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 1, &[(Part::Heal, 13)])], // near the N edge
            towers: vec![SimTower {
                owner: 1,
                pos: pos(25, 22),
                energy: 100,
                hits: 3000,
            }],
            ..Default::default()
        };
        for _ in 0..10 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::Heal(1)]);
            i.set_tower(0, TowerAction::Attack(1));
            resolve_tick(&mut world, &i);
        }
        assert!(
            world.creeps.iter().any(|c| c.id == 1 && c.is_alive()),
            "drain out-heals the edge tower"
        );
        assert_eq!(
            world.towers[0].energy, 0,
            "tower spent 10 energy per shot for 10 ticks"
        );
    }

    #[test]
    fn safe_mode_zeroes_hostile_combat() {
        // Owner 0 is in safe mode; a hostile (owner 1) attacking owner-0's creep does nothing.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 1, 25, 25, &[(Part::Attack, 20)]),
                creep(2, 0, 25, 26, &[(Part::Move, 5)]),
            ],
            safe_mode_owner: Some(0),
            ..Default::default()
        };
        let mut i = Intents::new();
        i.set(1, vec![CombatAction::Attack(2)]);
        let report = resolve_tick(&mut world, &i);
        assert_eq!(
            report.outcomes[&2].effective_damage, 0,
            "safe mode zeroes the hostile's attack"
        );
    }

    #[test]
    fn melee_attack_back_hits_the_attacker() {
        // Two melee creeps trade: each takes the other's attack power (the rampart-less case).
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 0, 25, 25, &[(Part::Attack, 10)]),
                creep(2, 1, 25, 26, &[(Part::Attack, 10)]),
            ],
            ..Default::default()
        };
        let mut i = Intents::new();
        i.set(1, vec![CombatAction::Attack(2)]);
        let report = resolve_tick(&mut world, &i);
        // Target took A's 300; attacker took the 300 attack-back.
        assert_eq!(report.outcomes[&2].effective_damage, 300);
        assert_eq!(report.outcomes[&1].effective_damage, 300);
    }

    #[test]
    fn kiting_at_move_parity_takes_zero_melee() {
        // EXP-KITE-1 (scripted moves; the agent that *chooses* them is H2). A ranged kiter
        // (7 RANGED + 7 MOVE, parity on plain) holds range 3 from a melee chaser (10 ATTACK +
        // 10 MOVE, parity) by stepping away in lockstep. Because attacks use tick-START positions,
        // the chaser's range-1 melee never connects while range stays 3 — the kiter takes 0 melee
        // and chips the chaser down.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 0, 30, 25, &[(Part::RangedAttack, 7), (Part::Move, 7)]), // kiter
                creep(2, 1, 27, 25, &[(Part::Attack, 10), (Part::Move, 10)]), // chaser, range 3 behind
            ],
            ..Default::default()
        };
        let kiter_max = world.creeps[0].body.hits_max();
        for _ in 0..10 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::RangedAttack(2)]); // kiter shoots
            i.set_move(1, Direction::Right); // and steps away
            i.set(2, vec![CombatAction::Attack(1)]); // chaser swings (out of range)
            i.set_move(2, Direction::Right); // and chases
            resolve_tick(&mut world, &i);
        }
        let kiter = world
            .creeps
            .iter()
            .find(|c| c.id == 1)
            .expect("kiter alive");
        let chaser = world
            .creeps
            .iter()
            .find(|c| c.id == 2)
            .expect("chaser alive");
        assert_eq!(kiter.body.hits, kiter_max, "kiter at range 3 takes 0 melee");
        assert!(
            chaser.body.hits < chaser.body.hits_max(),
            "chaser is chipped by ranged fire"
        );
        // Range held at 3 (both advanced 10 tiles in lockstep).
        assert_eq!(kiter.pos.get_range_to(chaser.pos), 3);
    }

    fn structure(
        id: StructureId,
        kind: StructureKind,
        owner: Option<PlayerId>,
        x: u8,
        y: u8,
        hits: u32,
    ) -> SimStructure {
        SimStructure {
            id,
            kind,
            owner,
            pos: pos(x, y),
            hits,
            hits_max: hits,
        }
    }

    #[test]
    fn dismantle_breaches_a_wall() {
        // A 10-WORK dismantler (500/tick) breaks a 1500-hit wall in 3 ticks (EXP-BREACH).
        let mut world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 25, &[(Part::Work, 10)])],
            structures: vec![structure(100, StructureKind::Wall, None, 25, 26, 1500)],
            ..Default::default()
        };
        let mut destroyed = false;
        for _ in 0..3 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::Dismantle(100)]);
            destroyed |= resolve_tick(&mut world, &i)
                .destroyed_structures
                .contains(&100);
        }
        assert!(
            destroyed && world.structures.is_empty(),
            "wall breached in 3 ticks"
        );
    }

    #[test]
    fn melee_destroys_a_spawn() {
        // 20-ATTACK (600/tick) destroys a 1000-hit spawn on the 2nd hit (1200 ≥ 1000).
        let mut world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 25, &[(Part::Attack, 20)])],
            structures: vec![structure(100, StructureKind::Spawn, Some(1), 25, 26, 1000)],
            ..Default::default()
        };
        let mut i = Intents::new();
        i.set(1, vec![CombatAction::AttackStructure(100)]);
        assert!(resolve_tick(&mut world, &i).destroyed_structures.is_empty());
        let mut i = Intents::new();
        i.set(1, vec![CombatAction::AttackStructure(100)]);
        assert!(resolve_tick(&mut world, &i)
            .destroyed_structures
            .contains(&100));
    }

    #[test]
    fn rampart_shields_creep_from_rma_but_not_single_target() {
        // A defender stands on its rampart at range 2. RMA skips it (rampart-shielded); a single-
        // target rangedAttack still hits it.
        let make = || CombatWorld {
            creeps: vec![
                creep(1, 0, 25, 25, &[(Part::RangedAttack, 10)]),
                creep(2, 1, 25, 27, &[(Part::Move, 5)]),
            ],
            structures: vec![structure(
                100,
                StructureKind::Rampart,
                Some(1),
                25,
                27,
                300_000,
            )],
            ..Default::default()
        };
        let mut w = make();
        let mut i = Intents::new();
        i.set(1, vec![CombatAction::RangedMassAttack]);
        assert_eq!(
            resolve_tick(&mut w, &i).outcomes[&2].raw_damage,
            0,
            "RMA skips a creep on a rampart"
        );
        let mut w = make();
        let mut i = Intents::new();
        i.set(1, vec![CombatAction::RangedAttack(2)]);
        assert!(
            resolve_tick(&mut w, &i).outcomes[&2].raw_damage > 0,
            "single-target ranged hits it"
        );
    }

    #[test]
    fn rampart_suppresses_attack_back() {
        // Attacker standing on a rampart melees a creep with ATTACK parts → target hit, no attack-back.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 0, 25, 25, &[(Part::Attack, 10)]),
                creep(2, 1, 25, 26, &[(Part::Attack, 10)]),
            ],
            structures: vec![structure(
                100,
                StructureKind::Rampart,
                Some(0),
                25,
                25,
                1000,
            )],
            ..Default::default()
        };
        let mut i = Intents::new();
        i.set(1, vec![CombatAction::Attack(2)]);
        let r = resolve_tick(&mut world, &i);
        assert_eq!(
            r.outcomes[&2].effective_damage, 300,
            "target still takes the hit"
        );
        assert_eq!(
            r.outcomes[&1].effective_damage, 0,
            "attacker on a rampart takes no attack-back"
        );
    }

    #[test]
    fn tower_heal_keeps_a_defender_alive() {
        // Our defender (no self-heal) is shot for 150/tick; a friendly tower heals 400/tick → lives.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 1, 25, 25, &[(Part::RangedAttack, 15)]), // hostile, 150 @ range 3
                creep(2, 0, 25, 28, &[(Part::Move, 5)]),          // our defender
            ],
            towers: vec![SimTower {
                owner: 0,
                pos: pos(25, 26),
                energy: 1000,
                hits: 3000,
            }],
            ..Default::default()
        };
        for _ in 0..5 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::RangedAttack(2)]);
            i.set_tower(0, TowerAction::Heal(2));
            resolve_tick(&mut world, &i);
        }
        assert!(
            world.creeps.iter().any(|c| c.id == 2 && c.is_alive()),
            "tower heal sustains the defender"
        );
    }

    #[test]
    fn tower_repair_outpaces_dismantle() {
        // Tower repair (800/tick at range ≤5) beats a 10-WORK dismantle (500/tick) → rampart holds.
        let mut world = CombatWorld {
            creeps: vec![creep(1, 1, 25, 25, &[(Part::Work, 10)])],
            structures: vec![structure(
                100,
                StructureKind::Rampart,
                Some(0),
                25,
                26,
                5000,
            )],
            towers: vec![SimTower {
                owner: 0,
                pos: pos(25, 28),
                energy: 1000,
                hits: 3000,
            }],
            ..Default::default()
        };
        for _ in 0..5 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::Dismantle(100)]);
            i.set_tower(0, TowerAction::Repair(100));
            resolve_tick(&mut world, &i);
        }
        let r = world
            .structures
            .iter()
            .find(|s| s.id == 100)
            .expect("rampart holds");
        assert_eq!(
            r.hits, r.hits_max,
            "repair 800 > dismantle 500 ⇒ rampart stays capped"
        );
    }
}
