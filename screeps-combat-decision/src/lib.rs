//! # screeps-combat-decision
//!
//! The tactical seam + the bot's pure combat decisions (ADR 0006 §B.2, seam **S17** / ADR 0015).
//!
//! This crate is the JS-free boundary that lets the bot's **real** combat decision code run both on
//! the live server and inside the in-process combat micro-sim (`screeps-combat-engine`) with no
//! fork: a decision reads a [`CombatView`] (value-type DTOs, **no `game::*` below this seam**) and
//! emits [`CombatIntent`]s. Two adapters build the view — a **live** one over `game::*` (the thin
//! per-tick shim in the bot's `jobs::squad_combat` / `missions::attack_mission`, isolated like the
//! `screeps-rover` `screeps_impl.rs`) and a **sim** one over `CombatWorld` (in
//! `screeps-combat-agent`). There is then exactly one implementation of target-selection /
//! focus-fire / heal, so self-play is `IbexAgent` vs `IbexAgent` with no tactics drift.
//!
//! **Why a standalone crate** (not a module in the bot): the decisions are pure logic over
//! `screeps-game-api` value types — the same profile as `screeps-combat-engine` / `screeps-foreman`
//! — so the sim harness depends on *this* (a tiny crate) instead of the whole bot, and the crate
//! boundary mechanically enforces the "no `game::*` below the seam" rule (this crate cannot even
//! reach the live game). **No cargo feature** (operator decision 2026-06-16): the decisions stay
//! pure and feature-free; the `game::*`-reading adapters live in the bot, never here.
//!
//! Extraction is **parity-first**: the live shim must emit byte-identical combat intents to the
//! prior inline logic (the bot's `intents::IntentRecorder` digest — which covers only the combat
//! categories Attack/RangedAttack/RangedMassAttack/Heal/RangedHeal, *not* movement). Two decisions
//! live here: [`select_focus_target`] (the squad's shared focus, was
//! `attack_mission::compute_focus_target`) and [`decide_combat`] (a creep's per-tick attack + heal
//! intents, was `squad_combat`'s `execute_*_with_orders` / `fallback_*`). **Movement (formation +
//! kiting) is deferred to P2.M2** (the anchor-mover rework) and is not part of the digest gate.
//!
//! [`cohesion`] holds the squad-cohesion *geometry* (spread / max-pairwise / in-formation-rate) —
//! the movement-workstream validation instrument and the basis for H3's military score, shared by
//! the sim and the live bot (the seg-57 wiring is H3).

/// Squad-cohesion geometry (the movement validation instrument; see module docs).
pub mod cohesion;
/// Pure per-tile pricing for cohesive, safe, higher-EV kite/flee positioning (P2.G3-tail).
pub mod kite;

use screeps::local::LocalCostMatrix;
use screeps::{Direction, Part, Position, RawObjectId, RoomCoordinate, RoomName, RoomXY, StructureType};

/// One working/destroyed body part as the decision sees it (front-to-back order, mirroring
/// `creep.body()` / the engine's per-part 100-hit pools). `hits == 0` ⇒ the part is destroyed and
/// contributes no power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombatBodyPart {
    pub part: Part,
    pub hits: u32,
}

/// A creep as the tactical decision sees it — JS-free, no live `Creep` handle. Built by the live
/// adapter (`creep.pos()/hits()/body()/try_raw_id()`) or the sim adapter (from a `SimCreep`).
#[derive(Clone, Debug, PartialEq)]
pub struct CombatCreepDto {
    /// The game object id when one exists (live creeps; sim creeps get a synthetic id).
    pub id: Option<RawObjectId>,
    pub pos: Position,
    pub hits: u32,
    pub hits_max: u32,
    /// Body in spawn order (`body[0]` degrades first, matching the engine).
    pub body: Vec<CombatBodyPart>,
}

impl CombatCreepDto {
    /// Count of *working* (`hits > 0`) parts of a given type — the tactical primitive (heal power,
    /// melee/ranged classification, MOVE parity all derive from these counts).
    pub fn working_parts(&self, part: Part) -> usize {
        self.body.iter().filter(|p| p.part == part && p.hits > 0).count()
    }
    /// Whether the creep has at least one working part of `part` (the engine `has active part`).
    pub fn has_working(&self, part: Part) -> bool {
        self.body.iter().any(|p| p.part == part && p.hits > 0)
    }
    fn is_damaged(&self) -> bool {
        self.hits < self.hits_max
    }
    fn as_target(&self) -> FocusTarget {
        FocusTarget { pos: self.pos, id: self.id }
    }
}

/// Who owns a structure, as the decision sees it (the live `as_owned()/my()` partition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
    Mine,
    Hostile,
    /// Unowned (constructed walls, roads, containers, …).
    Neutral,
}

/// A structure as the tactical decision sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct CombatStructureDto {
    pub pos: Position,
    pub structure_type: StructureType,
    pub hits: u32,
    pub hits_max: u32,
    pub ownership: Ownership,
    /// For a **tower**: its stored energy. A tower with `< TOWER_ENERGY_COST` (10) can neither fire
    /// nor heal, so it must be excluded from the threat field AND from the max-heal estimate. 0 for
    /// non-towers (irrelevant).
    pub energy: u32,
}

/// Squad-level state the per-creep decision reads. `center` is the squad's **real centroid** (the
/// shared coordinate frame); `movement`/`cohesion_radius` carry the squad's shared directive
/// ([`decide_squad_with_pathing`]) so `decide_movement` moves the block as one. `cohesion_radius == 0`
/// marks an unmanaged/solo creep (no squad goal → the per-creep fallback).
#[derive(Clone, Debug, PartialEq)]
pub struct SquadStateDto {
    /// The squad's shared coordinate frame — its centroid / virtual anchor.
    pub center: Position,
    /// The room the squad is fighting in (target selection is gated to the visible room).
    pub room: screeps::RoomName,
    /// The squad's shared movement directive this tick (the block's goal).
    pub movement: SquadMovement,
    /// Loose-centroid cohesion radius K (0 ⇒ unmanaged/solo, no squad goal).
    pub cohesion_radius: u32,
}

/// Per-creep orders the squad layer hands the per-tick decision (the live `TickOrders`, combat
/// subset). `None` ⇒ no orders this tick → the body-part-aware **fallback** path. Movement orders
/// are intentionally absent (movement rides P2.M2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CreepOrders {
    /// The shared focus **creep** (always carries an `id`; structures are handled by the in-range
    /// structure scan, mirroring the live `resolve_creep()` which is `None` for structure targets).
    pub focus: Option<FocusTarget>,
    /// This creep's assigned heal target (already resolved; `None` ⇒ heal-best-nearby).
    pub heal_target: Option<FocusTarget>,
}

/// The **read seam**: a `CombatWorld` slice from the deciding creep's perspective, JS-free. Borrows
/// the DTO backing storage (built once per tick by an adapter) so it is allocation-light to pass.
pub struct CombatView<'a> {
    pub tick: u32,
    /// The creep whose intents [`decide_combat`] computes.
    pub me: &'a CombatCreepDto,
    pub squad: &'a SquadStateDto,
    /// Per-creep orders, or `None` for the fallback path.
    pub orders: Option<CreepOrders>,
    /// Friendly creeps in view, **including `me`** (so heal-best-nearby can target self).
    pub friends: &'a [CombatCreepDto],
    /// Hostile creeps in view, in a stable order (the adapter preserves `creep_data.hostile()`
    /// order, so tie-breaks match the prior inline logic).
    pub hostiles: &'a [CombatCreepDto],
    pub structures: &'a [CombatStructureDto],
}

/// A chosen target: a position (sufficient for structures, which do not move) plus the object id
/// when the target is a creep (so the live executor can re-resolve a moving creep).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusTarget {
    pub pos: Position,
    pub id: Option<RawObjectId>,
}

/// The **write seam**: the guarded combat intents in `intents.rs` (one per engine pipeline) plus
/// the movement intents the rover system executes. Each combat intent carries its **target
/// position** (what the `IntentRecorder` digest folds) and the target's **id** when it is a creep
/// (`None` for a structure → the live executor resolves by position). Emitted by a
/// [`TacticalAgent`]; translated back into the guarded sink by the live executor, or applied to
/// `CombatWorld` by the sim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CombatIntent {
    Attack { target: Position, id: Option<RawObjectId> },
    RangedAttack { target: Position, id: Option<RawObjectId> },
    RangedMassAttack,
    Heal { target: Position, id: Option<RawObjectId> },
    RangedHeal { target: Position, id: Option<RawObjectId> },
    Dismantle { target: Position, id: Option<RawObjectId> },
    /// Move toward `target`, stopping within `range` (rover `move_to(..).range(..)`).
    MoveTo { target: Position, range: u8 },
    /// Move away from each of `from`, keeping at least `range` (rover `flee`).
    Flee { from: Vec<Position>, range: u8 },
    /// Hold position / take no action this tick.
    Idle,
}

/// A swappable tactical brain over the seam. The bot's real logic is `IbexAgent` (in
/// `screeps-combat-agent`, calling the pure decisions in this module); scripted opponents implement
/// it too, so self-play and adversarial scenarios run the same `decide` contract (per-creep).
pub trait TacticalAgent {
    /// Decide one creep's intents for the tick from the read seam.
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent>;
}

fn structure_rank(ty: StructureType) -> u32 {
    match ty {
        StructureType::InvaderCore => 0,
        StructureType::Spawn => 1,
        StructureType::Tower => 2,
        _ => 10,
    }
}

/// The squad's shared focus target — an **expected-value** choice (ADR 0020 §4.2). Among the hostiles
/// we can actually kill, pick the one whose death removes the most enemy capability per tick
/// (`threat / ttk`):
/// - **Discard the unkillable.** A hostile whose reachable heal ≥ our focusable DPS out-heals us
///   (`net == 0`), and a hostile standing on a hostile rampart shields behind it — our single-target
///   fire redirects to the rampart (engine `rangedAttack.js:33-36`), so neither dies to direct fire.
///   This makes "kill the healer first" **conditional on the healer being killable**: the old
///   unconditional rule dogpiled an out-healed / rampart-sheltered healer (the operator-observed bait).
/// - **`threat`** removed = the target's attack + ranged + heal output (killing a healer denies the
///   enemy's sustain — H heal/tick ≈ H damage/tick of value). **`ttk`** = `ceil(hits / net)`. Maximize
///   `threat / ttk` (compared by integer cross-multiply; ties → lower hits = nearer kill).
///
/// Fallbacks: a best-effort lowest-hits **unshielded** hostile (killability is a per-tick snapshot —
/// heal may drop, or we chip it down), then hostile structures by rank (InvaderCore > Spawn > Tower >
/// other), which the breach logic resolves to the shielding rampart. `our_dps` is the squad's
/// focusable single-target output (melee + ranged); `0` ⇒ no offense → straight to the fallbacks.
///
/// (NOTE: safeMode — where the enemy room nullifies all our combat — is an engage-level veto handled
/// by the upcoming Lanchester gate, not here; the DTOs don't yet carry it.)
pub fn select_focus_target(hostiles: &[CombatCreepDto], structures: &[CombatStructureDto], our_dps: u32) -> Option<FocusTarget> {
    // Primary: the top of the EV order (killable, unshielded, best threat/ttk).
    if let Some((c, _)) = ev_target_order(hostiles, structures, our_dps).first() {
        return Some(c.as_target());
    }
    // Fallback 1: best-effort lowest-hits UNSHIELDED hostile (killability is a per-tick snapshot).
    let ramparts = hostile_rampart_tiles(structures);
    if let Some(c) = hostiles.iter().filter(|c| !ramparts.contains(&(c.pos.x().u8(), c.pos.y().u8()))).min_by_key(|c| c.hits) {
        return Some(c.as_target());
    }
    // Fallback 2: hostile structures by rank (breach logic resolves a shielding rampart).
    structures
        .iter()
        .filter(|s| s.ownership == Ownership::Hostile)
        .min_by_key(|s| structure_rank(s.structure_type))
        .map(|s| FocusTarget { pos: s.pos, id: None })
}

/// Hostile-rampart tiles — a creep here is shielded (single-target fire redirects to the rampart).
fn hostile_rampart_tiles(structures: &[CombatStructureDto]) -> std::collections::HashSet<(u8, u8)> {
    structures
        .iter()
        .filter(|s| s.structure_type == StructureType::Rampart && s.ownership == Ownership::Hostile && s.hits > 0)
        .map(|s| (s.pos.x().u8(), s.pos.y().u8()))
        .collect()
}

/// **Maximum** heal/tick a creep at `pos` could receive THIS tick — the engine nets damage THEN heal
/// THEN checks death (`creeps/tick.js:120-136`), so a target dies only if `damage ≥ hits + heal`; this
/// is that `heal`. Counts BOTH sources (the "potential maximum" — we assume the enemy saves the target,
/// so we never commit to a kill it can out-heal):
/// - hostile **creep** healers in heal range (`HEAL_POWER` ≤1, `RANGED_HEAL_POWER` ≤3; incl. a
///   self-healer on the tile);
/// - hostile **towers** (room-wide, range falloff ~400→100/tick each — `tower_heal_at_range`), which
///   can heal a defender same-tick and dominate the sustain in a turtle room.
fn heal_reaching(hostiles: &[CombatCreepDto], structures: &[CombatStructureDto], pos: Position) -> u32 {
    use screeps_combat_engine::constants::{HEAL_POWER, RANGED_HEAL_POWER};
    let creep_heal: u32 = hostiles
        .iter()
        .filter(|h| h.has_working(Part::Heal))
        .map(|h| {
            let per = match h.pos.get_range_to(pos) {
                0..=1 => HEAL_POWER,
                2..=3 => RANGED_HEAL_POWER,
                _ => 0,
            };
            h.working_parts(Part::Heal) as u32 * per
        })
        .sum();
    let tower_heal: u32 = structures
        .iter()
        .filter(|s| {
            s.structure_type == StructureType::Tower
                && s.ownership == Ownership::Hostile
                && s.hits > 0
                && s.energy >= screeps_combat_engine::constants::TOWER_ENERGY_COST
        })
        .map(|t| screeps_combat_engine::damage::tower_heal_at_range(t.pos.get_range_to(pos)))
        .sum();
    creep_heal + tower_heal
}

/// Capability removed by killing `c`: its attack + ranged + heal output (denying sustain counts —
/// H heal/tick ≈ H damage/tick of value).
fn threat_value(c: &CombatCreepDto) -> u32 {
    use screeps_combat_engine::constants::{ATTACK_POWER, HEAL_POWER, RANGED_ATTACK_POWER};
    c.working_parts(Part::Attack) as u32 * ATTACK_POWER
        + c.working_parts(Part::RangedAttack) as u32 * RANGED_ATTACK_POWER
        + c.working_parts(Part::Heal) as u32 * HEAL_POWER
}

/// Killable, unshielded hostiles in EV order (best `threat / ttk` first), each with its per-tick KILL
/// BUDGET = `hits + heal reaching it` (the damage to finish it THIS tick). Shared by
/// [`select_focus_target`] (primary = first) and [`assign_focus_fire`] (spill) so they agree. Empty ⇒
/// nothing killable (out-healed, rampart-shielded, or `our_dps == 0`).
fn ev_target_order<'a>(hostiles: &'a [CombatCreepDto], structures: &[CombatStructureDto], our_dps: u32) -> Vec<(&'a CombatCreepDto, u32)> {
    let ramparts = hostile_rampart_tiles(structures);
    let mut ranked: Vec<(&CombatCreepDto, u32, u64, u64)> = hostiles
        .iter()
        .filter(|c| !ramparts.contains(&(c.pos.x().u8(), c.pos.y().u8())))
        .filter_map(|c| {
            let heal = heal_reaching(hostiles, structures, c.pos);
            let net = our_dps.saturating_sub(heal);
            if net == 0 {
                return None; // out-healed → unkillable
            }
            let ttk = c.hits.div_ceil(net).max(1) as u64;
            let threat = threat_value(c).max(1) as u64;
            Some((c, c.hits + heal, threat, ttk)) // budget = damage to finish it this tick
        })
        .collect();
    // EV desc via cross-multiply (threat_a/ttk_a vs threat_b/ttk_b); tie → lower hits first.
    ranked.sort_by(|a, b| (b.2 * a.3).cmp(&(a.2 * b.3)).then(a.0.hits.cmp(&b.0.hits)));
    ranked.into_iter().map(|(c, budget, _, _)| (c, budget)).collect()
}

/// Per-member focus with DAMAGE SPILL (ADR 0020 §4.2): allocate shooters (highest DPS first) across the
/// EV-ordered targets, capping each at its per-tick kill budget so the squad does NOT over-damage one
/// creep — once a target's budget is covered, extra shooters spill to the next target (the last soaks
/// the remainder). Members with no offense get `None`. Returns a Vec parallel to `members`; `None` ⇒
/// the consumer falls back to the shared `decision.focus`. The heal side is already deficit-capped +
/// spread by [`assign_heals`]; this is the symmetric over-damage cap.
fn assign_focus_fire(members: &[SquadMemberView], hostiles: &[CombatCreepDto], structures: &[CombatStructureDto]) -> Vec<Option<FocusTarget>> {
    let our_dps: u32 = members.iter().filter(|m| m.hits > 0).map(|m| m.melee_power + m.ranged_power).sum();
    let order = ev_target_order(hostiles, structures, our_dps);
    let mut out = vec![None; members.len()];
    if order.is_empty() {
        return out;
    }
    let mut shooters: Vec<(usize, u32)> = members
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let dps = m.melee_power + m.ranged_power;
            (m.hits > 0 && dps > 0).then_some((i, dps))
        })
        .collect();
    shooters.sort_by_key(|(_, dps)| std::cmp::Reverse(*dps));
    let mut ti = 0usize;
    let mut remaining = order[0].1 as i64;
    for (mi, dps) in shooters {
        out[mi] = Some(order[ti].0.as_target());
        remaining -= dps as i64;
        if remaining <= 0 && ti + 1 < order.len() {
            ti += 1;
            remaining = order[ti].1 as i64;
        }
    }
    out
}

/// **The per-creep combat decision** (ADR 0006 Inc B): one creep's attack + heal intents for a
/// tick. A faithful port of `squad_combat`'s `execute_attack_with_orders` + `execute_heal_with_orders`
/// (when `view.orders` is `Some`) and `fallback_attack` + `fallback_heal` (when `None`). Intents
/// are pushed in the live pipeline order — melee (A), then ranged (B), then heal — so the
/// `IntentRecorder` digest matches. Movement is **not** emitted here (it rides P2.M2).
pub fn decide_combat(view: &CombatView) -> Vec<CombatIntent> {
    let mut out = Vec::new();
    match view.orders {
        Some(orders) => {
            attack_with_orders(view, &orders, &mut out);
            heal_with_orders(view, &orders, &mut out);
        }
        None => {
            fallback_attack(view, &mut out);
            fallback_heal(view, &mut out);
        }
    }
    out
}

fn min_hits_hostile_within<'a>(view: &CombatView<'a>, range: u32) -> Option<&'a CombatCreepDto> {
    view.hostiles.iter().filter(|c| view.me.pos.get_range_to(c.pos) <= range).min_by_key(|c| c.hits)
}

/// The target a SOLO creep (no squad focus) should pick within `range`: a hostile **healer** first
/// (lowest-hits — break the enemy's sustain), else the lowest-hits hostile. Mirrors
/// [`select_focus_target`]'s healer-first priority so an unmanaged creep fights as smartly as a
/// squad-coordinated one (U8 focus consistency) instead of just chipping the nearest weakling while
/// a healer keeps the pack topped up.
fn priority_hostile_within<'a>(view: &CombatView<'a>, range: u32) -> Option<&'a CombatCreepDto> {
    let me = view.me;
    let in_range = |c: &&CombatCreepDto| me.pos.get_range_to(c.pos) <= range;
    view.hostiles.iter().filter(in_range).filter(|c| c.has_working(Part::Heal)).min_by_key(|c| c.hits).or_else(|| {
        view.hostiles.iter().filter(in_range).min_by_key(|c| c.hits)
    })
}

fn best_hostile_structure_within<'a>(view: &CombatView<'a>, range: u32) -> Option<&'a CombatStructureDto> {
    view.structures
        .iter()
        .filter(|s| s.ownership == Ownership::Hostile && view.me.pos.get_range_to(s.pos) <= range)
        .min_by_key(|s| structure_rank(s.structure_type))
}

fn attack_with_orders(view: &CombatView, orders: &CreepOrders, out: &mut Vec<CombatIntent>) {
    let me = view.me;
    // The resolved focus *creep* (the live `resolve_creep()` — `None` for structure targets).
    let focus = orders.focus;

    // Pipeline A: melee — prefer the focus if adjacent, else the lowest-hits adjacent hostile.
    if me.has_working(Part::Attack) {
        let target = match focus {
            Some(f) if me.pos.get_range_to(f.pos) <= 1 => Some(f),
            _ => min_hits_hostile_within(view, 1).map(|c| c.as_target()),
        };
        if let Some(t) = target {
            out.push(CombatIntent::Attack { target: t.pos, id: t.id });
        }
    }

    // Pipeline B: ranged — mass-attack when stacked, else focus-fire, else nearby structures.
    if me.has_working(Part::RangedAttack) {
        let in_range_3 = view.hostiles.iter().filter(|c| me.pos.get_range_to(c.pos) <= 3).count();
        let in_range_1 = view.hostiles.iter().filter(|c| me.pos.get_range_to(c.pos) <= 1).count();
        if in_range_3 > 0 {
            if in_range_1 >= 3 || (in_range_3 >= 3 && in_range_1 >= 1) {
                out.push(CombatIntent::RangedMassAttack);
            } else {
                let target = match focus {
                    Some(f) if me.pos.get_range_to(f.pos) <= 3 => Some(f),
                    _ => min_hits_hostile_within(view, 3).map(|c| c.as_target()),
                };
                if let Some(t) = target {
                    out.push(CombatIntent::RangedAttack { target: t.pos, id: t.id });
                }
            }
        } else if let Some(s) = best_hostile_structure_within(view, 3) {
            out.push(CombatIntent::RangedAttack { target: s.pos, id: None });
        }
    }
}

fn heal_with_orders(view: &CombatView, orders: &CreepOrders, out: &mut Vec<CombatIntent>) {
    if !view.me.has_working(Part::Heal) {
        return;
    }
    match orders.heal_target {
        Some(h) => {
            let range = view.me.pos.get_range_to(h.pos);
            if range <= 1 {
                out.push(CombatIntent::Heal { target: h.pos, id: h.id });
            } else if range <= 3 {
                out.push(CombatIntent::RangedHeal { target: h.pos, id: h.id });
            } else {
                heal_best_nearby(view, out);
            }
        }
        None => heal_best_nearby(view, out),
    }
}

fn fallback_attack(view: &CombatView, out: &mut Vec<CombatIntent>) {
    let me = view.me;
    if view.hostiles.is_empty() {
        // No hostile creeps → ranged then melee structures (the live order).
        if me.has_working(Part::RangedAttack) {
            if let Some(s) = best_hostile_structure_within(view, 3) {
                out.push(CombatIntent::RangedAttack { target: s.pos, id: None });
            }
        }
        if me.has_working(Part::Attack) {
            if let Some(s) = best_hostile_structure_within(view, 1) {
                out.push(CombatIntent::Attack { target: s.pos, id: None });
            }
        }
        return;
    }

    // Pipeline A: melee the adjacent target — a healer first, else the lowest-hits hostile (U8).
    if me.has_working(Part::Attack) {
        if let Some(t) = priority_hostile_within(view, 1) {
            out.push(CombatIntent::Attack { target: t.pos, id: t.id });
        }
    }
    // Pipeline B: mass-attack when ≥3 adjacent, else focus a healer (then lowest-hits) in range 3 (U8).
    if me.has_working(Part::RangedAttack) {
        let in_range_1 = view.hostiles.iter().filter(|c| me.pos.get_range_to(c.pos) <= 1).count();
        if in_range_1 >= 3 {
            out.push(CombatIntent::RangedMassAttack);
        } else if let Some(t) = priority_hostile_within(view, 3) {
            out.push(CombatIntent::RangedAttack { target: t.pos, id: t.id });
        }
    }
}

fn fallback_heal(view: &CombatView, out: &mut Vec<CombatIntent>) {
    if view.me.has_working(Part::Heal) {
        heal_best_nearby(view, out);
    }
}

/// Heal priority shared by the ordered & fallback paths (`squad_combat::heal_best_nearby`): an
/// adjacent damaged friendly (incl. self), else self if damaged, else a ranged damaged friendly.
fn heal_best_nearby(view: &CombatView, out: &mut Vec<CombatIntent>) {
    let me = view.me;
    let adjacent = view
        .friends
        .iter()
        .filter(|c| me.pos.get_range_to(c.pos) <= 1 && c.is_damaged())
        .min_by_key(|c| c.hits);
    if let Some(t) = adjacent {
        out.push(CombatIntent::Heal { target: t.pos, id: t.id });
        return;
    }
    if me.is_damaged() {
        out.push(CombatIntent::Heal { target: me.pos, id: me.id });
        return;
    }
    let ranged = view
        .friends
        .iter()
        .filter(|c| {
            let r = me.pos.get_range_to(c.pos);
            r > 1 && r <= 3 && c.is_damaged()
        })
        .min_by_key(|c| c.hits);
    if let Some(t) = ranged {
        out.push(CombatIntent::RangedHeal { target: t.pos, id: t.id });
    }
}

/// A working-melee creep with no working ranged — the thing a kiter must keep its distance from.
fn is_melee_only(c: &CombatCreepDto) -> bool {
    c.has_working(Part::Attack) && !c.has_working(Part::RangedAttack)
}

/// A kiter within this many tiles of a room edge gets that edge added as a flee repulsor (U8): the
/// raw "flee = maximize distance to the threat" steers a kiter into the room CORNER (the farthest
/// point), where at MOVE parity it loses a step against the wall and the chaser closes to melee.
/// Treating a near edge as something to flee *too* makes the kiter round the corner along the
/// interior instead of jamming into it.
const EDGE_AVOID_THRESHOLD: u32 = 6;
const ROOM_EDGE_MAX: u8 = 49;

/// Build the flee-repulsor list for a kiting creep: the threat positions plus a synthetic point on
/// each room edge the creep is within [`EDGE_AVOID_THRESHOLD`] of (placed at the creep's own
/// coordinate on that edge, so fleeing it pushes straight toward the interior). With no near edge
/// this is just `threats` — byte-identical to the prior behavior in open space.
fn kite_repulsors(pos: Position, threats: &[Position]) -> Vec<Position> {
    let mut out = threats.to_vec();
    let (x, y) = (pos.x().u8(), pos.y().u8());
    let room = pos.room_name();
    let edge = |ex: u8, ey: u8| {
        Position::new(RoomCoordinate::new(ex).expect("0..=49"), RoomCoordinate::new(ey).expect("0..=49"), room)
    };
    if (x as u32) <= EDGE_AVOID_THRESHOLD {
        out.push(edge(0, y));
    }
    if (ROOM_EDGE_MAX - x) as u32 <= EDGE_AVOID_THRESHOLD {
        out.push(edge(ROOM_EDGE_MAX, y));
    }
    if (y as u32) <= EDGE_AVOID_THRESHOLD {
        out.push(edge(x, 0));
    }
    if (ROOM_EDGE_MAX - y) as u32 <= EDGE_AVOID_THRESHOLD {
        out.push(edge(x, ROOM_EDGE_MAX));
    }
    out
}

/// **The per-creep tactical movement decision** (ADR 0006 Inc B / P2.M): one creep's movement
/// *goal* for the tick — `MoveTo`/`Flee` (the executor, live or sim, turns it into a path step via
/// rover). A faithful port of `squad_combat`'s body-part-aware `fallback_movement`/kiting:
/// - **ranged** (± melee): kite — `Flee` from a melee-only hostile within range 2 (to keep out of
///   melee while staying in shooting range), else close to range 3 of the target, else hold;
/// - **pure melee**: close to range 1 of the target;
/// - **pure healer**: follow the nearest damaged ally to range 1.
///
/// "Target" is the shared focus creep when set, else the nearest hostile. Returns 0 or 1 intents
/// (empty = hold this tick). This is the **per-creep** layer; the squad anchor advance is P2.M2.
///
/// Precedence (P2.G3-tail): (1) **critical-HP** raw-flee — the one sanctioned cohesion break (a creep
/// about to die); (2) **immediate melee-evade** for a ranged creep (the SK-duo guard, byte-identical
/// to the prior kiting) — *before* the squad goal so a directive never walks a kiter into melee;
/// (3) follow the **squad movement directive** (the block moves as one to the pathfinding-scored
/// goal); (4) **rejoin** if a managed-squad member strayed past the cohesion radius, else hold;
/// (5) **fallback** — the prior per-creep kiting/engage/heal-follow, for a solo/unmanaged creep
/// (`cohesion_radius == 0`). The existing per-creep tests exercise (5) and stay byte-identical.
pub fn decide_movement(view: &CombatView) -> Vec<CombatIntent> {
    let me = view.me;

    // (1) Critical-HP override — the one sanctioned break from the squad (a creep about to die).
    if me.hits_max > 0 && (me.hits as f32 / me.hits_max as f32) < CRITICAL_HP_FRACTION {
        let near: Vec<Position> = view.hostiles.iter().filter(|c| me.pos.get_range_to(c.pos) <= 3).map(|c| c.pos).collect();
        if !near.is_empty() {
            return vec![CombatIntent::Flee { from: near, range: 3 }];
        }
    }

    // (2) Immediate melee-evade for a ranged creep — evaluated BEFORE the squad goal (the SK-duo
    //     guard): a focus/advance directive must never charge a kiter into a melee threat.
    if me.has_working(Part::RangedAttack) {
        let melee_threats: Vec<Position> = view
            .hostiles
            .iter()
            .filter(|c| is_melee_only(c) && me.pos.get_range_to(c.pos) <= 2)
            .map(|c| c.pos)
            .collect();
        if !melee_threats.is_empty() {
            return vec![CombatIntent::Flee { from: kite_repulsors(me.pos, &melee_threats), range: 3 }];
        }
    }

    // (3) Follow the squad's shared movement directive (the block moves as one).
    match view.squad.movement {
        SquadMovement::Advance { goal, range } => return move_to_or_hold(me.pos, goal, range),
        SquadMovement::Kite { goal } => return move_to_or_hold(me.pos, goal, 0),
        SquadMovement::Hold => {
            // (4) Managed squad, "hold optimal": rejoin if strayed past K, else hold. A solo/unmanaged
            //     creep (cohesion_radius 0) has no squad goal → fall through to the per-creep fallback.
            if view.squad.cohesion_radius > 0 {
                if me.pos.get_range_to(view.squad.center) > view.squad.cohesion_radius {
                    return vec![CombatIntent::MoveTo { target: view.squad.center, range: view.squad.cohesion_radius as u8 }];
                }
                return Vec::new();
            }
        }
    }

    // (5) Fallback — the prior per-creep movement (no squad goal / solo).
    decide_movement_fallback(view)
}

/// Move toward `goal`, stopping within `range`; empty (hold) when already in range.
fn move_to_or_hold(from: Position, goal: Position, range: u8) -> Vec<CombatIntent> {
    if from.get_range_to(goal) > range as u32 {
        vec![CombatIntent::MoveTo { target: goal, range }]
    } else {
        Vec::new()
    }
}

/// The prior per-creep tactical movement (kiting/engage/heal-follow) — the fallback when there is no
/// squad directive (solo/unmanaged creep). Unchanged from the pre-G3-tail `decide_movement` body.
fn decide_movement_fallback(view: &CombatView) -> Vec<CombatIntent> {
    let me = view.me;
    let has_attack = me.has_working(Part::Attack);
    let has_ranged = me.has_working(Part::RangedAttack);
    let has_heal = me.has_working(Part::Heal);

    // What we are fighting: the squad's shared focus creep if any, else the nearest hostile.
    let nearest = view.hostiles.iter().min_by_key(|c| me.pos.get_range_to(c.pos));
    let target_pos = view
        .orders
        .and_then(|o| o.focus)
        .map(|f| f.pos)
        .or_else(|| nearest.map(|c| c.pos));

    let mv = if has_ranged {
        // Kite: break contact with any adjacent melee-only threat; else hold shooting range 3.
        let melee_threats: Vec<Position> = view
            .hostiles
            .iter()
            .filter(|c| is_melee_only(c) && me.pos.get_range_to(c.pos) <= 2)
            .map(|c| c.pos)
            .collect();
        if !melee_threats.is_empty() {
            Some(CombatIntent::Flee { from: kite_repulsors(me.pos, &melee_threats), range: 3 })
        } else {
            target_pos
                .filter(|tp| me.pos.get_range_to(*tp) > 3)
                .map(|tp| CombatIntent::MoveTo { target: tp, range: 3 })
        }
    } else if has_attack {
        // Pure melee: close to range 1.
        target_pos
            .filter(|tp| me.pos.get_range_to(*tp) > 1)
            .map(|tp| CombatIntent::MoveTo { target: tp, range: 1 })
    } else if has_heal {
        // Pure support: it can't win a melee, so evade any melee-CAPABLE hostile closing on it
        // (edge-aware — U8), self-healing as it backs off; only when nothing threatens does it move
        // up to a wounded ally to heal it. (Before U8-2 it just walked to allies and got cut down.)
        let melee_threats: Vec<Position> = view
            .hostiles
            .iter()
            .filter(|c| c.has_working(Part::Attack) && me.pos.get_range_to(c.pos) <= 2)
            .map(|c| c.pos)
            .collect();
        if !melee_threats.is_empty() {
            Some(CombatIntent::Flee { from: kite_repulsors(me.pos, &melee_threats), range: 3 })
        } else {
            view.friends
                .iter()
                .filter(|c| c.is_damaged() && c.pos != me.pos)
                .min_by_key(|c| me.pos.get_range_to(c.pos))
                .filter(|c| me.pos.get_range_to(c.pos) > 1)
                .map(|c| CombatIntent::MoveTo { target: c.pos, range: 1 })
        }
    } else {
        None
    };
    mv.into_iter().collect()
}

// ─── Squad-level decision (P2.G3) ────────────────────────────────────────────
//
// The squad analog of `decide_combat`/`decide_movement`: the pure tactics ONE layer
// up. It picks the squad's shared focus and decides engage-vs-retreat with coupled
// hysteresis, returning orders the per-creep decisions consume. Lives here (not in
// the game-coupled `SquadManager`) so the SAME squad tactics run live and in the sim
// — the whole point of the harness (no tactics fork). The live `SquadManager` and the
// sim build a [`SquadView`] and apply the [`SquadDecision`]; the manager is then a thin
// lifecycle+adapter layer with no tactics math.
//
// v1 = shared focus + engage/retreat hysteresis. Heal *assignment* (the greedy
// healer→target matching, today `SquadContext::compute_heal_assignments`) and slot
// reassignment / threat orientation migrate here next (they are already pure over the
// member data).

/// Loose-centroid cohesion radius K: a member beyond this from the squad goal/centroid rejoins
/// (Step 5); the kite scorer steepens its cohesion penalty past it.
pub const SQUAD_COHESION_RADIUS: u32 = 2;
/// HP fraction at/below which a member may break cohesion to flee individually — the ONE sanctioned
/// "individual benefit outweighs the squad" exception (a creep about to die).
pub const CRITICAL_HP_FRACTION: f32 = 0.30;

/// The squad's combat lifecycle state, as the decision computes it (the live
/// `military::squad::SquadState` combat subset — kept JS-free here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SquadOrderState {
    /// Roster incomplete / not yet at the objective.
    Forming,
    /// At/approaching the objective, no engageable target this tick.
    Moving,
    /// Actively engaging — members focus-fire the shared target.
    Engaged,
    /// Disengaging (low HP); re-engages only above the separated hysteresis band.
    Retreating,
}

/// The squad's shared per-tick movement directive — ONE goal the whole block moves toward, so
/// cohesion is structural (every in-cohesion member targets the same tile). The per-creep
/// `decide_movement` consumes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SquadMovement {
    /// Advance the block toward `goal`, stopping within `range` (engage at weapon range).
    Advance { goal: Position, range: u8 },
    /// Kite/flee the block to a pathfinding-scored safe + cohesive + value-preserving `goal` tile.
    Kite { goal: Position },
    /// Hold position (already optimal / nothing to move toward this tick).
    #[default]
    Hold,
}

/// A squad member as the squad decision sees it — the cached per-tick status the live
/// `SquadContext` already tracks, JS-free. `pos`/`has_ranged` feed the centroid + the kite plan;
/// `id`/`damage_taken_last_tick` (Step 7) feed the heal assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SquadMemberView {
    pub hits: u32,
    pub hits_max: u32,
    /// Count of working HEAL parts (the heal-capacity primitive).
    pub heal_power: u32,
    /// Current position (None before the first position sync).
    pub pos: Option<Position>,
    /// Has a working RANGED_ATTACK part (drives "the squad can kite").
    pub has_ranged: bool,
    /// Melee output/tick (working ATTACK parts × `ATTACK_POWER`) — the focus-damage reward's range-1
    /// term (ADR 0019 focus_damage richness). Default 0 (the basic constructors omit it).
    pub melee_power: u32,
    /// Ranged output/tick (working RANGED_ATTACK parts × `RANGED_ATTACK_POWER`) — the focus-damage
    /// reward's within-`r*` term. Default 0.
    pub ranged_power: u32,
    /// Damage taken since last tick (predicted incoming, for proactive heal assignment).
    pub damage_taken_last_tick: u32,
}

/// A computed heal assignment over member **indices** (the live adapter / sim resolve indices to the
/// actual creep). `healer_idx` heals `target_idx`; `expected_heal` is the (over-heal-capped) amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealAssignment {
    pub healer_idx: usize,
    pub target_idx: usize,
    pub expected_heal: u32,
}

/// The squad-level read seam: the roster's cached status + the room's hostiles/
/// structures + the squad's retreat policy + its current state (for hysteresis).
pub struct SquadView<'a> {
    pub members: &'a [SquadMemberView],
    pub hostiles: &'a [CombatCreepDto],
    pub structures: &'a [CombatStructureDto],
    /// HP fraction below which the squad retreats (composition-supplied).
    pub retreat_threshold: f32,
    /// The squad's state coming into this tick (drives the coupled hysteresis).
    pub current_state: SquadOrderState,
    /// The target room's controller is in **safe mode** owned by someone other than us → ALL our
    /// combat there is nullified (engine per-intent guard, e.g. `attack.js:30-32`). A hard engage veto
    /// (ADR 0020 §8 / the Lanchester gate): never commit to a fight we can deal zero damage in.
    pub enemy_safe_mode: bool,
}

/// The squad-level decision: the new combat state, the shared focus the members concentrate fire on
/// (the per-creep `decide_combat` consumes it), the shared movement directive (`decide_movement`
/// consumes it), the real squad centroid, and the cohesion radius. `focus` is set whenever a target
/// exists, independent of state, so a retreating ranged squad keeps shooting while it kites. `Clone`
/// (not `Copy`) ahead of the Step-7 heal-assignment vector.
#[derive(Clone, Debug, PartialEq)]
pub struct SquadDecision {
    pub state: SquadOrderState,
    pub focus: Option<FocusTarget>,
    pub movement: SquadMovement,
    /// The squad's centroid (the real coordinate frame; `None` if no member has a position).
    pub center: Option<Position>,
    pub cohesion_radius: u32,
    /// Per-tick heal assignments over member indices (the greedy healer→target matching, P2.G3-tail
    /// Step 7 — ported pure from `SquadContext::compute_heal_assignments`).
    pub heal_assignments: Vec<HealAssignment>,
    /// Per-member focus assignment with **damage spill** (ADR 0020 §4.2), parallel to `members`:
    /// `Some(t)` ⇒ this member shoots `t`; `None` ⇒ it uses the shared `focus`. Allocates shooters
    /// across EV-ordered targets, capping each at its kill budget, so combined fire doesn't
    /// over-damage one creep (the excess spills to the next target). Empty ⇒ no creep is killable
    /// (everyone falls back to `focus`).
    pub focus_assignments: Vec<Option<FocusTarget>>,
    /// The direction the formation should FACE this tick — centroid → focus (P2.G4-O2). `Some` only
    /// when Engaged with a focus. A box-fighting squad orients toward it (tanks/high-HP front, healers
    /// back, present fresh armor); a kiting (skirmish) squad ignores it. Pure tactic — the live
    /// `SquadManager` / sim applies it (`orient_toward` + `reassign_slots`); the job executes movement.
    pub orientation: Option<Direction>,
}

/// Mean HP fraction over members that have spawned (`hits_max > 0`).
fn squad_avg_hp_fraction(members: &[SquadMemberView]) -> f32 {
    let living: Vec<_> = members.iter().filter(|m| m.hits_max > 0).collect();
    if living.is_empty() {
        return 0.0;
    }
    let total: f32 = living.iter().map(|m| m.hits as f32 / m.hits_max as f32).sum();
    total / living.len() as f32
}

/// Ticks of heal sustain folded into our effective-HP when assessing a fight: a squad that out-heals
/// incoming for this long effectively carries that much extra HP. Seed; the EXP-* loop tunes it.
const ENGAGE_HEAL_SUSTAIN_TICKS: u64 = 10;
/// Lanchester attrition order (ADR 0020 §8.1): 2 = square law (force concentration rewarded — our
/// focus-fire/spill), 1 = linear. A single integer default (parity-safe, no `powf`) until archetype
/// selection (ADR 0020 step 6) picks it per matchup.
const LANCHESTER_N: u32 = 2;
/// Hysteresis band on the fighting-strength balance (permille of enemy strength): engage only when our
/// strength leads by ≥ this, retreat when it trails by ≥ this — no yo-yo around parity.
const ENGAGE_BALANCE_BAND: i64 = 200; // ±20%

/// One side's Lanchester fighting strength: `dps × ehp^(n-1)`. n=2 ⇒ rate×mass (square law, rewards
/// concentration); n=1 ⇒ rate only. Integer (parity-safe).
fn fighting_strength(dps: u64, ehp: u64, n: u32) -> u64 {
    if n >= 2 {
        dps.saturating_mul(ehp)
    } else {
        dps
    }
}

/// The EV engage assessment (ADR 0020 §4.3 Lanchester gate): is this fight winnable, and by how much?
struct EngageAssessment {
    /// Fighting-strength balance (our − killable-enemy) in permille of enemy strength; >0 favours us.
    balance: i64,
    /// Hard veto — never engage: enemy safe mode, OR incoming damage we can neither remove (kill the
    /// source) nor out-heal, so we just bleed out.
    unwinnable: bool,
}

/// Assess engage-vs-retreat by Lanchester fighting strength over the **killable** enemy force, plus the
/// hard vetoes the kill calc surfaces (ADR 0020 §8.2): out-healed/shielded enemies can't be cleared, and
/// energized hostile towers + unkillable creeps are irremovable damage — if that exceeds our heal
/// sustain (or the room is safe-moded), the fight is unwinnable regardless of the strength balance.
fn assess_engage(view: &SquadView, centroid: Option<Position>) -> EngageAssessment {
    use screeps_combat_engine::constants::{ATTACK_POWER, HEAL_POWER, RANGED_ATTACK_POWER, TOWER_ENERGY_COST};
    if view.enemy_safe_mode {
        return EngageAssessment { balance: -1000, unwinnable: true }; // our combat is nullified
    }
    let creep_dps = |c: &CombatCreepDto| -> u64 {
        c.working_parts(Part::Attack) as u64 * ATTACK_POWER as u64 + c.working_parts(Part::RangedAttack) as u64 * RANGED_ATTACK_POWER as u64
    };
    let our_dps: u64 = view.members.iter().filter(|m| m.hits > 0).map(|m| (m.melee_power + m.ranged_power) as u64).sum();
    let our_heal: u64 = view.members.iter().filter(|m| m.hits > 0).map(|m| m.heal_power as u64 * HEAL_POWER as u64).sum();
    let our_hits: u64 = view.members.iter().filter(|m| m.hits > 0).map(|m| m.hits as u64).sum();
    let our_ehp = our_hits + our_heal * ENGAGE_HEAL_SUSTAIN_TICKS;

    // Killable enemies (we out-damage their max same-tick heal incl. towers) → the force we can clear.
    let order = ev_target_order(view.hostiles, view.structures, our_dps as u32);
    let killable: std::collections::HashSet<(u8, u8)> = order.iter().map(|(c, _)| (c.pos.x().u8(), c.pos.y().u8())).collect();
    let (mut killable_dps, mut killable_ehp, mut unkillable_dps) = (0u64, 0u64, 0u64);
    for c in view.hostiles {
        let d = creep_dps(c);
        if killable.contains(&(c.pos.x().u8(), c.pos.y().u8())) {
            killable_dps += d;
            killable_ehp += c.hits as u64;
        } else {
            unkillable_dps += d; // out-healed / rampart-shielded → we can't remove this damage source
        }
    }
    // Energized hostile tower damage at our centroid — irremovable on a fight's timescale.
    let tower_dps: u64 = centroid.map_or(0, |ctr| {
        view.structures
            .iter()
            .filter(|s| s.structure_type == StructureType::Tower && s.ownership == Ownership::Hostile && s.hits > 0 && s.energy >= TOWER_ENERGY_COST)
            .map(|t| screeps_combat_engine::damage::tower_attack_damage_at_range(ctr.get_range_to(t.pos)) as u64)
            .sum()
    });
    // We bleed out if damage we can neither kill nor out-heal is positive.
    let unwinnable = unkillable_dps + tower_dps > our_heal;

    // No enemy creep deals damage (e.g. a STRUCTURE SIEGE — dismantle/raze, whose offense isn't
    // melee/ranged) ⇒ there's no creep attrition race to lose, so the Lanchester μ doesn't apply;
    // it's engageable (the tower `unwinnable` veto above still guards a tower turtle).
    let balance = if killable_dps == 0 && unkillable_dps == 0 {
        1000
    } else {
        let our_strength = fighting_strength(our_dps, our_ehp, LANCHESTER_N);
        let enemy_strength = fighting_strength(killable_dps, killable_ehp, LANCHESTER_N).max(1);
        ((our_strength as i128 - enemy_strength as i128) * 1000 / enemy_strength as i128).clamp(-1000, 1000) as i64
    };
    EngageAssessment { balance, unwinnable }
}

/// **The squad-level tactical decision** (ADR 0008 §4, P2.G3). Picks the squad's shared
/// focus ([`select_focus_target`] from the whole roster's perspective) and resolves
/// engage-vs-retreat with **coupled hysteresis** (no yo-yo): once `Retreating`, the squad
/// re-engages only above a band well separated from the retreat threshold (and never
/// while a member is critical); otherwise it retreats on the trigger and engages when a
/// target exists. The per-creep `decide_combat`/`decide_movement` consume the focus +
/// state; the live `SquadManager` and the sim share this one implementation.
pub fn decide_squad(view: &SquadView) -> SquadDecision {
    // Our focusable single-target output (melee + ranged) over living members — feeds the EV focus
    // pick's kill-inequality (is a target out-healed?) and ttk ranking (ADR 0020 §4.2 / §8.2).
    let our_dps: u32 = view.members.iter().filter(|m| m.hits > 0).map(|m| m.melee_power + m.ranged_power).sum();
    let focus = select_focus_target(view.hostiles, view.structures, our_dps);
    let engaged_or_moving = if focus.is_some() {
        SquadOrderState::Engaged
    } else {
        SquadOrderState::Moving
    };

    let center = cohesion::centroid(&member_positions(view.members));

    // EV winnability (ADR 0020 §4.3): the Lanchester fighting-strength balance over the KILLABLE enemy
    // force + the hard vetoes (safeMode / irremovable damage we can't out-heal). This replaces the old
    // flat-HP `squad_should_retreat` so we commit only to fights we predict we WIN, not just ones where
    // we're healthy — and retreat from an unwinnable turtle/tower room even at full HP.
    let assess = assess_engage(view, center);
    // HP safety floor (kept from the old heuristic, OR'd in): pull back when badly hurt regardless of
    // the strength balance, and don't re-engage until recovered above a separated band (no yo-yo).
    let avg = squad_avg_hp_fraction(view.members);
    let any_critical = view.members.iter().any(|m| m.hits_max > 0 && (m.hits as f32 / m.hits_max as f32) < 0.25);
    let re_engage_band = (view.retreat_threshold + 0.3).min(0.95);

    let retreat_now = assess.unwinnable || assess.balance <= -ENGAGE_BALANCE_BAND || any_critical || avg < view.retreat_threshold;
    let can_reengage = !assess.unwinnable && !any_critical && avg > re_engage_band && assess.balance >= ENGAGE_BALANCE_BAND;
    let state = match view.current_state {
        SquadOrderState::Retreating => {
            if can_reengage {
                engaged_or_moving
            } else {
                SquadOrderState::Retreating
            }
        }
        _ => {
            if retreat_now {
                SquadOrderState::Retreating
            } else {
                engaged_or_moving
            }
        }
    };

    // The non-pathing movement directive: engage advances the block toward the focus at weapon range
    // (ranged 3, else 1); retreat/idle hold. `decide_squad_with_pathing` overrides engage-vs-melee +
    // retreat with the pathfinding-scored kite goal.
    let squad_has_ranged = view.members.iter().any(|m| m.has_ranged);
    let movement = match (state, focus) {
        (SquadOrderState::Engaged, Some(f)) => SquadMovement::Advance {
            goal: f.pos,
            range: if squad_has_ranged { 3 } else { 1 },
        },
        _ => SquadMovement::Hold,
    };

    let heal_assignments = assign_heals(view.members);
    // Damage spill (ADR 0020 §4.2): per-member focus so combined fire doesn't over-damage one creep.
    let focus_assignments = assign_focus_fire(view.members, view.hostiles, view.structures);

    // Formation facing (O2): when engaged with a focus, the block faces the threat — the centroid →
    // focus direction. A box-fighting squad orients to it; a kiting squad ignores it. Pure: the
    // adapter applies it (`orient_toward` + `reassign_slots`).
    let orientation = match (state, focus, center) {
        (SquadOrderState::Engaged, Some(f), Some(c)) => c.get_direction_to(f.pos),
        _ => None,
    };

    SquadDecision {
        state,
        focus,
        movement,
        center,
        cohesion_radius: SQUAD_COHESION_RADIUS,
        heal_assignments,
        focus_assignments,
        orientation,
    }
}

/// Greedy heal assignment over member indices (a faithful pure port of
/// `SquadContext::compute_heal_assignments`): sort the wounded by urgency (deficit + predicted
/// incoming), greedily give each the available healer with the most healing (range bands 12@≤1 /
/// 4@≤3, adjacent preferred on a tie), cap to the remaining deficit; then any idle healer pre-heals
/// the in-range member taking the most predicted damage. Indices are into `members`; the adapter
/// resolves them to creeps.
fn assign_heals(members: &[SquadMemberView]) -> Vec<HealAssignment> {
    let healers: Vec<usize> = (0..members.len())
        .filter(|&i| members[i].heal_power > 0 && members[i].pos.is_some())
        .collect();
    if healers.is_empty() {
        return Vec::new();
    }

    struct Target {
        idx: usize,
        pos: Position,
        remaining: u32,
    }
    let mut targets: Vec<Target> = (0..members.len())
        .filter_map(|i| {
            let m = &members[i];
            let pos = m.pos?;
            if m.hits_max == 0 || (m.hits >= m.hits_max && m.damage_taken_last_tick == 0) {
                return None;
            }
            let deficit = m.hits_max - m.hits;
            Some(Target { idx: i, pos, remaining: deficit + m.damage_taken_last_tick })
        })
        .collect();
    targets.sort_by_key(|t| std::cmp::Reverse(t.remaining));

    let mut assigned = vec![false; healers.len()];
    let mut out = Vec::new();

    for t in targets.iter_mut() {
        if t.remaining == 0 {
            continue;
        }
        let mut best: Option<(usize, u32, bool)> = None; // (healer slot, heal, ranged)
        for (slot, &mi) in healers.iter().enumerate() {
            if assigned[slot] {
                continue;
            }
            let hp = members[mi].pos.expect("healer filtered to have a position");
            let range = hp.get_range_to(t.pos);
            let (heal, ranged) = if range <= 1 {
                (members[mi].heal_power * 12, false)
            } else if range <= 3 {
                (members[mi].heal_power * 4, true)
            } else {
                continue;
            };
            let better = match best {
                None => true,
                Some((_, bh, br)) => heal > bh || (heal == bh && !ranged && br),
            };
            if better {
                best = Some((slot, heal, ranged));
            }
        }
        if let Some((slot, heal, _)) = best {
            assigned[slot] = true;
            let effective = heal.min(t.remaining);
            t.remaining = t.remaining.saturating_sub(effective);
            out.push(HealAssignment { healer_idx: healers[slot], target_idx: t.idx, expected_heal: effective });
        }
    }

    // Preemptive: an idle healer pre-heals the in-range member taking the most predicted damage.
    for (slot, &mi) in healers.iter().enumerate() {
        if assigned[slot] {
            continue;
        }
        let hp = members[mi].pos.expect("healer filtered to have a position");
        let best = (0..members.len())
            .filter(|&j| j != mi && members[j].pos.is_some_and(|p| hp.get_range_to(p) <= 3))
            .max_by_key(|&j| members[j].damage_taken_last_tick);
        if let Some(j) = best {
            let m = &members[j];
            if m.damage_taken_last_tick > 0 || m.hits < m.hits_max {
                let range = hp.get_range_to(m.pos.unwrap());
                let heal = if range > 1 { members[mi].heal_power * 4 } else { members[mi].heal_power * 12 };
                out.push(HealAssignment { healer_idx: mi, target_idx: j, expected_heal: heal });
            }
        }
    }

    out
}

/// Member positions that have synced (the centroid input).
fn member_positions(members: &[SquadMemberView]) -> Vec<Position> {
    members.iter().filter_map(|m| m.pos).collect()
}

/// A threat's plain-terrain fatigue cadence — ticks it spends to step one tile (ADR 0019 Stage 2,
/// the reachability seed speed). Engine model: a step adds `weight × 2` fatigue (plain rate), a tick
/// clears `2 × move`, so ticks/step = ceil(weight / move). `weight` = working non-MOVE parts (combat
/// bodies don't carry, so every other part generates fatigue). `None` ⇒ no working MOVE → immobile,
/// not a chaser (Guard 5: seeds no reachability wave).
fn threat_step_ticks(c: &CombatCreepDto) -> Option<u32> {
    let mut move_parts = 0u32;
    let mut weight = 0u32;
    for bp in &c.body {
        if bp.hits == 0 {
            continue;
        }
        if bp.part == Part::Move {
            move_parts += 1;
        } else {
            weight += 1;
        }
    }
    (move_parts > 0).then(|| weight.div_ceil(move_parts).max(1))
}

/// Hostiles a kiting block must price for safety: melee-capable threats (incl. keepers, which carry
/// both) are kept beyond melee (reach 2 → kite to range 3, still shootable); ranged-only threats get
/// reach 0 (a ranged squad trades at range, it can't out-kite an equal-range threat — the value term
/// holds range 3). Harmless creeps (no attack parts) are skipped. (Towers are priced separately.)
fn kite_threats(hostiles: &[CombatCreepDto]) -> Vec<kite::KiteThreat> {
    hostiles
        .iter()
        .filter_map(|c| {
            let melee = c.has_working(Part::Attack);
            let ranged = c.has_working(Part::RangedAttack);
            if !melee && !ranged {
                return None;
            }
            let working = |part: Part| c.body.iter().filter(|bp| bp.part == part && bp.hits > 0).count() as u32;
            Some(kite::KiteThreat {
                pos: c.pos,
                kind: if melee { kite::ThreatKind::MeleeOnly } else { kite::ThreatKind::Ranged },
                reach: if melee { 2 } else { 0 },
                step_ticks: threat_step_ticks(c),
                attack_power: working(Part::Attack) * screeps_combat_engine::constants::ATTACK_POWER,
                ranged_power: working(Part::RangedAttack) * screeps_combat_engine::constants::RANGED_ATTACK_POWER,
            })
        })
        .collect()
}

/// Live hostile towers that can actually FIRE — i.e. with `>= TOWER_ENERGY_COST` stored energy (a
/// drained tower deals no damage, so it must not shape the threat field / tower-avoidance term).
fn kite_towers(structures: &[CombatStructureDto]) -> Vec<kite::KiteTower> {
    use screeps_combat_engine::constants::TOWER_ENERGY_COST;
    structures
        .iter()
        .filter(|s| {
            s.ownership == Ownership::Hostile && s.structure_type == StructureType::Tower && s.hits > 0 && s.energy >= TOWER_ENERGY_COST
        })
        .map(|s| kite::KiteTower { pos: s.pos })
        .collect()
}

/// The actual-hits inputs for the engage DMG reward (ADR 0019 focus_damage richness): the squad's own
/// melee/ranged output (per-tick, from the living members) + the focus creep's hits (kill-priority) +
/// the **maximum same-tick heal** reaching the focus (enemy creep healers AND energized hostile towers,
/// via [`heal_reaching`] — the engine nets damage→heal→death, so the heal we must out-damage). So the
/// engage reward closes a melee block to range 1, presses a near-dead focus, and disengages a focus the
/// enemy can out-heal (creeps or towers).
fn focus_damage_inputs(view: &SquadView, focus_pos: Position) -> kite::FocusDamage {
    let melee_power: u32 = view.members.iter().filter(|m| m.hits > 0).map(|m| m.melee_power).sum();
    let ranged_power: u32 = view.members.iter().filter(|m| m.hits > 0).map(|m| m.ranged_power).sum();
    let focus_hits = view.hostiles.iter().find(|h| h.pos == focus_pos).map(|h| h.hits).unwrap_or(0);
    let focus_heal = heal_reaching(view.hostiles, view.structures, focus_pos);
    kite::FocusDamage { melee_power, ranged_power, focus_hits, focus_heal }
}

/// Build the per-(room, tick) shared [`kite::PositionLayers`] (threat field + reachability flood) from
/// the room's hostiles + structures (ADR 0019 Stage 3b build-once-per-room). These layers depend only
/// on the room's enemies — not the deciding squad — so the live `SquadManager` builds this **once per
/// room** and passes it to every squad's [`decide_squad_with_pathing`] via `shared`, instead of each
/// squad rebuilding the floods. The per-squad cohesion still comes from each squad's own search `g`.
pub fn build_room_layers(
    hostiles: &[CombatCreepDto],
    structures: &[CombatStructureDto],
    room: RoomName,
    matrix: &screeps::local::LocalCostMatrix,
    max_ops: u32,
) -> kite::PositionLayers {
    let threats = kite_threats(hostiles);
    let towers = kite_towers(structures);
    kite::PositionLayers::build(&threats, &towers, room, matrix, max_ops)
}

/// **The full squad decision incl. the pathfinding-scored kite goal** (P2.G3-tail). Runs
/// [`decide_squad`] for the focus + hysteresis + state, then — only when kiting is warranted
/// (`Retreating`, or `Engaged` with a ranged squad and a melee-capable threat near the centroid) —
/// runs ONE [`kite::plan_kite_anchor`] to override `movement` with a `Kite` goal that is
/// simultaneously safe, cohesive, and value-preserving (a `None` plan ⇒ holding is best). The live
/// `SquadManager` and the sim both call this with their room's cost matrix; the bounded local search
/// is shared (no fork). `decide_squad` alone is the no-pathing path (it never searches).
/// Weight per blocker-hit for the combat breach search — larger than the max step count (50×50) so
/// the corridor minimizes total hits to clear, ties broken by length (mirrors the derelict breach
/// pricing). The *algorithm* is the pathfinding system's `room_grid_dijkstra`; this is combat pricing.
const BREACH_HIT_WEIGHT: u64 = 4_096;

/// O3 — layered dismantle targeting. When the squad's focus is a hostile **structure** that can only
/// be reached by clearing a hostile rampart/wall, redirect the focus to the FIRST such blocker on the
/// cheapest breach corridor — break the breach before the target it shields. Pure: the search is the
/// pathfinding system's [`screeps_rover::room_grid_dijkstra`]; combat supplies the *pricing* —
/// terrain walls + non-rampart structures are impassable (route around / the target is the goal),
/// hostile ramparts/walls are dismantlable, priced by their hits. Returns the focus unchanged when
/// the target is already reachable, there's no focus structure, or no corridor exists.
fn breach_redirect(
    focus: FocusTarget,
    centroid: Position,
    structures: &[CombatStructureDto],
    room_callback: &mut dyn FnMut(RoomName) -> Option<LocalCostMatrix>,
) -> FocusTarget {
    // Structures carry no resolved id in the focus (creeps do) — only redirect a structure focus,
    // and only within the squad's room (the breach search is single-room).
    if focus.id.is_some() || focus.pos.room_name() != centroid.room_name() {
        return focus;
    }
    let room = centroid.room_name();
    let matrix = match room_callback(room) {
        Some(m) => m,
        None => return focus,
    };
    let goal = (focus.pos.x().u8(), focus.pos.y().u8());

    // Classify hostile structures by tile: ramparts/walls are dismantlable (priced by hits); every
    // other hostile structure (and the goal tile itself) blocks walking-through — the search routes
    // around them and stops at `goal_range` 1 of the target.
    let mut breach_hits: std::collections::HashMap<(u8, u8), u64> = std::collections::HashMap::new();
    let mut solid: std::collections::HashSet<(u8, u8)> = std::collections::HashSet::new();
    for s in structures.iter() {
        let tile = (s.pos.x().u8(), s.pos.y().u8());
        match s.structure_type {
            // Constructed walls are unowned (`Neutral`) but always block + are dismantlable.
            StructureType::Wall => {
                breach_hits.insert(tile, s.hits as u64);
            }
            // A hostile rampart shields its tile and is dismantlable; ours/none don't block us.
            StructureType::Rampart if s.ownership == Ownership::Hostile => {
                breach_hits.insert(tile, s.hits as u64);
            }
            // Any other hostile structure blocks walking-through → route around it.
            _ if s.ownership == Ownership::Hostile => {
                solid.insert(tile);
            }
            // Our/neutral non-wall structures (roads, containers, …) don't block the corridor.
            _ => {}
        }
    }

    let enter_cost = |x: u8, y: u8| -> Option<u64> {
        // Terrain wall (baked as max cost by the caller's matrix) → impassable.
        if RoomXY::checked_new(x, y).map(|xy| matrix.get(xy) == u8::MAX).unwrap_or(true) {
            return None;
        }
        if (x, y) == goal {
            return None; // the target's own tile isn't walkable; the search stops at range 1.
        }
        if let Some(hits) = breach_hits.get(&(x, y)) {
            return Some(1 + hits * BREACH_HIT_WEIGHT);
        }
        if solid.contains(&(x, y)) {
            return None; // another structure blocks the tile → route around.
        }
        Some(1)
    };

    let start = (centroid.x().u8(), centroid.y().u8());
    let path = match screeps_rover::room_grid_dijkstra(&enter_cost, start, goal, 1) {
        Some(p) => p,
        None => return focus, // no corridor even through dismantlable blockers
    };
    // The first dismantlable blocker on the corridor is the breach to break first.
    for tile in path {
        if breach_hits.contains_key(&tile) {
            if let (Ok(cx), Ok(cy)) = (RoomCoordinate::new(tile.0), RoomCoordinate::new(tile.1)) {
                return FocusTarget { pos: Position::new(cx, cy, room), id: None };
            }
        }
    }
    focus
}

pub fn decide_squad_with_pathing(
    view: &SquadView,
    shared: Option<&kite::PositionLayers>,
    tactics: kite::SquadTacticParams,
    room_callback: &mut dyn FnMut(RoomName) -> Option<LocalCostMatrix>,
    max_ops: u32,
) -> SquadDecision {
    let mut decision = decide_squad(view);
    let centroid = match decision.center {
        Some(c) => c,
        None => return decision, // no positioned members → nothing to kite
    };

    // O3 — layered dismantle: if the focus is a structure shielded by a rampart/wall, redirect to the
    // breach blocker on the path (break it first) and re-aim an Advance at it. Runs only in the
    // structure-siege phase (a structure focus means no hostile creeps remain), so the per-tick grid
    // search is bounded to that phase.
    if let Some(focus) = decision.focus {
        let redirected = breach_redirect(focus, centroid, view.structures, room_callback);
        if redirected.pos != focus.pos {
            decision.focus = Some(redirected);
            if let SquadMovement::Advance { range, .. } = decision.movement {
                decision.movement = SquadMovement::Advance { goal: redirected.pos, range };
            }
        }
    }

    let should_kite = match decision.state {
        SquadOrderState::Retreating => true,
        SquadOrderState::Engaged => {
            // Kite only once a melee-capable threat is close enough that *holding distance* matters
            // (within the kite-maintain band of the centroid). Farther out, the squad keeps the
            // `Advance` directive and closes to weapon range first — otherwise the cohesion term
            // (centred on the squad's current position) would out-weigh the value term and the squad
            // would sit out of shooting range. `3` = shooting range, where it transitions to kiting.
            let squad_has_ranged = view.members.iter().any(|m| m.has_ranged);
            let melee_threat_near = view
                .hostiles
                .iter()
                .any(|c| c.has_working(Part::Attack) && centroid.get_range_to(c.pos) <= 3);
            squad_has_ranged && melee_threat_near
        }
        _ => false,
    };

    // ENGAGE positioning (ADR 0019 Stage 3b): a ranged squad that is engaged with a focus and NOT
    // kiting picks the best engagement tile via the SAME scored search, reweighted to stand-and-fight
    // (`KiteScoreParams::engage()` — in weapon range of the focus, minimal threat, cohesive) instead
    // of a straight-line `Advance` to the focus. flee (kite) vs stand (engage) thus share one search,
    // differing only by the weight preset. Ranged-only: a melee/siege squad keeps the range-1 Advance
    // (the weapon-range r* parameterization is a follow-up). A structure (breach) focus keeps the
    // breach Advance set above.
    //
    // CLOSE *AND* POSITION IN ONE SEARCH: the engage preset's dominant proximity (advance-to-damage)
    // layer makes the bounded flood's best-effort tile the one closest to the focus, so a distant focus
    // is marched toward each tick; proximity is 0 once inside `r*`, so on arrival the safety/cohesion/
    // DMG terms pick the engage tile. No separate "approach vs position" branch — the layer does both
    // (the survival veto still forbids marching onto a lethal tile). EXP-COHESION-1 covers the march.
    let squad_has_ranged = view.members.iter().any(|m| m.has_ranged);
    let engage_position = !should_kite
        && matches!(decision.state, SquadOrderState::Engaged)
        && squad_has_ranged
        && decision.focus.is_some_and(|f| f.id.is_some());

    // Survival-veto / safety inputs (#2/#4): the most-fragile living member's hits + the squad's heal
    // sustain; and the optimal weapon range r* (3 ranged, 1 melee) for the proximity + focus terms.
    let fragile_hits = view.members.iter().filter(|m| m.hits > 0).map(|m| m.hits).min().unwrap_or(0);
    let squad_heal: u32 = view.members.iter().map(|m| m.heal_power).sum();
    let weapon_range = if squad_has_ranged { 3 } else { 1 };

    if should_kite {
        let threats = kite_threats(view.hostiles);
        let towers = kite_towers(view.structures);
        let kite_view = kite::SquadKiteView {
            centroid,
            threats: &threats,
            towers: &towers,
            focus: decision.focus.map(|f| f.pos),
            // Kite weights the DMG term 0, so the richness inputs are moot here — keep it None.
            focus_damage: None,
            params: tactics.kite,
            fragile_hits,
            squad_heal,
            weapon_range,
        };
        decision.movement = match kite::plan_kite_anchor(&kite_view, shared, room_callback, max_ops) {
            Some(plan) => SquadMovement::Kite { goal: plan.goal },
            None => SquadMovement::Hold, // already the safest + most cohesive tile
        };
    } else if engage_position {
        let threats = kite_threats(view.hostiles);
        let towers = kite_towers(view.structures);
        // ACTUAL-HITS DMG richness (ADR 0019 focus_damage): the squad's own melee/ranged output, the
        // focus's hits (kill-priority), and the heal/tick reaching the focus (enemy healers in heal
        // range of it) — so the engage reward rewards tiles by net hits actually landed, pulls a melee
        // block to range 1, and shrinks to 0 against an out-healed target (→ safety repositions it).
        let focus_damage = decision.focus.and_then(|f| f.id.map(|_| focus_damage_inputs(view, f.pos)));
        let engage_view = kite::SquadKiteView {
            centroid,
            threats: &threats,
            towers: &towers,
            focus: decision.focus.map(|f| f.pos),
            focus_damage,
            params: tactics.engage,
            fragile_hits,
            squad_heal,
            weapon_range,
        };
        decision.movement = match kite::plan_kite_anchor(&engage_view, shared, room_callback, max_ops) {
            // Move onto the scored engagement tile (range 0); `None` ⇒ the centroid is already the best
            // place to fight from → hold and deal damage.
            Some(plan) => SquadMovement::Advance { goal: plan.goal, range: 0 },
            None => SquadMovement::Hold,
        };
    }

    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }
    fn body(parts: &[(Part, u32)]) -> Vec<CombatBodyPart> {
        parts
            .iter()
            .flat_map(|&(part, n)| std::iter::repeat_n(CombatBodyPart { part, hits: 100 }, n as usize))
            .collect()
    }
    fn raw(id: u8) -> RawObjectId {
        format!("{:024x}", id).parse().unwrap()
    }
    fn creep(id: u8, x: u8, y: u8, hits: u32, parts: &[(Part, u32)]) -> CombatCreepDto {
        let b = body(parts);
        let hits_max = b.len() as u32 * 100;
        CombatCreepDto { id: Some(raw(id)), pos: pos(x, y), hits: hits.min(hits_max), hits_max, body: b }
    }
    fn structure(x: u8, y: u8, ty: StructureType, ownership: Ownership) -> CombatStructureDto {
        // Towers default to full energy (so tower tests see a firing/healing tower); 0 for the rest.
        let energy = if ty == StructureType::Tower { 1000 } else { 0 };
        CombatStructureDto { pos: pos(x, y), structure_type: ty, hits: 1000, hits_max: 1000, ownership, energy }
    }
    fn squad() -> SquadStateDto {
        // cohesion_radius 0 + Hold ⇒ the per-creep fallback path (the existing tests' behavior).
        SquadStateDto { center: pos(25, 25), room: "W1N1".parse().unwrap(), movement: SquadMovement::Hold, cohesion_radius: 0 }
    }

    struct Scene {
        squad: SquadStateDto,
        friends: Vec<CombatCreepDto>,
        hostiles: Vec<CombatCreepDto>,
        structures: Vec<CombatStructureDto>,
    }
    impl Scene {
        fn view<'a>(&'a self, me: &'a CombatCreepDto, orders: Option<CreepOrders>) -> CombatView<'a> {
            CombatView {
                tick: 1,
                me,
                squad: &self.squad,
                orders,
                friends: &self.friends,
                hostiles: &self.hostiles,
                structures: &self.structures,
            }
        }
    }

    // ── select_focus_target — EV target selection (ADR 0020 §4.2) ────────
    #[test]
    fn focus_ev_skips_an_out_healed_healer_for_a_killable_attacker() {
        // The bait: a LOW-HITS healer (the old unconditional "kill healer first" pick) that is
        // out-healed by an adjacent second healer (180 heal/tick > our 100 DPS → net 0, unkillable).
        // EV discards it and picks the exposed, killable attacker instead — no wasted fire.
        let c1 = creep(2, 30, 30, 200, &[(Part::Heal, 5)]); // low hits, but...
        let c2 = creep(3, 30, 31, 1000, &[(Part::Heal, 10)]); // ...adjacent → heals c1 120/tick (+self 60)
        let attacker = creep(1, 10, 10, 300, &[(Part::Attack, 3)]); // far, unhealed → killable
        let f = select_focus_target(&[c1, c2, attacker], &[], 100).unwrap();
        assert_eq!(f.id, Some(raw(1)), "skips the out-healed healer, kills the exposed attacker");
    }

    #[test]
    fn focus_ev_skips_a_rampart_shielded_target() {
        // A low-hits target on a hostile rampart is shielded (our fire redirects to the rampart) →
        // EV skips it for the unshielded one, even though the shielded one has fewer hits.
        let shielded = creep(1, 25, 25, 100, &[(Part::Attack, 1)]);
        let exposed = creep(2, 26, 25, 300, &[(Part::Attack, 1)]);
        let ramparts = vec![structure(25, 25, StructureType::Rampart, Ownership::Hostile)];
        let f = select_focus_target(&[shielded, exposed], &ramparts, 200).unwrap();
        assert_eq!(f.id, Some(raw(2)), "the rampart-shielded creep is unkillable by direct fire");
    }

    #[test]
    fn focus_ev_maximizes_threat_per_ttk_not_just_lowest_hits() {
        // A: 1 ATTACK, 100 hits → threat 30, ttk 1 (ev 30). B: 5 ATTACK, 300 hits → threat 150,
        // ttk 3 (ev 50). EV picks B (more enemy capability removed per tick) over the lower-hits A.
        let a = creep(1, 10, 10, 100, &[(Part::Attack, 1)]);
        let b = creep(2, 40, 40, 300, &[(Part::Attack, 5)]);
        let f = select_focus_target(&[a, b], &[], 100).unwrap();
        assert_eq!(f.id, Some(raw(2)), "threat/ttk beats raw lowest-hits");
    }

    #[test]
    fn assign_focus_fire_spills_overkill_to_the_next_target() {
        // Two ranged shooters (70 dps each → 140 total). Target X: high threat (5 ATTACK), 30 hits →
        // a 1-shot, top EV. Target Y: low threat, 700 hits. One shooter finishes X (budget 30 < 70);
        // the SECOND shooter spills to Y instead of wasting 110 dps overkilling X.
        let members = vec![ranged_member_at(700, 700, 25, 25), ranged_member_at(700, 700, 26, 25)];
        let x = creep(1, 30, 25, 30, &[(Part::Attack, 5)]);
        let y = creep(2, 35, 25, 700, &[(Part::Attack, 1), (Part::Move, 6)]);
        let a = assign_focus_fire(&members, &[x, y], &[]);
        assert_eq!(a[0].and_then(|f| f.id), Some(raw(1)), "first shooter finishes the high-EV 1-shot");
        assert_eq!(a[1].and_then(|f| f.id), Some(raw(2)), "the overkill shooter spills to the next target");
    }

    #[test]
    fn assign_focus_fire_keeps_all_on_one_big_target_and_none_without_offense() {
        let members = vec![ranged_member_at(700, 700, 25, 25), ranged_member_at(700, 700, 26, 25)];
        let big = creep(1, 30, 25, 700, &[(Part::Attack, 1), (Part::Move, 6)]); // budget 700 > 140 dps
        let a = assign_focus_fire(&members, &[big.clone()], &[]);
        assert_eq!(a[0].and_then(|f| f.id), Some(raw(1)));
        assert_eq!(a[1].and_then(|f| f.id), Some(raw(1)), "one big target soaks both shooters (no spill)");
        // No offense (healers only) ⇒ nothing killable ⇒ all None (fall back to the shared focus).
        let healers = vec![healer_at(600, 600, 5, 25, 25), healer_at(600, 600, 5, 26, 25)];
        assert!(assign_focus_fire(&healers, &[big], &[]).iter().all(|f| f.is_none()));
    }

    #[test]
    fn tower_heal_counts_toward_killability_only_when_energized() {
        // Two ranged shooters (140 dps). A 100-hit target beside a hostile tower. An ENERGIZED tower
        // heals ~400/tick → out-heals us → the target is unkillable (no shooter assigned). A DRAINED
        // tower (energy 0) can't heal → the target becomes killable. (Operator: count tower heal, but
        // only when the tower has >= TOWER_ENERGY_COST energy. Same gate applies to its damage.)
        let members = vec![ranged_member_at(700, 700, 20, 25), ranged_member_at(700, 700, 21, 25)];
        let target = creep(1, 25, 25, 100, &[(Part::Attack, 1), (Part::Move, 1)]);
        let tower_on = structure(25, 26, StructureType::Tower, Ownership::Hostile); // helper → energy 1000
        let on = assign_focus_fire(&members, &[target.clone()], &[tower_on.clone()]);
        assert!(on.iter().all(|f| f.is_none()), "energized tower out-heals → target unkillable");

        let tower_off = CombatStructureDto { energy: 0, ..tower_on };
        let off = assign_focus_fire(&members, &[target], &[tower_off]);
        assert_eq!(off[0].and_then(|f| f.id), Some(raw(1)), "a drained tower can't heal → target killable");
    }

    #[test]
    fn kill_budget_includes_same_tick_heal_so_no_premature_spill() {
        // The engine nets damage THEN heal THEN checks death (tick.js:120-136), so a creep dies this
        // tick only if damage >= hits + same-tick heal. X has 100 hits + 60 self-heal → budget 160.
        // Two shooters (120 + 40 = 160 dps) must BOTH stay on X (budget exactly met); if the budget
        // wrongly ignored heal (100), the 120-shooter would "finish" X and the 40 would spill to Y.
        let shooter = |rp: u32, x: u8| SquadMemberView { hits: 700, hits_max: 700, pos: Some(pos(x, 25)), has_ranged: true, ranged_power: rp, ..Default::default() };
        let members = vec![shooter(120, 25), shooter(40, 26)];
        let x = creep(1, 30, 25, 100, &[(Part::Heal, 5)]); // self-heals 60/tick at range 0 → budget 160
        let y = creep(2, 35, 25, 500, &[(Part::Attack, 5)]); // a spill target if X were under-budgeted
        let a = assign_focus_fire(&members, &[x, y], &[]);
        assert_eq!(a[0].and_then(|f| f.id), Some(raw(1)));
        assert_eq!(a[1].and_then(|f| f.id), Some(raw(1)), "both stay on X — heal is counted in its kill budget");
    }

    #[test]
    fn focus_ev_fallbacks() {
        // No offense (our_dps 0) ⇒ nothing is "killable" ⇒ best-effort lowest-hits unshielded creep.
        let weak = creep(1, 20, 20, 100, &[(Part::Attack, 1)]);
        let strong = creep(2, 40, 40, 400, &[(Part::Attack, 5)]);
        assert_eq!(select_focus_target(&[strong, weak.clone()], &[], 0).unwrap().id, Some(raw(1)));
        // No hostiles → InvaderCore beats spawn/tower; my/neutral excluded.
        let structs = vec![
            structure(10, 10, StructureType::Tower, Ownership::Hostile),
            structure(11, 11, StructureType::InvaderCore, Ownership::Hostile),
            structure(12, 12, StructureType::Spawn, Ownership::Mine),
        ];
        let t = select_focus_target(&[], &structs, 100).unwrap();
        assert_eq!((t.pos, t.id), (pos(11, 11), None));
        assert_eq!(select_focus_target(&[], &[], 100), None);
    }

    #[test]
    fn dead_heal_part_is_not_a_healer() {
        let mut faux = creep(1, 20, 20, 600, &[(Part::Heal, 1), (Part::Move, 5)]); // 600 hits
        faux.body[0].hits = 0; // its only HEAL part is destroyed → not a healer
        let weak = creep(2, 30, 30, 150, &[(Part::Attack, 5)]); // genuinely lower hits + real threat
        // EV: the dead HEAL part contributes no heal threat; the armed `weak` is the higher threat/ttk kill.
        assert_eq!(select_focus_target(&[faux, weak.clone()], &[], 100).unwrap().id, weak.id);
    }

    // ── decide_combat: ordered path ─────────────────────────────────────
    #[test]
    fn ranged_with_orders_focus_fires_the_designated_target() {
        let focus = creep(9, 26, 25, 300, &[(Part::Move, 3)]); // adjacent-ish, range 1
        let other = creep(8, 24, 25, 50, &[(Part::Move, 3)]); // weaker but not the focus
        let s = Scene {
            squad: squad(),
            friends: vec![],
            hostiles: vec![other, focus.clone()],
            structures: vec![],
        };
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        let orders = Some(CreepOrders { focus: Some(focus.as_target()), heal_target: None });
        let intents = decide_combat(&s.view(&me, orders));
        // Focus is within range 3 → RangedAttack the focus (not the weaker non-focus).
        assert_eq!(intents, vec![CombatIntent::RangedAttack { target: pos(26, 25), id: focus.id }]);
    }

    #[test]
    fn ranged_with_orders_mass_attacks_when_stacked() {
        // Three hostiles adjacent (range 1) → RMA, not single-target.
        let hostiles = vec![
            creep(5, 24, 25, 100, &[(Part::Move, 1)]),
            creep(6, 26, 25, 100, &[(Part::Move, 1)]),
            creep(7, 25, 24, 100, &[(Part::Move, 1)]),
        ];
        let s = Scene { squad: squad(), friends: vec![], hostiles, structures: vec![] };
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        let orders = Some(CreepOrders { focus: None, heal_target: None });
        assert_eq!(decide_combat(&s.view(&me, orders)), vec![CombatIntent::RangedMassAttack]);
    }

    #[test]
    fn melee_with_orders_prefers_adjacent_focus_then_emits_in_pipeline_order() {
        let focus = creep(9, 26, 25, 300, &[(Part::Move, 3)]);
        let s = Scene { squad: squad(), friends: vec![], hostiles: vec![focus.clone()], structures: vec![] };
        // A full-HP melee + ranged + heal creep: pipeline order A (Attack focus), then B
        // (RangedAttack focus); no heal (nothing wounded). Proves the emission order.
        let me = creep(1, 25, 25, 600, &[(Part::Attack, 2), (Part::RangedAttack, 2), (Part::Heal, 2)]);
        let orders = Some(CreepOrders { focus: Some(focus.as_target()), heal_target: None });
        let intents = decide_combat(&s.view(&me, orders));
        assert_eq!(
            intents,
            vec![
                CombatIntent::Attack { target: pos(26, 25), id: focus.id },
                CombatIntent::RangedAttack { target: pos(26, 25), id: focus.id },
            ]
        );
    }

    #[test]
    fn heal_with_orders_uses_assigned_target_by_range() {
        let wounded_adj = creep(2, 25, 26, 50, &[(Part::Move, 5)]); // range 1
        let me = creep(1, 25, 25, 600, &[(Part::Heal, 6)]);
        let s = Scene {
            squad: squad(),
            friends: vec![me.clone(), wounded_adj.clone()],
            hostiles: vec![],
            structures: vec![],
        };
        let orders = Some(CreepOrders { focus: None, heal_target: Some(wounded_adj.as_target()) });
        assert_eq!(
            decide_combat(&s.view(&me, orders)),
            vec![CombatIntent::Heal { target: pos(25, 26), id: wounded_adj.id }]
        );
    }

    // ── decide_combat: fallback path ────────────────────────────────────
    #[test]
    fn fallback_ranged_focuses_lowest_hits_in_range() {
        let s = Scene {
            squad: squad(),
            friends: vec![],
            hostiles: vec![creep(5, 27, 25, 400, &[(Part::Move, 4)]), creep(6, 26, 25, 90, &[(Part::Move, 1)])],
            structures: vec![],
        };
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        assert_eq!(
            decide_combat(&s.view(&me, None)),
            vec![CombatIntent::RangedAttack { target: pos(26, 25), id: Some(raw(6)) }]
        );
    }

    #[test]
    fn fallback_attacks_structures_when_no_hostiles() {
        let s = Scene {
            squad: squad(),
            friends: vec![],
            hostiles: vec![],
            structures: vec![structure(26, 25, StructureType::Spawn, Ownership::Hostile)],
        };
        let me = creep(1, 25, 25, 600, &[(Part::Attack, 6)]);
        assert_eq!(
            decide_combat(&s.view(&me, None)),
            vec![CombatIntent::Attack { target: pos(26, 25), id: None }]
        );
    }

    #[test]
    fn fallback_heal_prefers_adjacent_then_self_then_ranged() {
        let me = creep(1, 25, 25, 300, &[(Part::Heal, 6)]); // damaged self
        let adj = creep(2, 25, 26, 50, &[(Part::Move, 5)]); // adjacent, weaker
        let far = creep(3, 25, 28, 50, &[(Part::Move, 5)]); // range 3
        // Adjacent wounded ally (weaker than self) → heal the ally.
        let s = Scene {
            squad: squad(),
            friends: vec![me.clone(), adj.clone(), far.clone()],
            hostiles: vec![],
            structures: vec![],
        };
        assert_eq!(
            decide_combat(&s.view(&me, None)),
            vec![CombatIntent::Heal { target: pos(25, 26), id: adj.id }]
        );
        // No adjacent ally, self damaged → self-heal.
        let healthy_far = creep(3, 25, 28, 250, &[(Part::Move, 5)]); // damaged but range 3
        let s2 = Scene { squad: squad(), friends: vec![me.clone(), healthy_far.clone()], hostiles: vec![], structures: vec![] };
        assert_eq!(decide_combat(&s2.view(&me, None)), vec![CombatIntent::Heal { target: pos(25, 25), id: me.id }]);
        // Self full, a ranged ally damaged → ranged-heal it.
        let full_me = creep(1, 25, 25, 600, &[(Part::Heal, 6)]);
        let s3 = Scene { squad: squad(), friends: vec![full_me.clone(), healthy_far.clone()], hostiles: vec![], structures: vec![] };
        assert_eq!(
            decide_combat(&s3.view(&full_me, None)),
            vec![CombatIntent::RangedHeal { target: pos(25, 28), id: healthy_far.id }]
        );
    }

    // ── decide_movement (per-creep tactical movement) ───────────────────
    #[test]
    fn melee_closes_to_range_1() {
        let s = Scene { squad: squad(), friends: vec![], hostiles: vec![creep(9, 30, 25, 300, &[(Part::Move, 3)])], structures: vec![] };
        let me = creep(1, 25, 25, 600, &[(Part::Attack, 6)]);
        assert_eq!(
            decide_movement(&s.view(&me, None)),
            vec![CombatIntent::MoveTo { target: pos(30, 25), range: 1 }]
        );
    }

    #[test]
    fn melee_adjacent_holds() {
        let s = Scene { squad: squad(), friends: vec![], hostiles: vec![creep(9, 26, 25, 300, &[(Part::Move, 3)])], structures: vec![] };
        let me = creep(1, 25, 25, 600, &[(Part::Attack, 6)]);
        assert!(decide_movement(&s.view(&me, None)).is_empty(), "already adjacent → hold");
    }

    #[test]
    fn ranged_kiter_flees_an_adjacent_melee_threat() {
        // A melee-only hostile at range 2 → flee from it to keep out of melee.
        let chaser = creep(9, 27, 25, 600, &[(Part::Attack, 6), (Part::Move, 6)]);
        let s = Scene { squad: squad(), friends: vec![], hostiles: vec![chaser], structures: vec![] };
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        assert_eq!(
            decide_movement(&s.view(&me, None)),
            vec![CombatIntent::Flee { from: vec![pos(27, 25)], range: 3 }]
        );
    }

    #[test]
    fn ranged_closes_to_shooting_range_when_far() {
        // Target at range 5, no melee threat near → close to range 3.
        let s = Scene { squad: squad(), friends: vec![], hostiles: vec![creep(9, 30, 25, 600, &[(Part::RangedAttack, 6)])], structures: vec![] };
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        assert_eq!(
            decide_movement(&s.view(&me, None)),
            vec![CombatIntent::MoveTo { target: pos(30, 25), range: 3 }]
        );
    }

    #[test]
    fn ranged_at_shooting_range_holds() {
        // Target at range 3, no melee threat → hold (shoot in place).
        let s = Scene { squad: squad(), friends: vec![], hostiles: vec![creep(9, 28, 25, 600, &[(Part::RangedAttack, 6)])], structures: vec![] };
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        assert!(decide_movement(&s.view(&me, None)).is_empty(), "range 3 → hold and shoot");
    }

    #[test]
    fn healer_follows_the_nearest_wounded_ally() {
        let wounded = creep(2, 28, 25, 100, &[(Part::Move, 5)]); // range 3, damaged
        let s = Scene { squad: squad(), friends: vec![wounded.clone()], hostiles: vec![], structures: vec![] };
        let me = creep(1, 25, 25, 600, &[(Part::Heal, 6)]);
        assert_eq!(
            decide_movement(&s.view(&me, None)),
            vec![CombatIntent::MoveTo { target: pos(28, 25), range: 1 }]
        );
    }

    // ── decide_movement: the squad-goal precedence (managed squad, cohesion_radius > 0) ──
    fn managed_squad(movement: SquadMovement) -> SquadStateDto {
        SquadStateDto { center: pos(25, 25), room: "W1N1".parse().unwrap(), movement, cohesion_radius: 2 }
    }

    #[test]
    fn decide_movement_follows_the_squad_kite_goal() {
        let goal = pos(28, 25);
        let s = Scene {
            squad: managed_squad(SquadMovement::Kite { goal }),
            friends: vec![],
            hostiles: vec![creep(9, 32, 25, 600, &[(Part::RangedAttack, 6)])], // ranged, no melee-evade
            structures: vec![],
        };
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        assert_eq!(decide_movement(&s.view(&me, None)), vec![CombatIntent::MoveTo { target: goal, range: 0 }]);
    }

    #[test]
    fn decide_movement_rejoins_a_strayed_member() {
        let s = Scene {
            squad: managed_squad(SquadMovement::Hold),
            friends: vec![],
            hostiles: vec![],
            structures: vec![],
        };
        // Range 5 from the centroid (> K=2), nothing to fight → regroup to the squad.
        let me = creep(1, 30, 25, 700, &[(Part::RangedAttack, 7)]);
        assert_eq!(decide_movement(&s.view(&me, None)), vec![CombatIntent::MoveTo { target: pos(25, 25), range: 2 }]);
    }

    #[test]
    fn decide_movement_critical_hp_breaks_cohesion_to_flee() {
        let s = Scene {
            squad: managed_squad(SquadMovement::Advance { goal: pos(20, 25), range: 1 }),
            friends: vec![],
            hostiles: vec![creep(9, 26, 25, 600, &[(Part::Attack, 6)])], // adjacent melee
            structures: vec![],
        };
        // 100/700 ≈ 14% < CRITICAL_HP_FRACTION → flee individually, ignoring the Advance directive.
        let me = creep(1, 25, 25, 100, &[(Part::RangedAttack, 7)]);
        match &decide_movement(&s.view(&me, None))[..] {
            [CombatIntent::Flee { from, range }] => {
                assert_eq!(*range, 3);
                assert!(from.contains(&pos(26, 25)), "flees the nearby threat");
            }
            other => panic!("expected a critical-HP Flee that overrides the squad goal, got {other:?}"),
        }
    }

    // ── decide_squad (squad-level focus + engage/retreat hysteresis) ────
    fn member(hits: u32, hits_max: u32, heal_power: u32) -> SquadMemberView {
        SquadMemberView { hits, hits_max, heal_power, ..Default::default() }
    }
    fn ranged_member_at(hits: u32, hits_max: u32, x: u8, y: u8) -> SquadMemberView {
        // 7 RANGED_ATTACK parts (×10) — a real ranged body, so the engage DMG reward (focus_damage) is
        // exercised, not collapsed to 0.
        SquadMemberView { hits, hits_max, heal_power: 0, pos: Some(pos(x, y)), has_ranged: true, ranged_power: 70, ..Default::default() }
    }
    fn healer_at(hits: u32, hits_max: u32, heal_power: u32, x: u8, y: u8) -> SquadMemberView {
        SquadMemberView { hits, hits_max, heal_power, pos: Some(pos(x, y)), has_ranged: false, melee_power: 0, ranged_power: 0, damage_taken_last_tick: 0 }
    }
    fn squad_view<'a>(
        members: &'a [SquadMemberView],
        hostiles: &'a [CombatCreepDto],
        current_state: SquadOrderState,
    ) -> SquadView<'a> {
        SquadView { members, hostiles, structures: &[], retreat_threshold: 0.3, current_state, enemy_safe_mode: false }
    }

    #[test]
    fn squad_engages_when_a_target_exists_else_moves() {
        let hostiles = vec![creep(1, 25, 25, 100, &[(Part::Attack, 1)])];
        // A combat-capable squad (70 ranged DPS) vs a weak 30-DPS target → EV-winnable → Engaged.
        let members = vec![ranged_member_at(600, 600, 25, 26)];
        let d = decide_squad(&squad_view(&members, &hostiles, SquadOrderState::Moving));
        assert_eq!(d.state, SquadOrderState::Engaged);
        assert!(d.focus.is_some());

        let d2 = decide_squad(&squad_view(&members, &[], SquadOrderState::Moving));
        assert_eq!(d2.state, SquadOrderState::Moving);
        assert!(d2.focus.is_none());
    }

    #[test]
    fn squad_retreats_on_low_avg_or_critical_member() {
        let hostiles = vec![creep(1, 25, 25, 100, &[(Part::Attack, 1)])];
        // avg HP 0.2 < 0.3 threshold → retreat.
        let low = vec![member(120, 600, 0)];
        assert_eq!(decide_squad(&squad_view(&low, &hostiles, SquadOrderState::Engaged)).state, SquadOrderState::Retreating);
        // avg fine (~0.58) but one member critical (<25%) → retreat.
        let mixed = vec![member(600, 600, 0), member(100, 600, 0)];
        assert_eq!(decide_squad(&squad_view(&mixed, &hostiles, SquadOrderState::Engaged)).state, SquadOrderState::Retreating);
    }

    #[test]
    fn squad_retreat_hysteresis_has_no_yo_yo() {
        // A combat-winnable matchup (70 DPS vs a 30-DPS target) so μ engages — the HP hysteresis is
        // what's under test here: retreat stays sticky until HP recovers past the re-engage band.
        let hostiles = vec![creep(1, 25, 25, 100, &[(Part::Attack, 1)])];
        // Recovered to 0.5 — above the 0.3 threshold but below the re-engage band (0.6) → stay retreating.
        let mid = vec![ranged_member_at(300, 600, 25, 26)];
        assert_eq!(decide_squad(&squad_view(&mid, &hostiles, SquadOrderState::Retreating)).state, SquadOrderState::Retreating);
        // Recovered above the band (0.7 > 0.6) → re-engage.
        let high = vec![ranged_member_at(420, 600, 25, 26)];
        assert_eq!(decide_squad(&squad_view(&high, &hostiles, SquadOrderState::Retreating)).state, SquadOrderState::Engaged);
    }

    #[test]
    fn lanchester_retreats_from_an_unwinnable_fight_even_at_full_hp() {
        // One ranged creep (70 DPS, FULL HP) vs four 150-DPS bruisers → the Lanchester balance is
        // hugely negative → retreat. The OLD flat-HP rule would have engaged (we're at 100% HP) — this
        // is the core fix for "wiped in war": commit on winnability, not just health.
        let members = vec![ranged_member_at(700, 700, 25, 25)];
        let enemy: Vec<_> = (0..4).map(|i| creep(10 + i, 30, 25 + i as u8, 1000, &[(Part::Attack, 5), (Part::Move, 5)])).collect();
        assert_eq!(decide_squad(&squad_view(&members, &enemy, SquadOrderState::Engaged)).state, SquadOrderState::Retreating);
    }

    #[test]
    fn tank_and_heal_sieges_a_tower_base_instead_of_retreating() {
        // Operator: retreat is about LOSS. A siege squad that OUT-HEALS the tower fire takes no net
        // loss → it should engage + dismantle the base (a win), not retreat; below the heal sustain it
        // bleeds → retreat. A tower at range 20 from the squad deals 150/tick.
        let tank = SquadMemberView { hits: 2000, hits_max: 2000, pos: Some(pos(5, 25)), ..Default::default() };
        let healer = |parts: u32| SquadMemberView { hits: 2000, hits_max: 2000, heal_power: parts, pos: Some(pos(5, 26)), ..Default::default() };
        let base = vec![
            structure(25, 25, StructureType::Tower, Ownership::Hostile), // energy 1000 (helper) → fires
            structure(26, 25, StructureType::Spawn, Ownership::Hostile),
        ];
        let mk = |members: &[SquadMemberView], st| {
            decide_squad(&SquadView { members, hostiles: &[], structures: &base, retreat_threshold: 0.3, current_state: st, enemy_safe_mode: false }).state
        };
        // 14 HEAL ×12 = 168 > 150 tower → sustained, no loss → siege the base.
        assert_eq!(mk(&[tank, healer(14)], SquadOrderState::Engaged), SquadOrderState::Engaged, "out-heal the towers → dismantle, don't retreat");
        // 12 HEAL ×12 = 144 < 150 → bleeding → retreat (a loss).
        assert_eq!(mk(&[tank, healer(12)], SquadOrderState::Engaged), SquadOrderState::Retreating, "can't out-heal the towers → retreat");
    }

    #[test]
    fn safe_mode_vetoes_engagement() {
        // A trivially-winnable matchup, but the enemy room is in safe mode → our combat is nullified →
        // never engage (ADR 0020 §8 engage-veto).
        let members = vec![ranged_member_at(700, 700, 25, 25)];
        let hostiles = vec![creep(1, 26, 25, 100, &[(Part::Attack, 1)])];
        let view = SquadView { members: &members, hostiles: &hostiles, structures: &[], retreat_threshold: 0.3, current_state: SquadOrderState::Engaged, enemy_safe_mode: true };
        assert_eq!(decide_squad(&view).state, SquadOrderState::Retreating, "safe mode nullifies our combat → never engage");
    }

    // ── decide_squad movement directive + decide_squad_with_pathing ─────
    #[test]
    fn decide_squad_no_pathing_advances_to_weapon_range() {
        // A ranged squad with a target → Engaged, Advance to shooting range 3 (no kite search).
        let members = vec![ranged_member_at(700, 700, 25, 25)];
        let hostiles = vec![creep(9, 30, 25, 600, &[(Part::RangedAttack, 6)])];
        let view = SquadView {
            members: &members,
            hostiles: &hostiles,
            structures: &[],
            retreat_threshold: 0.3,
            current_state: SquadOrderState::Moving,
            enemy_safe_mode: false,
        };
        let d = decide_squad(&view);
        assert_eq!(d.state, SquadOrderState::Engaged);
        match d.movement {
            SquadMovement::Advance { range, .. } => assert_eq!(range, 3, "ranged squad advances to shooting range"),
            other => panic!("expected Advance, got {other:?}"),
        }
        assert!(d.center.is_some(), "centroid from member positions");
    }

    #[test]
    fn engaged_ranged_squad_uses_scored_engage_positioning() {
        // A ranged squad engaged with a ranged-creep focus, no melee threat near → NOT kiting → the
        // ENGAGE branch (ADR 0019 Stage 3b) runs the scored search with engage weights and produces a
        // positioning Advance{range:0} (move onto the chosen in-weapon-range tile), NOT the naive
        // Advance{range:3} straight at the focus nor a Kite. The flee↔stand split is the weight preset.
        let members = vec![ranged_member_at(700, 700, 25, 25)];
        let hostiles = vec![creep(9, 30, 25, 600, &[(Part::RangedAttack, 6)])]; // ranged-only: no melee → no kite
        let view = squad_view(&members, &hostiles, SquadOrderState::Engaged);
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let d = decide_squad_with_pathing(&view, None, kite::SquadTacticParams::default(), &mut cb, kite::MAX_KITE_OPS);
        assert_eq!(d.state, SquadOrderState::Engaged);
        match d.movement {
            SquadMovement::Advance { goal, range } => {
                assert_eq!(range, 0, "engage positioning moves onto the scored tile");
                assert!(goal.get_range_to(pos(30, 25)) <= 3, "the scored tile is in weapon range of the focus: {goal:?}");
            }
            SquadMovement::Hold => {} // acceptable if the centroid is already the optimal fighting tile
            other => panic!("expected an engage Advance{{range:0}} or Hold, got {other:?}"),
        }
    }

    #[test]
    fn engaged_ranged_squad_advances_toward_a_far_focus() {
        // ADR 0019 advance-to-damage layer: a ranged squad engaged with a focus BEYOND the bounded
        // search horizon must still march toward it (the dominant euclidean proximity term makes the
        // flood's best-effort tile the one closest to the focus) — NOT Hold short. The chosen goal is
        // strictly closer to the focus than the centroid (progress), with no special "approach" branch.
        let members = vec![ranged_member_at(700, 700, 10, 25)];
        let hostiles = vec![creep(9, 40, 25, 600, &[(Part::RangedAttack, 6)])]; // far focus (range 30)
        let view = squad_view(&members, &hostiles, SquadOrderState::Engaged);
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let d = decide_squad_with_pathing(&view, None, kite::SquadTacticParams::default(), &mut cb, kite::MAX_KITE_OPS);
        match d.movement {
            SquadMovement::Advance { goal, .. } => {
                assert!(
                    goal.get_range_to(pos(40, 25)) < pos(10, 25).get_range_to(pos(40, 25)),
                    "the goal advances toward the far focus: {goal:?}"
                );
            }
            other => panic!("expected an Advance toward the far focus, got {other:?}"),
        }
    }

    #[test]
    fn squad_orients_the_formation_toward_the_focus_when_engaged() {
        // Squad centroid (25,25); a hostile to the east at (30,25) → the block faces Right (O2).
        let members = vec![ranged_member_at(700, 700, 25, 25)];
        let hostiles = vec![creep(9, 30, 25, 600, &[(Part::RangedAttack, 6)])];
        let d = decide_squad(&squad_view(&members, &hostiles, SquadOrderState::Engaged));
        assert_eq!(d.state, SquadOrderState::Engaged);
        assert_eq!(d.orientation, Some(Direction::Right), "faces the threat to the east");

        // A threat to the north (smaller y) → faces Top.
        let north = vec![creep(9, 25, 20, 600, &[(Part::RangedAttack, 6)])];
        let dn = decide_squad(&squad_view(&members, &north, SquadOrderState::Engaged));
        assert_eq!(dn.orientation, Some(Direction::Top), "faces the threat to the north");

        // No focus (no hostiles) → Moving, no orientation to apply.
        let d2 = decide_squad(&squad_view(&members, &[], SquadOrderState::Moving));
        assert_eq!(d2.state, SquadOrderState::Moving);
        assert_eq!(d2.orientation, None);
    }

    #[test]
    fn breach_redirect_targets_the_rampart_shielding_the_structure() {
        // A terrain-wall column at x=8 with one gap at (8,25) plugged by a dismantlable hostile
        // rampart; the only corridor from the squad (5,25) to the spawn (10,25) crosses it. O3
        // redirects the focus to that rampart — break the breach before the target it shields.
        let xy = |x: u8, y: u8| RoomXY::checked_new(x, y).unwrap();
        let mut cm = LocalCostMatrix::new();
        for y in 0..50u8 {
            if y != 25 {
                cm.set(xy(8, y), u8::MAX);
            }
        }
        let mut cb = move |_r| Some(cm.clone());

        let structures = vec![
            CombatStructureDto { pos: pos(8, 25), structure_type: StructureType::Rampart, hits: 100, hits_max: 100, ownership: Ownership::Hostile, energy: 0 },
            CombatStructureDto { pos: pos(10, 25), structure_type: StructureType::Spawn, hits: 5000, hits_max: 5000, ownership: Ownership::Hostile, energy: 0 },
        ];
        let members = vec![SquadMemberView {
            hits: 1000,
            hits_max: 1000,
            heal_power: 0,
            pos: Some(pos(5, 25)),
            has_ranged: false,
            melee_power: 0,
            ranged_power: 0,
            damage_taken_last_tick: 0,
        }];
        let view = SquadView {
            members: &members,
            hostiles: &[],
            structures: &structures,
            retreat_threshold: 0.3,
            current_state: SquadOrderState::Moving,
            enemy_safe_mode: false,
        };

        let d = decide_squad_with_pathing(&view, None, kite::SquadTacticParams::default(), &mut cb, kite::MAX_KITE_OPS);
        assert_eq!(d.focus.map(|f| f.pos), Some(pos(8, 25)), "focus the shielding rampart, not the spawn behind it");
        match d.movement {
            SquadMovement::Advance { goal, .. } => assert_eq!(goal, pos(8, 25), "advance toward the breach"),
            other => panic!("expected Advance to the breach, got {other:?}"),
        }
    }

    #[test]
    fn decide_squad_with_pathing_kites_a_melee_threat_but_stays_in_range() {
        // Ranged duo, a melee threat adjacent to the centroid → Kite to a tile clear of the melee
        // reach (>2) but the value term keeps the focus shootable (the goal is near, not fled to ∞).
        let members = vec![ranged_member_at(700, 700, 25, 25), ranged_member_at(700, 700, 26, 25)];
        let hostiles = vec![creep(9, 24, 25, 600, &[(Part::Attack, 6), (Part::Move, 6)])];
        let view = SquadView {
            members: &members,
            hostiles: &hostiles,
            structures: &[],
            retreat_threshold: 0.3,
            current_state: SquadOrderState::Engaged,
            enemy_safe_mode: false,
        };
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let d = decide_squad_with_pathing(&view, None, kite::SquadTacticParams::default(), &mut cb, kite::MAX_KITE_OPS);
        assert_eq!(d.state, SquadOrderState::Engaged);
        match d.movement {
            SquadMovement::Kite { goal } => {
                assert!(goal.get_range_to(pos(24, 25)) > 2, "kite goal clears the melee reach: {goal:?}");
                assert!(goal.get_range_to(pos(24, 25)) <= 4, "but stays near the threat (focus shootable): {goal:?}");
            }
            other => panic!("expected a Kite directive vs a melee threat, got {other:?}"),
        }
    }

    #[test]
    fn threat_step_ticks_models_fatigue_cadence_and_filters_immobile() {
        // ADR 0019 Guard 5 (seed filter) speed model: only mobile chasers seed the reachability flood,
        // at their plain-terrain fatigue cadence ceil(weight/move).
        // Balanced 1:1 body → moves every tick (cadence 1).
        assert_eq!(threat_step_ticks(&creep(1, 25, 25, 500, &[(Part::Attack, 5), (Part::Move, 5)])), Some(1));
        // Under-MOVE (weight 10, move 5) → 2 ticks/step.
        assert_eq!(threat_step_ticks(&creep(1, 25, 25, 500, &[(Part::Attack, 10), (Part::Move, 5)])), Some(2));
        // Over-MOVE (weight 3, move 6) → still 1 (floored).
        assert_eq!(threat_step_ticks(&creep(1, 25, 25, 500, &[(Part::Attack, 3), (Part::Move, 6)])), Some(1));
        // No working MOVE → immobile → None (seeds NO reachability wave; still a present threat).
        assert_eq!(threat_step_ticks(&creep(1, 25, 25, 500, &[(Part::Attack, 5)])), None);
    }

    #[test]
    fn scored_squad_plan_is_deterministic() {
        // ADR 0019 must-fix #6 (deterministic argmax): the scored search has no RNG and a total tile
        // order, so identical input yields an identical goal across runs (no tick-to-tick jitter from
        // arbitrary tie resolution).
        let members = vec![ranged_member_at(700, 700, 25, 25), ranged_member_at(700, 700, 26, 25)];
        let hostiles = vec![creep(9, 24, 25, 600, &[(Part::Attack, 6), (Part::Move, 6)])];
        let view = SquadView { members: &members, hostiles: &hostiles, structures: &[], retreat_threshold: 0.3, current_state: SquadOrderState::Engaged, enemy_safe_mode: false };
        let mut cb1 = |_r| Some(LocalCostMatrix::new());
        let mut cb2 = |_r| Some(LocalCostMatrix::new());
        let a = decide_squad_with_pathing(&view, None, kite::SquadTacticParams::default(), &mut cb1, kite::MAX_KITE_OPS);
        let b = decide_squad_with_pathing(&view, None, kite::SquadTacticParams::default(), &mut cb2, kite::MAX_KITE_OPS);
        assert_eq!(a.movement, b.movement, "same input → same goal (deterministic)");
    }

    #[test]
    fn assign_heals_gives_the_most_wounded_the_best_in_range_healer() {
        // Member 0 = a wounded ranged attacker (range 1 from the healer); member 1 = a full healer.
        let members = vec![
            ranged_member_at(100, 700, 25, 25),
            healer_at(600, 600, 5, 26, 25),
        ];
        let d = decide_squad(&SquadView {
            members: &members,
            hostiles: &[creep(9, 24, 25, 600, &[(Part::Attack, 6)])],
            structures: &[],
            retreat_threshold: 0.3,
            current_state: SquadOrderState::Engaged,
            enemy_safe_mode: false,
        });
        // The healer (idx 1) is assigned to the wounded attacker (idx 0), adjacent → 12/part.
        assert_eq!(d.heal_assignments.len(), 1);
        let h = d.heal_assignments[0];
        assert_eq!(h.healer_idx, 1);
        assert_eq!(h.target_idx, 0);
        assert_eq!(h.expected_heal, 5 * 12, "adjacent heal = 12/part");
    }

    #[test]
    fn assign_heals_empty_when_no_healers_or_no_wounded() {
        // No healers.
        let m1 = vec![ranged_member_at(100, 700, 25, 25)];
        assert!(decide_squad(&SquadView { members: &m1, hostiles: &[], structures: &[], retreat_threshold: 0.3, current_state: SquadOrderState::Moving, enemy_safe_mode: false }).heal_assignments.is_empty());
        // A healer but everyone full + no damage taken.
        let m2 = vec![ranged_member_at(700, 700, 25, 25), healer_at(600, 600, 5, 26, 25)];
        assert!(decide_squad(&SquadView { members: &m2, hostiles: &[], structures: &[], retreat_threshold: 0.3, current_state: SquadOrderState::Moving, enemy_safe_mode: false }).heal_assignments.is_empty());
    }

    #[test]
    fn decide_squad_with_pathing_holds_when_no_engageable_target() {
        let members = vec![ranged_member_at(700, 700, 25, 25)];
        let view = SquadView {
            members: &members,
            hostiles: &[],
            structures: &[],
            retreat_threshold: 0.3,
            current_state: SquadOrderState::Moving,
            enemy_safe_mode: false,
        };
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let d = decide_squad_with_pathing(&view, None, kite::SquadTacticParams::default(), &mut cb, kite::MAX_KITE_OPS);
        assert_eq!(d.state, SquadOrderState::Moving);
        assert_eq!(d.movement, SquadMovement::Hold);
    }

    // ── U8: edge-aware kiting (don't flee into the corner) ──

    #[test]
    fn kite_repulsors_are_just_the_threat_in_open_space() {
        // A kiter in the room interior (far from every edge) gets no synthetic edge repulsors —
        // byte-identical to the prior flee behavior.
        let threats = vec![pos(24, 25)];
        assert_eq!(kite_repulsors(pos(25, 25), &threats), threats, "open space ⇒ threats only");
    }

    #[test]
    fn kite_repulsors_add_the_near_edges() {
        // Near the SE corner (within EDGE_AVOID_THRESHOLD of both x=49 and y=49): the right and
        // bottom edges are added as repulsors so fleeing pushes back toward the interior.
        let threats = vec![pos(44, 44)];
        let r = kite_repulsors(pos(46, 46), &threats);
        assert!(r.contains(&pos(49, 46)), "right edge at the kiter's y");
        assert!(r.contains(&pos(46, 49)), "bottom edge at the kiter's x");
        assert!(!r.contains(&pos(0, 46)) && !r.contains(&pos(46, 0)), "far edges are not added");
        assert_eq!(r.len(), 3, "threat + 2 near edges");
    }

    #[test]
    fn cornered_kiter_flees_the_edges_too() {
        // A ranged kiter pinned near the SE corner with a melee-only threat at range 2: the emitted
        // Flee carries the edge repulsors, so the rover flee rounds the corner instead of jamming
        // into it (the U8 fix). In open space the same situation would flee the threat alone.
        let me = creep(1, 47, 47, 700, &[(Part::RangedAttack, 5), (Part::Move, 2)]);
        let threat = creep(2, 45, 45, 1500, &[(Part::Attack, 10), (Part::Move, 5)]);
        let scene = Scene { squad: squad(), friends: vec![me.clone()], hostiles: vec![threat], structures: vec![] };
        let intents = decide_movement(&scene.view(&me, Some(CreepOrders::default())));
        match intents.as_slice() {
            [CombatIntent::Flee { from, .. }] => {
                assert!(from.contains(&pos(49, 47)) && from.contains(&pos(47, 49)), "fleeing the near edges too: {from:?}");
            }
            other => panic!("expected an edge-aware Flee, got {other:?}"),
        }
    }

    // ── U8-2: a pure support creep evades melee instead of standing ──

    #[test]
    fn solo_healer_evades_a_closing_melee_threat() {
        // A pure healer (no attack/ranged) with a melee-capable hostile at range 2 flees rather than
        // walking up to heal and getting cut down (the U8-2 bug: it used to just stand/follow).
        let me = creep(1, 25, 25, 500, &[(Part::Heal, 5), (Part::Move, 5)]);
        let chaser = creep(2, 25, 27, 1000, &[(Part::Attack, 7), (Part::Move, 7)]);
        let scene = Scene { squad: squad(), friends: vec![me.clone()], hostiles: vec![chaser], structures: vec![] };
        let intents = decide_movement(&scene.view(&me, Some(CreepOrders::default())));
        assert!(matches!(intents.as_slice(), [CombatIntent::Flee { .. }]), "healer flees the melee threat: {intents:?}");
    }

    // ── U8-3: solo focus prioritizes the enemy healer ──

    #[test]
    fn solo_fallback_focuses_the_healer_not_the_weakling() {
        // orders=None (no squad focus): a ranged creep targets the hostile HEALER (break the enemy's
        // sustain) over a lower-hits non-healer in range — matching the coordinated path's priority.
        let me = creep(1, 25, 25, 700, &[(Part::RangedAttack, 7)]);
        let weakling = creep(2, 24, 25, 50, &[(Part::Move, 1)]); // lower hits, in range 1
        let healer = creep(3, 26, 25, 500, &[(Part::Heal, 5)]); // a healer, more hits, in range 1
        let scene = Scene { squad: squad(), friends: vec![me.clone()], hostiles: vec![weakling, healer], structures: vec![] };
        let intents = decide_combat(&scene.view(&me, None));
        assert!(
            intents.contains(&CombatIntent::RangedAttack { target: pos(26, 25), id: Some(raw(3)) }),
            "solo creep focuses the healer (id 3): {intents:?}"
        );
    }
}
