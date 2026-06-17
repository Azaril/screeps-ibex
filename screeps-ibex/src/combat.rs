//! The tactical seam (ADR [0006](../docs/design/0006-eval-and-iteration-harness.md) §B.2, seam
//! **S17** / [ADR 0015](../docs/design/0015-testing-and-validation-strategy.md)).
//!
//! This is the JS-free boundary that lets the bot's **real** combat decision code run inside the
//! in-process combat micro-sim (`screeps-combat-engine`) with no fork: the decision reads a
//! [`CombatView`] (value-type DTOs, **no `game::*` below this seam**) and emits [`CombatIntent`]s.
//! Two adapters build the view — a **live** one over `game::*` (the thin per-tick shim in
//! `missions::attack_mission`, isolated like the `screeps-rover` `screeps_impl.rs`) and a **sim**
//! one over `CombatWorld` (in `screeps-combat-agent`). There is then exactly one implementation of
//! target-selection / formation / kite / focus-fire, so self-play is `IbexAgent` vs `IbexAgent`
//! with no tactics drift (ADR 0006 §B.2).
//!
//! **No cargo feature** (operator preference 2026-06-16): the decision functions are pure and
//! `screeps-ibex` host-links, so the sim crate depends on the bot at the host target and calls them
//! directly. This module is the crate's only intentionally-`pub` surface beyond the wasm exports.
//!
//! Extraction is **parity-first**: the live shim must emit byte-identical intents to the prior
//! inline logic (the `intents::IntentRecorder` digest) before any sim result is trusted. The first
//! decision extracted here is [`select_focus_target`] (was `attack_mission::compute_focus_target`).

use screeps::{Part, Position, RawObjectId, RoomName, StructureType};

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
    /// The game object id when one exists (live creeps; `None` for sim-only creeps).
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
    /// Whether the creep has at least one working part of `part`.
    pub fn has_working(&self, part: Part) -> bool {
        self.body.iter().any(|p| p.part == part && p.hits > 0)
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

/// Squad-level state the decision reads. Grows over H2 (orientation, mode, retreat threshold);
/// the first decision needs only the shared centroid + the room it is operating in.
#[derive(Clone, Debug, PartialEq)]
pub struct SquadStateDto {
    /// The squad's shared coordinate frame — its centroid / virtual anchor.
    pub center: Position,
    /// The room the squad is fighting in (target selection is gated to the visible room).
    pub room: RoomName,
}

/// The **read seam**: a `CombatWorld` slice from one side's perspective, JS-free. Borrows the DTO
/// backing storage (built once per tick by an adapter) so it is allocation-light to pass around.
pub struct CombatView<'a> {
    pub tick: u32,
    pub squad: &'a SquadStateDto,
    /// Friendly creeps (the squad + allies in view).
    pub friends: &'a [CombatCreepDto],
    /// Hostile creeps in view, in a stable order (the live adapter preserves `creep_data.hostile()`
    /// order, so tie-breaks match the prior inline logic).
    pub hostiles: &'a [CombatCreepDto],
    pub structures: &'a [CombatStructureDto],
}

/// A chosen focus target: a position (sufficient for structures, which do not move) plus the
/// object id when the target is a creep (so the live executor can re-resolve a moving creep).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusTarget {
    pub pos: Position,
    pub id: Option<RawObjectId>,
}

/// The **write seam**: mirrors the guarded intents in `intents.rs` (one per engine pipeline) plus
/// the movement intents the rover system executes. Emitted by a [`TacticalAgent`]; translated back
/// into the guarded sink by the live executor, or applied to `CombatWorld` by the sim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CombatIntent {
    Attack(RawObjectId),
    RangedAttack(RawObjectId),
    RangedMassAttack,
    Heal(RawObjectId),
    RangedHeal(RawObjectId),
    Dismantle(RawObjectId),
    /// Move toward `target`, stopping within `range` (rover `move_to(..).range(..)`).
    MoveTo { target: Position, range: u8 },
    /// Move away from each of `from`, keeping at least `range` (rover `flee`).
    Flee { from: Vec<Position>, range: u8 },
    /// Hold position / take no action this tick.
    Idle,
}

/// A swappable tactical brain over the seam. The bot's real logic is `IbexAgent` (in
/// `screeps-combat-agent`, calling the pure decisions in this module); scripted opponents implement
/// it too, so self-play and adversarial scenarios run the same `decide` contract.
pub trait TacticalAgent {
    /// Decide one squad's intents for the tick from the read seam.
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent>;
}

/// **The first extracted decision** (ADR 0006 Inc B): the squad's shared focus target. A faithful
/// port of `attack_mission::compute_focus_target` — kept byte-identical in output so the live shim
/// passes intent parity:
/// 1. hostile with a working HEAL part, lowest hits (kill healers first to deny regen);
/// 2. else the lowest-hits hostile (focus fire for kills);
/// 3. else a hostile structure, prioritized InvaderCore > Spawn > Tower > other;
/// 4. else `None`.
///
/// `min_by_key` returns the *first* of equal minimums, matching the prior logic's tie-break given
/// the adapter preserves hostile/structure ordering.
pub fn select_focus_target(view: &CombatView) -> Option<FocusTarget> {
    if !view.hostiles.is_empty() {
        // Priority 1: hostiles with a working HEAL part, lowest hits.
        if let Some(t) = view
            .hostiles
            .iter()
            .filter(|c| c.has_working(Part::Heal))
            .min_by_key(|c| c.hits)
        {
            return Some(FocusTarget { pos: t.pos, id: t.id });
        }
        // Priority 2: lowest-hits hostile (always succeeds on a non-empty list).
        if let Some(t) = view.hostiles.iter().min_by_key(|c| c.hits) {
            return Some(FocusTarget { pos: t.pos, id: t.id });
        }
    }

    // Priority 3: hostile structures (position-only; structures don't move).
    view.structures
        .iter()
        .filter(|s| s.ownership == Ownership::Hostile)
        .min_by_key(|s| match s.structure_type {
            StructureType::InvaderCore => 0u32,
            StructureType::Spawn => 1,
            StructureType::Tower => 2,
            _ => 10,
        })
        .map(|s| FocusTarget { pos: s.pos, id: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: u8, y: u8) -> Position {
        use screeps::{RoomCoordinate, RoomName};
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    fn body(parts: &[(Part, u32)]) -> Vec<CombatBodyPart> {
        parts
            .iter()
            .flat_map(|&(part, n)| std::iter::repeat_n(CombatBodyPart { part, hits: 100 }, n as usize))
            .collect()
    }

    fn creep(id: u8, x: u8, y: u8, hits: u32, parts: &[(Part, u32)]) -> CombatCreepDto {
        // A deterministic fake RawObjectId from a byte (host-constructible, no game handle).
        let raw = format!("{:024x}", id).parse::<RawObjectId>().unwrap();
        CombatCreepDto {
            id: Some(raw),
            pos: pos(x, y),
            hits,
            hits_max: hits,
            body: body(parts),
        }
    }

    fn structure(x: u8, y: u8, ty: StructureType, ownership: Ownership) -> CombatStructureDto {
        CombatStructureDto {
            pos: pos(x, y),
            structure_type: ty,
            hits: 1000,
            hits_max: 1000,
            ownership,
        }
    }

    fn view<'a>(
        squad: &'a SquadStateDto,
        hostiles: &'a [CombatCreepDto],
        structures: &'a [CombatStructureDto],
    ) -> CombatView<'a> {
        CombatView { tick: 1, squad, friends: &[], hostiles, structures }
    }

    fn squad() -> SquadStateDto {
        SquadStateDto { center: pos(25, 25), room: "W1N1".parse().unwrap() }
    }

    #[test]
    fn healer_is_targeted_before_a_weaker_non_healer() {
        // A 100-hit plain creep and a 500-hit healer: the healer is chosen despite higher hits.
        let s = squad();
        let hostiles = vec![
            creep(1, 20, 20, 100, &[(Part::Attack, 1)]),
            creep(2, 30, 30, 500, &[(Part::Heal, 3)]),
        ];
        let t = select_focus_target(&view(&s, &hostiles, &[])).expect("a target");
        assert_eq!(t.pos, pos(30, 30), "kill the healer first");
        assert_eq!(t.id, hostiles[1].id);
    }

    #[test]
    fn lowest_hits_healer_wins_among_healers() {
        let s = squad();
        let hostiles = vec![
            creep(1, 20, 20, 800, &[(Part::Heal, 5)]),
            creep(2, 30, 30, 300, &[(Part::Heal, 5)]),
        ];
        let t = select_focus_target(&view(&s, &hostiles, &[])).unwrap();
        assert_eq!(t.pos, pos(30, 30), "weakest healer");
    }

    #[test]
    fn lowest_hits_hostile_when_no_healer() {
        let s = squad();
        let hostiles = vec![
            creep(1, 20, 20, 400, &[(Part::Attack, 5)]),
            creep(2, 30, 30, 150, &[(Part::RangedAttack, 5)]),
        ];
        let t = select_focus_target(&view(&s, &hostiles, &[])).unwrap();
        assert_eq!(t.pos, pos(30, 30), "focus the weakest");
    }

    #[test]
    fn a_destroyed_heal_part_does_not_count_as_a_healer() {
        // The "healer" has only a dead HEAL part → it's treated as a plain creep, so the genuinely
        // weaker non-healer is chosen by priority 2.
        let s = squad();
        let mut faux_healer = creep(1, 20, 20, 600, &[(Part::Heal, 1)]);
        faux_healer.body[0].hits = 0; // destroyed HEAL
        let hostiles = vec![faux_healer, creep(2, 30, 30, 150, &[(Part::Attack, 5)])];
        let t = select_focus_target(&view(&s, &hostiles, &[])).unwrap();
        assert_eq!(t.pos, pos(30, 30), "dead HEAL ⇒ not a healer, fall to lowest-hits");
    }

    #[test]
    fn structures_only_when_no_hostile_creeps() {
        let s = squad();
        let structures = vec![
            structure(10, 10, StructureType::Tower, Ownership::Hostile),
            structure(11, 11, StructureType::Spawn, Ownership::Hostile),
            structure(12, 12, StructureType::InvaderCore, Ownership::Hostile),
        ];
        let t = select_focus_target(&view(&s, &[], &structures)).unwrap();
        assert_eq!(t.pos, pos(12, 12), "invader core outranks spawn/tower");
        assert_eq!(t.id, None, "structures are targeted by position");
    }

    #[test]
    fn my_structures_are_never_targeted() {
        let s = squad();
        let structures = vec![
            structure(10, 10, StructureType::Spawn, Ownership::Mine),
            structure(11, 11, StructureType::Tower, Ownership::Neutral),
        ];
        assert_eq!(select_focus_target(&view(&s, &[], &structures)), None);
    }

    #[test]
    fn nothing_to_hit_is_none() {
        let s = squad();
        assert_eq!(select_focus_target(&view(&s, &[], &[])), None);
    }
}
