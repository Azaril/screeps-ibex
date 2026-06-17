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

use screeps::{Part, Position, RawObjectId, StructureType};

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
}

/// Squad-level state the decision reads. Grows over H2/M2 (orientation, mode, retreat threshold);
/// the decisions extracted so far need only the shared centroid + the operating room.
#[derive(Clone, Debug, PartialEq)]
pub struct SquadStateDto {
    /// The squad's shared coordinate frame — its centroid / virtual anchor.
    pub center: Position,
    /// The room the squad is fighting in (target selection is gated to the visible room).
    pub room: screeps::RoomName,
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

/// The squad's shared focus target (ADR 0006 Inc B). A faithful port of
/// `attack_mission::compute_focus_target` — kept byte-identical in output so the live shim passes
/// intent parity:
/// 1. hostile with a working HEAL part, lowest hits (kill healers first to deny regen);
/// 2. else the lowest-hits hostile (focus fire for kills);
/// 3. else a hostile structure, prioritized InvaderCore > Spawn > Tower > other;
/// 4. else `None`.
///
/// `min_by_key` returns the *first* of equal minimums, matching the prior tie-break given the
/// adapter preserves hostile/structure ordering.
pub fn select_focus_target(hostiles: &[CombatCreepDto], structures: &[CombatStructureDto]) -> Option<FocusTarget> {
    if !hostiles.is_empty() {
        // Priority 1: hostiles with a working HEAL part, lowest hits.
        if let Some(t) = hostiles.iter().filter(|c| c.has_working(Part::Heal)).min_by_key(|c| c.hits) {
            return Some(t.as_target());
        }
        // Priority 2: lowest-hits hostile (always succeeds on a non-empty list).
        if let Some(t) = hostiles.iter().min_by_key(|c| c.hits) {
            return Some(t.as_target());
        }
    }
    // Priority 3: hostile structures (position-only; structures don't move).
    structures
        .iter()
        .filter(|s| s.ownership == Ownership::Hostile)
        .min_by_key(|s| structure_rank(s.structure_type))
        .map(|s| FocusTarget { pos: s.pos, id: None })
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

    // Pipeline A: melee the lowest-hits adjacent hostile.
    if me.has_working(Part::Attack) {
        if let Some(t) = min_hits_hostile_within(view, 1) {
            out.push(CombatIntent::Attack { target: t.pos, id: t.id });
        }
    }
    // Pipeline B: mass-attack when ≥3 adjacent, else focus the lowest-hits hostile in range 3.
    if me.has_working(Part::RangedAttack) {
        let in_range_1 = view.hostiles.iter().filter(|c| me.pos.get_range_to(c.pos) <= 1).count();
        if in_range_1 >= 3 {
            out.push(CombatIntent::RangedMassAttack);
        } else if let Some(t) = min_hits_hostile_within(view, 3) {
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
pub fn decide_movement(view: &CombatView) -> Vec<CombatIntent> {
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
            Some(CombatIntent::Flee { from: melee_threats, range: 3 })
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
        // Pure healer: follow the nearest damaged ally (excluding self) to range 1.
        view.friends
            .iter()
            .filter(|c| c.is_damaged() && c.pos != me.pos)
            .min_by_key(|c| me.pos.get_range_to(c.pos))
            .filter(|c| me.pos.get_range_to(c.pos) > 1)
            .map(|c| CombatIntent::MoveTo { target: c.pos, range: 1 })
    } else {
        None
    };
    mv.into_iter().collect()
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
        CombatStructureDto { pos: pos(x, y), structure_type: ty, hits: 1000, hits_max: 1000, ownership }
    }
    fn squad() -> SquadStateDto {
        SquadStateDto { center: pos(25, 25), room: "W1N1".parse().unwrap() }
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

    // ── select_focus_target ─────────────────────────────────────────────
    #[test]
    fn focus_prefers_healer_then_lowest_hits_then_structures() {
        let healer = creep(2, 30, 30, 500, &[(Part::Heal, 5)]);
        let weak = creep(1, 20, 20, 100, &[(Part::Attack, 1)]);
        assert_eq!(select_focus_target(&[weak.clone(), healer.clone()], &[]).unwrap().id, healer.id);
        // No healer → lowest hits.
        let strong = creep(3, 40, 40, 400, &[(Part::Attack, 5)]);
        assert_eq!(select_focus_target(&[strong, weak.clone()], &[]).unwrap().id, weak.id);
        // No hostiles → InvaderCore beats spawn/tower; my/neutral excluded.
        let structs = vec![
            structure(10, 10, StructureType::Tower, Ownership::Hostile),
            structure(11, 11, StructureType::InvaderCore, Ownership::Hostile),
            structure(12, 12, StructureType::Spawn, Ownership::Mine),
        ];
        let t = select_focus_target(&[], &structs).unwrap();
        assert_eq!((t.pos, t.id), (pos(11, 11), None));
        assert_eq!(select_focus_target(&[], &[]), None);
    }

    #[test]
    fn dead_heal_part_is_not_a_healer() {
        let mut faux = creep(1, 20, 20, 600, &[(Part::Heal, 1), (Part::Move, 5)]); // 600 hits
        faux.body[0].hits = 0; // its only HEAL part is destroyed → not a healer
        let weak = creep(2, 30, 30, 150, &[(Part::Attack, 5)]); // genuinely lower hits
        assert_eq!(select_focus_target(&[faux, weak.clone()], &[]).unwrap().id, weak.id);
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
}
