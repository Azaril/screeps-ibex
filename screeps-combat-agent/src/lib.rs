//! `screeps-combat-agent` — the tactical seam's **sim side** (ADR 0006 Part B, P2.H2).
//!
//! It bridges the two halves of the harness: a [`SimView`] builds the JS-free
//! [`screeps_combat_decision::CombatView`] from a [`screeps_combat_engine::CombatWorld`], and
//! [`IbexAgent`] runs the bot's **real** decision code ([`decide_combat`]) over that view. There is
//! then exactly one implementation of the tactics (the bot's), so self-play is `IbexAgent` vs
//! `IbexAgent` (or vs a scripted opponent) with no fork to drift or overfit — the whole point of
//! the trait seam (ADR 0006 §B.2).
//!
//! Sim creeps have no game `ObjectId`, so [`SimView`] mints a stable synthetic [`RawObjectId`] per
//! creep (from its [`CreepId`]) and keeps the reverse map, so an emitted [`CombatIntent`]'s target
//! id resolves back to a `CombatWorld` creep ([`SimView::creep_for`]) when the sim applies it.
//! [`to_engine_action`] performs that translation for the creep-targeted combat intents.
//!
//! Host-only (workspace-excluded): it depends only on the `screeps-combat-decision` (tactics) and
//! `screeps-combat-engine` (mechanism) member crates — not the whole bot.

pub mod pathing;
pub mod squad;

use screeps::{Position, RawObjectId, RoomName, StructureType};
use screeps_combat_decision::{
    decide_combat, decide_movement, select_focus_target, CombatBodyPart, CombatCreepDto, CombatIntent,
    CombatStructureDto, CombatView, CreepOrders, FocusTarget, Ownership, SquadMovement, SquadStateDto, TacticalAgent,
};
use screeps_combat_engine::{CombatAction, CombatWorld, CreepId, Intents, PlayerId, SimCreep, StructureKind};
use std::collections::HashMap;

/// Mint a stable, host-constructible `RawObjectId` for a sim creep from its `CreepId`. Sim creeps
/// have no game object id; this is purely an addressing handle the sim owns (24 hex digits).
fn synthetic_id(creep: CreepId) -> RawObjectId {
    format!("{:024x}", creep).parse().expect("a 24-hex string is a valid RawObjectId")
}

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

/// Owned DTO backing storage for one side's view of a `CombatWorld` for one tick. The shared squad
/// focus is computed once (`select_focus_target`); per-creep views come from [`SimView::view_for`].
pub struct SimView {
    tick: u32,
    me_owner: PlayerId,
    squad: SquadStateDto,
    /// The deciding side's living creeps, in `CombatWorld::creeps` order.
    friends: Vec<CombatCreepDto>,
    /// Parallel to `friends`: the engine `CreepId` of each (for keying engine intents).
    friend_ids: Vec<CreepId>,
    hostiles: Vec<CombatCreepDto>,
    structures: Vec<CombatStructureDto>,
    /// The shared focus **creep** for the tick (creep-only; structures are scanned per-creep),
    /// mirroring the live `TickOrders.attack_target` broadcast.
    focus: Option<FocusTarget>,
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
        let mut friend_ids = Vec::new();
        let mut hostiles = Vec::new();
        for c in world.creeps.iter().filter(|c| c.is_alive()) {
            let raw = synthetic_id(c.id);
            id_to_creep.insert(raw, c.id);
            let dto = creep_dto(c, raw);
            if c.owner == me_owner {
                friends.push(dto);
                friend_ids.push(c.id);
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

        // The shared focus is creep-only (structures are scanned per-creep by `decide_combat`).
        let focus = select_focus_target(&hostiles, &structures).filter(|f| f.id.is_some());

        Self {
            tick: world.tick,
            me_owner,
            // No squad directive in the per-creep SimView (cohesion_radius 0 → the fallback path);
            // the real squad goal is stamped by `SimSquad` in P2.G3-tail Step 8.
            squad: SquadStateDto { center, room, movement: SquadMovement::Hold, cohesion_radius: 0 },
            friends,
            friend_ids,
            hostiles,
            structures,
            focus,
            id_to_creep,
        }
    }

    /// The side this view is built for.
    pub fn me_owner(&self) -> PlayerId {
        self.me_owner
    }

    /// The deciding side's creeps (parallel to [`SimView::friend_id`]).
    pub fn friends(&self) -> &[CombatCreepDto] {
        &self.friends
    }

    /// The engine `CreepId` of friend `i` (for keying its engine intents).
    pub fn friend_id(&self, i: usize) -> CreepId {
        self.friend_ids[i]
    }

    /// The friend index of a given `CreepId`, if it is on this side and alive.
    pub fn friend_index(&self, id: CreepId) -> Option<usize> {
        self.friend_ids.iter().position(|&x| x == id)
    }

    /// A per-creep read seam for friend `i`, carrying the shared squad focus as its orders.
    pub fn view_for(&self, i: usize) -> CombatView<'_> {
        CombatView {
            tick: self.tick,
            me: &self.friends[i],
            squad: &self.squad,
            orders: Some(CreepOrders { focus: self.focus, heal_target: None }),
            friends: &self.friends,
            hostiles: &self.hostiles,
            structures: &self.structures,
        }
    }

    /// Resolve an emitted intent's target id back to its engine `CreepId`.
    pub fn creep_for(&self, id: RawObjectId) -> Option<CreepId> {
        self.id_to_creep.get(&id).copied()
    }
}

/// Translate a creep-targeted [`CombatIntent`] into a [`CombatAction`] the engine resolver accepts
/// (resolving the target id back to a `CreepId`). Returns `None` for movement intents and
/// structure-targeted intents (the latter need structure-id resolution — a follow-on with the
/// scenario/apply layer, P2.H4).
pub fn to_engine_action(intent: &CombatIntent, view: &SimView) -> Option<CombatAction> {
    match intent {
        CombatIntent::Attack { id: Some(raw), .. } => view.creep_for(*raw).map(CombatAction::Attack),
        CombatIntent::RangedAttack { id: Some(raw), .. } => view.creep_for(*raw).map(CombatAction::RangedAttack),
        CombatIntent::RangedMassAttack => Some(CombatAction::RangedMassAttack),
        CombatIntent::Heal { id: Some(raw), .. } => view.creep_for(*raw).map(CombatAction::Heal),
        CombatIntent::RangedHeal { id: Some(raw), .. } => view.creep_for(*raw).map(CombatAction::RangedHeal),
        _ => None,
    }
}

/// The bot's real tactical brain, driven over the seam. Wraps [`decide_combat`] (the extracted
/// per-tick attack + heal decision). Per-creep: call once per friendly creep with its `view_for`.
#[derive(Default)]
pub struct IbexAgent;

impl TacticalAgent for IbexAgent {
    fn decide(&mut self, view: &CombatView) -> Vec<CombatIntent> {
        // The full per-tick decision: combat (attack + heal) plus the per-creep tactical movement
        // goal (kite/engage/flee/heal-follow). Both are the bot's real, shared decisions.
        let mut intents = decide_combat(view);
        intents.extend(decide_movement(view));
        intents
    }
}

/// Build the engine [`Intents`] for one side by running `agent` over each of its creeps and
/// translating the emitted [`CombatIntent`]s: combat intents → engine [`CombatAction`]s
/// ([`to_engine_action`]), movement intents → a step [`screeps::Direction`] planned through rover
/// ([`pathing::resolve_move_direction`]) and applied via `set_move`. This is the sim's per-tick
/// step: hand the result to `resolve_tick` (the engine — the authoritative "server").
pub fn agent_intents<A: TacticalAgent>(world: &CombatWorld, sim: &SimView, agent: &mut A) -> Intents {
    let mut intents = Intents::new();
    for i in 0..sim.friends().len() {
        let view = sim.view_for(i);
        let me_pos = sim.friends()[i].pos;
        let creep_id = sim.friend_id(i);
        let mut actions = Vec::new();
        for intent in agent.decide(&view) {
            if let Some(action) = to_engine_action(&intent, sim) {
                actions.push(action);
            } else if let Some(dir) = pathing::resolve_move_direction(world, me_pos, sim.me_owner(), &intent) {
                intents.set_move(creep_id, dir);
            }
        }
        if !actions.is_empty() {
            intents.set(creep_id, actions);
        }
    }
    intents
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
    use screeps_combat_engine::{resolve_tick, SimBody};

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }
    fn creep(id: CreepId, owner: PlayerId, x: u8, y: u8, parts: &[(Part, usize)]) -> SimCreep {
        let body: Vec<Part> = parts.iter().flat_map(|&(p, n)| std::iter::repeat_n(p, n)).collect();
        SimCreep { id, owner, pos: pos(x, y), body: SimBody::unboosted(&body), fatigue: 0 }
    }

    /// Run `IbexAgent` for every friendly creep and collect the engine intents (combat + movement).
    fn ibex_intents(world: &CombatWorld, me_owner: PlayerId) -> Intents {
        let sv = SimView::from_world(world, me_owner, pos(25, 25), room());
        agent_intents(world, &sv, &mut IbexAgent)
    }

    #[test]
    fn ibex_agent_focus_fires_the_healer() {
        // The bot's real decision, over the sim: a ranged attacker focus-fires the hostile healer
        // (the squad focus) even though a non-healer is weaker.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 0, 24, 25, &[(Part::RangedAttack, 7)]), // me
                creep(2, 1, 23, 25, &[(Part::Move, 1)]),         // hostile weakling (low hits)
                creep(3, 1, 26, 25, &[(Part::Heal, 5)]),         // hostile healer
            ],
            ..Default::default()
        };
        let sv = SimView::from_world(&world, 0, pos(25, 25), room());
        let intents = decide_combat(&sv.view_for(0));
        // Healer (creep 3) is within range 3 → RangedAttack it, not the weaker non-focus.
        let raw = synthetic_id(3);
        assert_eq!(intents, vec![CombatIntent::RangedAttack { target: pos(26, 25), id: Some(raw) }]);
    }

    #[test]
    fn ibex_agent_focus_fires_a_healer_to_death_through_the_engine() {
        // End-to-end: 3 ranged attackers (owner 0) whose REAL decision picks the hostile healer;
        // fed to the engine resolver, they kill it (no self-heal — the healer takes no action).
        let mut world = CombatWorld {
            creeps: vec![
                creep(10, 1, 25, 25, &[(Part::Heal, 5)]), // 500-hit hostile healer
                creep(1, 0, 24, 25, &[(Part::RangedAttack, 7)]),
                creep(2, 0, 26, 25, &[(Part::RangedAttack, 7)]),
                creep(3, 0, 25, 24, &[(Part::RangedAttack, 7)]),
            ],
            ..Default::default()
        };
        let mut died = false;
        for _ in 0..10 {
            let intents = ibex_intents(&world, 0);
            if resolve_tick(&mut world, &intents).deaths.contains(&10) {
                died = true;
                break;
            }
        }
        assert!(died, "the bot's focus-fire kills the healer (210 dps vs 500 hits → 3 ticks)");
    }

    #[test]
    fn ibex_agent_kites_a_melee_chaser_taking_no_damage() {
        // EXP-KITE-1, now driven by the SEAM (not hand-set moves): a ranged kiter (7 RANGED + 7
        // MOVE) vs a melee chaser (10 ATTACK + 10 MOVE, MOVE parity), self-play through the engine.
        // decide_movement keeps the kiter out of melee range, so it takes 0 damage (melee never
        // connects; ranged fire has no attack-back) while chipping the chaser.
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 0, 30, 25, &[(Part::RangedAttack, 7), (Part::Move, 7)]), // kiter
                creep(2, 1, 27, 25, &[(Part::Attack, 10), (Part::Move, 10)]),     // melee chaser, range 3
            ],
            ..Default::default()
        };
        let kiter_max = world.creeps[0].body.hits_max();
        let chaser_max = world.creeps[1].body.hits_max();
        for _ in 0..10 {
            // Self-play: both sides decide via IbexAgent; merge (disjoint creep ids) and resolve.
            let sv0 = SimView::from_world(&world, 0, pos(30, 25), room());
            let mut intents = agent_intents(&world, &sv0, &mut IbexAgent);
            let sv1 = SimView::from_world(&world, 1, pos(27, 25), room());
            let i1 = agent_intents(&world, &sv1, &mut IbexAgent);
            intents.creeps.extend(i1.creeps);
            intents.moves.extend(i1.moves);
            resolve_tick(&mut world, &intents);
        }
        let kiter = world.creeps.iter().find(|c| c.id == 1).expect("kiter survives");
        let chaser = world.creeps.iter().find(|c| c.id == 2).expect("chaser still up");
        assert_eq!(kiter.body.hits, kiter_max, "kiter never let the melee chaser connect → 0 damage");
        assert!(chaser.body.hits < chaser_max, "kiter chipped the chaser with ranged fire");
        assert!(kiter.pos.get_range_to(chaser.pos) >= 2, "stayed out of melee range");
    }

    #[test]
    fn ibex_agent_mass_attacks_when_surrounded() {
        // One ranged creep with 3 hostiles adjacent → RMA (the stacked case), not single-target.
        let world = CombatWorld {
            creeps: vec![
                creep(1, 0, 25, 25, &[(Part::RangedAttack, 7)]),
                creep(2, 1, 24, 25, &[(Part::Move, 1)]),
                creep(3, 1, 26, 25, &[(Part::Move, 1)]),
                creep(4, 1, 25, 24, &[(Part::Move, 1)]),
            ],
            ..Default::default()
        };
        let sv = SimView::from_world(&world, 0, pos(25, 25), room());
        assert_eq!(decide_combat(&sv.view_for(0)), vec![CombatIntent::RangedMassAttack]);
    }

    #[test]
    fn ibex_agent_heals_a_wounded_ally() {
        // A healer with a wounded adjacent ally heals it (heal-best-nearby, no orders.heal_target).
        let mut wounded = creep(2, 0, 25, 26, &[(Part::Move, 5)]);
        wounded.body.hits = 100; // damaged (max 500)
        let world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 25, &[(Part::Heal, 5)]), wounded, creep(9, 1, 40, 40, &[(Part::Attack, 1)])],
            ..Default::default()
        };
        let sv = SimView::from_world(&world, 0, pos(25, 25), room());
        // friend index of the healer (creep 1) is 0.
        assert_eq!(
            decide_combat(&sv.view_for(0)),
            vec![CombatIntent::Heal { target: pos(25, 26), id: Some(synthetic_id(2)) }]
        );
    }

    #[test]
    fn hold_agent_always_idles() {
        let world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 25, &[(Part::Heal, 5)]), creep(2, 1, 30, 30, &[(Part::Heal, 5)])],
            ..Default::default()
        };
        let sv = SimView::from_world(&world, 0, pos(25, 25), room());
        assert_eq!(HoldAgent.decide(&sv.view_for(0)), vec![CombatIntent::Idle]);
    }
}
