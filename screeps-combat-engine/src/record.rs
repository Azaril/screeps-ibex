//! `CombatRecording` — the per-tick replay artifact that makes an engagement **introspectable**:
//! for every tick it captures the pre-tick world (creep/structure positions + hits), the intents
//! each actor issued (with the agent's optional "why" reason tag), and the resolved outcomes
//! (damage taken/healed, deaths, structures destroyed). This is the "see WHY a squad did X on tick
//! N" capability (ADR 0006 Part B §5) — deterministic, so old-vs-new tactics are diffable by
//! replaying the same scenario+seed.
//!
//! The data model lives here (the engine *mechanism*); a richer SVG/scrubber renderer is policy
//! and lives in `screeps-combat-eval`. [`CombatRecording::render`] gives a readable text dump.

use crate::resolve::{resolve_tick, CombatAction, Intents, TickReport, TowerAction};
use crate::state::{CombatWorld, CreepId, PlayerId, StructureId, StructureKind};
use screeps::Direction;

/// A creep's state at the start of a recorded tick.
#[derive(Clone, Debug)]
pub struct CreepFrame {
    pub id: CreepId,
    pub owner: PlayerId,
    pub x: u8,
    pub y: u8,
    pub hits: u32,
    pub hits_max: u32,
    pub fatigue: u32,
}

/// A structure's state at the start of a recorded tick.
#[derive(Clone, Debug)]
pub struct StructureFrame {
    pub id: StructureId,
    pub kind: StructureKind,
    pub owner: Option<PlayerId>,
    pub x: u8,
    pub y: u8,
    pub hits: u32,
    pub hits_max: u32,
}

/// What a creep was told to do this tick, and why.
#[derive(Clone, Debug)]
pub struct CreepIntentRecord {
    pub id: CreepId,
    pub actions: Vec<CombatAction>,
    pub mv: Option<Direction>,
    pub reason: Option<String>,
}

/// What happened to a creep this tick.
#[derive(Clone, Copy, Debug)]
pub struct CreepResult {
    pub id: CreepId,
    pub damage_taken: u32,
    pub healed: u32,
    pub died: bool,
}

/// One recorded tick: pre-tick state + intents + outcomes.
#[derive(Clone, Debug, Default)]
pub struct TickFrame {
    pub tick: u32,
    pub creeps: Vec<CreepFrame>,
    pub structures: Vec<StructureFrame>,
    pub intents: Vec<CreepIntentRecord>,
    pub tower_intents: Vec<(usize, TowerAction)>,
    pub results: Vec<CreepResult>,
    pub deaths: Vec<CreepId>,
    pub destroyed_structures: Vec<StructureId>,
}

/// A full engagement recording — a deterministic, scrubbable sequence of frames.
#[derive(Clone, Debug, Default)]
pub struct CombatRecording {
    pub frames: Vec<TickFrame>,
}

impl CombatRecording {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// A readable per-tick text dump (the minimal "scrubber"; richer SVG rendering is the eval
    /// crate's job). Deterministic: rows are id-sorted, so output never depends on map iteration.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        for f in &self.frames {
            let _ = writeln!(s, "=== tick {} ===", f.tick);
            for c in &f.creeps {
                let ir = f.intents.iter().find(|r| r.id == c.id);
                let acts = ir.map(|r| format!(" {:?}", r.actions)).unwrap_or_default();
                let mv = ir
                    .and_then(|r| r.mv)
                    .map(|d| format!(" mv:{:?}", d))
                    .unwrap_or_default();
                let why = ir
                    .and_then(|r| r.reason.as_deref())
                    .map(|x| format!("  [{x}]"))
                    .unwrap_or_default();
                let res = f.results.iter().find(|r| r.id == c.id);
                let outcome = res
                    .map(|r| {
                        let died = if r.died { " DIED" } else { "" };
                        format!(" (-{}/+{}{})", r.damage_taken, r.healed, died)
                    })
                    .unwrap_or_default();
                let _ = writeln!(
                    s,
                    "  #{} P{} ({},{}) {}/{}hp{}{}{}{}",
                    c.id, c.owner, c.x, c.y, c.hits, c.hits_max, mv, outcome, acts, why
                );
            }
            for st in &f.structures {
                let _ = writeln!(
                    s,
                    "  [{:?} P{:?} #{}] ({},{}) {}/{}",
                    st.kind, st.owner, st.id, st.x, st.y, st.hits, st.hits_max
                );
            }
            if !f.destroyed_structures.is_empty() {
                let _ = writeln!(s, "  destroyed: {:?}", f.destroyed_structures);
            }
        }
        s
    }
}

/// Snapshot the **pre-tick** world, resolve the tick, and append a [`TickFrame`]. Drop-in for
/// [`resolve_tick`] when you want a replay. Returns the same [`TickReport`].
pub fn record_tick(
    rec: &mut CombatRecording,
    world: &mut CombatWorld,
    intents: &Intents,
) -> TickReport {
    let tick = world.tick;
    let creeps: Vec<CreepFrame> = world
        .creeps
        .iter()
        .map(|c| CreepFrame {
            id: c.id,
            owner: c.owner,
            x: c.pos.x().u8(),
            y: c.pos.y().u8(),
            hits: c.body.hits,
            hits_max: c.body.hits_max(),
            fatigue: c.fatigue,
        })
        .collect();
    let structures: Vec<StructureFrame> = world
        .structures
        .iter()
        .map(|s| StructureFrame {
            id: s.id,
            kind: s.kind,
            owner: s.owner,
            x: s.pos.x().u8(),
            y: s.pos.y().u8(),
            hits: s.hits,
            hits_max: s.hits_max,
        })
        .collect();

    // Intent records: union of creeps with combat actions and creeps with only a move; id-sorted.
    let mut intent_records: Vec<CreepIntentRecord> = intents
        .creeps
        .iter()
        .map(|(&id, actions)| CreepIntentRecord {
            id,
            actions: actions.clone(),
            mv: intents.moves.get(&id).copied(),
            reason: intents.reasons.get(&id).cloned(),
        })
        .collect();
    for (&id, &dir) in &intents.moves {
        if !intents.creeps.contains_key(&id) {
            intent_records.push(CreepIntentRecord {
                id,
                actions: Vec::new(),
                mv: Some(dir),
                reason: intents.reasons.get(&id).cloned(),
            });
        }
    }
    intent_records.sort_by_key(|r| r.id);
    let mut tower_intents: Vec<(usize, TowerAction)> =
        intents.towers.iter().map(|(&i, &a)| (i, a)).collect();
    tower_intents.sort_by_key(|x| x.0);

    let report = resolve_tick(world, intents);

    let mut results: Vec<CreepResult> = report
        .outcomes
        .iter()
        .map(|(&id, o)| CreepResult {
            id,
            damage_taken: o.effective_damage,
            healed: o.heal,
            died: o.died,
        })
        .collect();
    results.sort_by_key(|r| r.id);
    let mut deaths = report.deaths.clone();
    deaths.sort_unstable();
    let mut destroyed_structures = report.destroyed_structures.clone();
    destroyed_structures.sort_unstable();

    rec.frames.push(TickFrame {
        tick,
        creeps,
        structures,
        intents: intent_records,
        tower_intents,
        results,
        deaths,
        destroyed_structures,
    });
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::SimBody;
    use crate::state::{SimCreep, SimStructure};
    use screeps::{Part, Position, RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(
            RoomCoordinate::new(x).unwrap(),
            RoomCoordinate::new(y).unwrap(),
            room,
        )
    }
    fn creep(id: CreepId, owner: PlayerId, x: u8, y: u8, parts: &[(Part, u32)]) -> SimCreep {
        let body: Vec<_> = parts
            .iter()
            .flat_map(|&(p, n)| {
                std::iter::repeat(crate::body::BodyPartDef::new(p)).take(n as usize)
            })
            .collect();
        SimCreep {
            id,
            owner,
            pos: pos(x, y),
            body: SimBody::new(body),
            fatigue: 0,
        }
    }

    #[test]
    fn records_a_kiting_engagement_with_reasons() {
        let mut world = CombatWorld {
            creeps: vec![
                creep(1, 0, 30, 25, &[(Part::RangedAttack, 7), (Part::Move, 7)]),
                creep(2, 1, 27, 25, &[(Part::Attack, 10), (Part::Move, 10)]),
            ],
            ..Default::default()
        };
        let mut rec = CombatRecording::new();
        for _ in 0..5 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::RangedAttack(2)]);
            i.set_move(1, Direction::Right);
            i.set_reason(1, "kite: hold range 3");
            i.set(2, vec![CombatAction::Attack(1)]);
            i.set_move(2, Direction::Right);
            i.set_reason(2, "chase");
            record_tick(&mut rec, &mut world, &i);
        }
        assert_eq!(rec.len(), 5);
        // Frame 0 captures the START positions (30,25)/(27,25) and the reasons.
        let f0 = &rec.frames[0];
        assert_eq!(f0.tick, 0);
        let kiter = f0.creeps.iter().find(|c| c.id == 1).unwrap();
        assert_eq!((kiter.x, kiter.y), (30, 25));
        let kiter_intent = f0.intents.iter().find(|r| r.id == 1).unwrap();
        assert_eq!(kiter_intent.reason.as_deref(), Some("kite: hold range 3"));
        // Last frame: the kiter advanced; the chaser has taken ranged damage at some point.
        let last = &rec.frames[4];
        assert_eq!(last.creeps.iter().find(|c| c.id == 1).unwrap().x, 34);
        // The text dump renders and mentions the reason tag.
        let text = rec.render();
        assert!(text.contains("=== tick 0 ==="));
        assert!(text.contains("kite: hold range 3"));
    }

    #[test]
    fn records_a_breach_with_destruction() {
        let mut world = CombatWorld {
            creeps: vec![creep(1, 0, 25, 25, &[(Part::Work, 10)])],
            structures: vec![SimStructure {
                id: 100,
                kind: StructureKind::Wall,
                owner: None,
                pos: pos(25, 26),
                hits: 1000,
                hits_max: 1000,
            }],
            ..Default::default()
        };
        let mut rec = CombatRecording::new();
        for _ in 0..2 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::Dismantle(100)]);
            record_tick(&mut rec, &mut world, &i);
        }
        // 10 WORK = 500/tick; wall (1000) destroyed on tick 1's frame.
        assert!(rec.frames[1].destroyed_structures.contains(&100));
        assert!(rec.render().contains("destroyed"));
    }
}
