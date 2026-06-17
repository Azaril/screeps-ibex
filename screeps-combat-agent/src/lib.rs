//! `screeps-combat-agent` — the tactical seam's **sim side** (ADR 0006 Part B, P2.H2).
//!
//! It bridges the two halves of the harness: a [`SimView`] builds the bot's JS-free
//! [`screeps_ibex::combat::CombatView`] from a [`screeps_combat_engine::CombatWorld`], and
//! [`IbexAgent`] runs the bot's **real** decision code over that view. There is then exactly one
//! implementation of the tactics (the bot's), so self-play is `IbexAgent` vs `IbexAgent` (or vs a
//! scripted opponent) with no fork to drift or overfit — the whole point of the trait seam
//! (ADR 0006 §B.2).
//!
//! Sim creeps have no game `ObjectId`, so [`SimView`] mints a stable synthetic [`RawObjectId`] per
//! creep (from its [`CreepId`]) and keeps the reverse map, so an emitted [`CombatIntent`]'s target
//! id resolves back to a `CombatWorld` creep ([`SimView::creep_for`]) when the sim applies it.
//!
//! Host-only (workspace-excluded): it depends on the full bot crate at the host target.

use screeps::{Position, RawObjectId, RoomName, StructureType};
use screeps_combat_engine::{CombatWorld, CreepId, PlayerId, SimCreep, StructureKind};
use screeps_ibex::combat::{
    select_focus_target, CombatBodyPart, CombatCreepDto, CombatIntent, CombatStructureDto, CombatView, Ownership,
    SquadStateDto, TacticalAgent,
};
use std::collections::HashMap;

/// Mint a stable, host-constructible `RawObjectId` for a sim creep from its `CreepId`. Sim creeps
/// have no game object id; this is purely an addressing handle the sim owns (24 hex digits).
fn synthetic_id(creep: CreepId) -> RawObjectId {
    format!("{:024x}", creep).parse().expect("a 24-hex string is a valid RawObjectId")
}

/// Map an engine structure kind to the game-api `StructureType` the decision reasons about.
fn structure_type(kind: StructureKind) -> StructureType {
    match kind {
        StructureKind::Spawn => StructureType::Spawn,
        StructureKind::Rampart => StructureType::Rampart,
        StructureKind::Wall => StructureType::Wall,
        StructureKind::Tower => StructureType::Tower,
    }
}

fn ownership(owner: Option<PlayerId>, me: PlayerId) -> Ownership {
    match owner {
        Some(o) if o == me => Ownership::Mine,
        Some(_) => Ownership::Hostile,
        None => Ownership::Neutral,
    }
}

fn creep_dto(c: &SimCreep, raw: RawObjectId) -> CombatCreepDto {
    let body = (0..c.body.parts.len())
        .map(|i| CombatBodyPart {
            part: c.body.parts[i].part,
            hits: c.body.part_hits(i),
        })
        .collect();
    CombatCreepDto {
        id: Some(raw),
        pos: c.pos,
        hits: c.body.hits,
        hits_max: c.body.hits_max(),
        body,
    }
}

/// Owned DTO backing storage for one side's view of a `CombatWorld` for one tick. Borrow it as a
/// [`CombatView`] with [`SimView::view`]; resolve a returned intent's target id back to its engine
/// creep with [`SimView::creep_for`].
pub struct SimView {
    tick: u32,
    squad: SquadStateDto,
    friends: Vec<CombatCreepDto>,
    hostiles: Vec<CombatCreepDto>,
    structures: Vec<CombatStructureDto>,
    /// synthetic `RawObjectId` → the engine `CreepId` it stands for (both sides).
    id_to_creep: HashMap<RawObjectId, CreepId>,
}

impl SimView {
    /// Build the view from `me_owner`'s perspective: its living creeps are `friends`, all other
    /// living creeps are `hostiles`; structures + towers are classified mine / hostile / neutral.
    /// `room` is the room the (single-room) world models; `center` is the deciding squad's centroid.
    pub fn from_world(world: &CombatWorld, me_owner: PlayerId, center: Position, room: RoomName) -> Self {
        let mut id_to_creep = HashMap::new();
        let mut friends = Vec::new();
        let mut hostiles = Vec::new();
        for c in world.creeps.iter().filter(|c| c.is_alive()) {
            let raw = synthetic_id(c.id);
            id_to_creep.insert(raw, c.id);
            let dto = creep_dto(c, raw);
            if c.owner == me_owner {
                friends.push(dto);
            } else {
                hostiles.push(dto);
            }
        }

        let mut structures: Vec<CombatStructureDto> = world
            .structures
            .iter()
            .filter(|s| s.is_alive())
            .map(|s| CombatStructureDto {
                pos: s.pos,
                structure_type: structure_type(s.kind),
                hits: s.hits,
                hits_max: s.hits_max,
                ownership: ownership(s.owner, me_owner),
            })
            .collect();
        // Towers are targetable structures too (engine kind `Tower`).
        structures.extend(world.towers.iter().filter(|t| t.is_alive()).map(|t| CombatStructureDto {
            pos: t.pos,
            structure_type: StructureType::Tower,
            hits: t.hits,
            hits_max: t.hits_max,
            ownership: ownership(Some(t.owner), me_owner),
        }));

        Self {
            tick: world.tick,
            squad: SquadStateDto { center, room },
            friends,
            hostiles,
            structures,
            id_to_creep,
        }
    }

    /// Borrow the backing storage as the bot's JS-free read seam.
    pub fn view(&self) -> CombatView<'_> {
        CombatView {
            tick: self.tick,
            squad: &self.squad,
            friends: &self.friends,
            hostiles: &self.hostiles,
            structures: &self.structures,
        }
    }

    /// Resolve an emitted intent's target id back to its engine `CreepId` (for applying the intent
    /// to the `CombatWorld`).
    pub fn creep_for(&self, id: RawObjectId) -> Option<CreepId> {
        self.id_to_creep.get(&id).copied()
    }
}

/// The bot's real tactical brain, driven over the seam. For P2.H2's first decision it wraps
/// [`select_focus_target`] (the squad's shared focus, mirroring the live `TickOrders.attack_target`
/// broadcast). Later H2 slices grow `decide` toward the full per-tick orders (kite/heal/engage).
#[derive(Default)]
pub struct IbexAgent;

impl TacticalAgent for IbexAgent {
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent> {
        match select_focus_target(view) {
            Some(t) => {
                let mut intents = vec![CombatIntent::MoveTo { target: t.pos, range: 1 }];
                if let Some(id) = t.id {
                    intents.push(CombatIntent::Attack(id));
                }
                intents
            }
            None => vec![CombatIntent::Idle],
        }
    }
}

/// A trivial scripted opponent — always holds. Proves the [`TacticalAgent`] trait is swappable for
/// self-play / adversarial scenarios (P2.H4 grows the roster: rush / kite / turtle / drain).
#[derive(Default)]
pub struct HoldAgent;

impl TacticalAgent for HoldAgent {
    fn decide(&mut self, _view: &CombatView) -> Vec<CombatIntent> {
        vec![CombatIntent::Idle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{Part, RoomCoordinate};
    use screeps_combat_engine::SimBody;

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }
    fn creep(id: CreepId, owner: PlayerId, x: u8, y: u8, parts: &[(Part, usize)]) -> SimCreep {
        let body: Vec<Part> = parts.iter().flat_map(|&(p, n)| std::iter::repeat_n(p, n)).collect();
        SimCreep {
            id,
            owner,
            pos: pos(x, y),
            body: SimBody::unboosted(&body),
            fatigue: 0,
        }
    }

    #[test]
    fn ibex_agent_targets_the_healer_over_the_sim_world() {
        // The bot's REAL select_focus_target, run over a CombatWorld via the sim adapter, must pick
        // the hostile healer (creep 3) even though it has more hits than the weakling (creep 2).
        let world = CombatWorld {
            creeps: vec![
                creep(1, 0, 25, 25, &[(Part::Attack, 5)]), // me
                creep(2, 1, 20, 20, &[(Part::Attack, 1)]), // hostile weakling (100 hits)
                creep(3, 1, 30, 30, &[(Part::Heal, 5)]),   // hostile healer (500 hits)
            ],
            ..Default::default()
        };
        let sv = SimView::from_world(&world, 0, pos(25, 25), room());
        let mut agent = IbexAgent;
        let intents = agent.decide(&sv.view());

        let attacked = intents
            .iter()
            .find_map(|i| match i {
                CombatIntent::Attack(id) => Some(*id),
                _ => None,
            })
            .expect("the agent emits an attack");
        assert_eq!(
            sv.creep_for(attacked),
            Some(3),
            "the bot's real logic, driven over the sim, picks the healer"
        );
        assert!(
            intents.contains(&CombatIntent::MoveTo { target: pos(30, 30), range: 1 }),
            "and closes on it"
        );
    }

    #[test]
    fn ibex_agent_falls_to_structures_then_idle() {
        // No hostile creeps, one hostile spawn → target the spawn by position (id None). With
        // nothing at all → Idle.
        use screeps_combat_engine::{SimStructure, StructureId};
        let spawn = SimStructure {
            id: 100 as StructureId,
            kind: StructureKind::Spawn,
            owner: Some(1),
            pos: pos(40, 40),
            hits: 1000,
            hits_max: 1000,
        };
        let world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 25, &[(Part::Attack, 5)])],
            structures: vec![spawn],
            ..Default::default()
        };
        let sv = SimView::from_world(&world, 0, pos(25, 25), room());
        let intents = IbexAgent.decide(&sv.view());
        assert!(
            intents.contains(&CombatIntent::MoveTo { target: pos(40, 40), range: 1 }),
            "moves on the hostile spawn"
        );
        assert!(
            !intents.iter().any(|i| matches!(i, CombatIntent::Attack(_))),
            "no creep-id attack for a structure target"
        );

        let empty = CombatWorld {
            creeps: vec![creep(1, 0, 25, 25, &[(Part::Attack, 5)])],
            ..Default::default()
        };
        let sv = SimView::from_world(&empty, 0, pos(25, 25), room());
        assert_eq!(IbexAgent.decide(&sv.view()), vec![CombatIntent::Idle]);
    }

    #[test]
    fn hold_agent_always_idles() {
        let world = CombatWorld {
            creeps: vec![creep(2, 1, 30, 30, &[(Part::Heal, 5)])],
            ..Default::default()
        };
        let sv = SimView::from_world(&world, 0, pos(25, 25), room());
        assert_eq!(HoldAgent.decide(&sv.view()), vec![CombatIntent::Idle]);
    }
}
