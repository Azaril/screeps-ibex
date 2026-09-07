//! The H5 parity oracle's **live-side deterministic driver** (ADR 0006 §B.4; WS-CLOSE lane (b3)).
//!
//! The private server cannot replay the bot's decisions reproducibly, so a golden vector needs
//! SCRIPTED intents. When the harness sets `Memory._features.eval.parity_script = "<scenario>"`
//! and injects the scenario's script into `Memory.parity_script`, this module — once per tick,
//! BEFORE the systems run — does two things for the creeps named `pv-<scenario>-<n>`:
//!
//! 1. **executes the fixed per-tick intent table** for that tick: combat actions go through the
//!    guarded sink in [`crate::intents`] (so the seg-57 `IntentRecorder` digest stays comparable
//!    with ordinary play), movement is a raw `creep.move(dir)` (the vector measures the ENGINE's
//!    conflict resolution, not the rover), pulls/tower fire are raw calls;
//! 2. **prints one `PV1 ` console line per visible scripted creep** (own AND hostile — both users'
//!    creeps stand in the same room, so one capture sees both sides) with the tick-START state
//!    (`x y hits fatigue parts`) plus the intents this driver issued, and one roster line naming
//!    the scripted creeps that are absent (dead) and the script's labelled structures/towers that
//!    are gone (destroyed). Each labelled structure (`s`) / tower (`tw`) still standing gets its own
//!    per-tick hits (+ tower energy) line, so a capture carries the same structure/tower/destroyed
//!    fields the sim's `replay()` frames do. The lines carry `Game.time`, so the capture is
//!    tick-exact regardless of the kit's 2 s sampling.
//!
//! `trace: true` in the script memory turns off (1) — the bot's ordinary systems decide and the
//! driver only prints frames (the layer-2 "unscripted bed").
//!
//! Inert when the feature is empty (the default): no Memory read, no console line. Ordinary
//! gameplay never adopts `pv-*` creeps (they are DB-seeded, never spawned, so no job or squad
//! binds them) — this driver is the only thing that moves them.
//!
//! The wire shape of `Memory.parity_script` is the engine crate's
//! `screeps_combat_engine::parity::TickScript` JSON (pinned there by `script_wire_shape_is_pinned`
//! and here by `script_wire_shape_matches_the_engine_pin`) wrapped in a [`ScriptMemory`] envelope.

use crate::features::Features;
use crate::intents::{self, IntentRecorder};
use crate::jobs::actions::SimultaneousActionFlags;
use log::*;
use screeps::prelude::*;
use screeps::{find, game, Creep, Direction, Part, ResourceType, RoomName, StructureObject};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use specs::World;
use std::collections::BTreeMap;

/// Console marker every driver line starts with (after the fern `(INFO) target: ` prefix). Pinned
/// by `screeps-ibex-eval::gates`.
pub const PV1_MARKER: &str = "PV1 ";

/// `Memory.<key>` holding the injected [`ScriptMemory`].
pub const SCRIPT_MEMORY_KEY: &str = "parity_script";

/// Longest scenario name the feature flag can carry (the flag is a `Copy` fixed-size field so
/// `Features` stays `Copy`; longer names are truncated at a char boundary).
pub const SCENARIO_NAME_CAP: usize = 31;

/// The scenario name in `Memory._features.eval.parity_script` — a fixed-capacity, `Copy` string so
/// the `Features` resource keeps its `Copy` contract. Empty = the driver is off.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct ParityScriptName {
    len: u8,
    bytes: [u8; SCENARIO_NAME_CAP],
}

impl ParityScriptName {
    pub fn new(name: &str) -> Self {
        let mut end = name.len().min(SCENARIO_NAME_CAP);
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0u8; SCENARIO_NAME_CAP];
        bytes[..end].copy_from_slice(&name.as_bytes()[..end]);
        Self { len: end as u8, bytes }
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl std::fmt::Debug for ParityScriptName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl Serialize for ParityScriptName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ParityScriptName {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::new(&s))
    }
}

// ── the injected script (engine `parity::TickScript` wire shape) ──────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ScriptAction {
    Attack { target: String },
    RangedAttack { target: String },
    RangedMassAttack,
    Heal { target: String },
    RangedHeal { target: String },
    Dismantle { target: String },
    AttackStructure { target: String },
    RangedAttackStructure { target: String },
    AttackController,
}

#[derive(Debug, Clone, Deserialize)]
struct ScriptIntent {
    creep: String,
    #[serde(default)]
    actions: Vec<ScriptAction>,
    #[serde(default, rename = "move")]
    mv: Option<String>,
    #[serde(default)]
    pull: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TowerScriptAction {
    Attack { target: String },
    Heal { target: String },
    Repair { target: String },
}

#[derive(Debug, Clone, Deserialize)]
struct TowerScript {
    tower: String,
    action: TowerScriptAction,
}

#[derive(Debug, Clone, Deserialize)]
struct TickScript {
    t: u32,
    #[serde(default)]
    intents: Vec<ScriptIntent>,
    #[serde(default)]
    towers: Vec<TowerScript>,
}

/// A structure/tower label → tile, so the script can address structures by label.
#[derive(Debug, Clone, Deserialize)]
struct Labeled {
    id: String,
    x: u8,
    y: u8,
}

/// `Memory.parity_script`.
#[derive(Debug, Clone, Deserialize)]
struct ScriptMemory {
    scenario: String,
    room: String,
    /// Absolute `Game.time` of scenario tick 0. Absent ⇒ the driver stamps the current tick.
    #[serde(default)]
    start: Option<u32>,
    /// Frames only, no intents (the layer-2 unscripted bed).
    #[serde(default)]
    trace: bool,
    /// Every scripted creep name (both users), for the absent/dead roster line.
    #[serde(default)]
    roster: Vec<String>,
    #[serde(default)]
    structures: Vec<Labeled>,
    #[serde(default)]
    towers: Vec<Labeled>,
    #[serde(default)]
    ticks: Vec<TickScript>,
}

// ── the PV1 lines ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Pv1Creep<'a> {
    g: u32,
    t: u32,
    n: &'a str,
    mine: bool,
    x: u8,
    y: u8,
    hits: u32,
    hits_max: u32,
    fatigue: u32,
    /// `[part, hits, boost]` per body part, front to back.
    parts: Vec<(&'static str, u32, Option<String>)>,
    /// Intents this driver issued for the creep this tick (own creeps only).
    did: Vec<String>,
}

/// A labelled structure still standing at its tile (tick-START hits).
#[derive(Serialize)]
struct Pv1Structure<'a> {
    g: u32,
    t: u32,
    s: &'a str,
    hits: u32,
}

/// A labelled tower still standing at its tile (tick-START hits + energy) plus the tower intents
/// this driver issued for it this tick (own towers only; `!` = the game API REJECTED the call —
/// e.g. `ERR_RCL_NOT_ENOUGH` for a tower whose room controller its owner does not hold — so a
/// capture shows a tower that was told to fire but could not).
#[derive(Serialize)]
struct Pv1Tower<'a> {
    g: u32,
    t: u32,
    tw: &'a str,
    hits: u32,
    energy: u32,
    did: Vec<String>,
}

/// The roster line: scripted creeps not visible (`absent`) and labelled structures/towers not
/// standing at their tile (`gone`) this tick. `gone` needs room vision to mean destroyed, so it
/// is only computed with vision (empty otherwise).
#[derive(Serialize)]
struct Pv1Roster<'a> {
    g: u32,
    t: u32,
    absent: Vec<&'a str>,
    gone: Vec<&'a str>,
}

/// The objects the scripted driver has RESERVED this tick — the bot's ordinary systems must not
/// act on them, or the capture measures the bot instead of the engine. Towers: the engine keeps
/// ONE tower intent per tick with `heal` over `repair` over `attack` (`towers/intents.js`), so a
/// `TowerMission` repair on a scripted tower silently replaces the script's shot (the first
/// tower-rampart recapture lost three of twelve shots that way). A World resource, refreshed by
/// [`run`] every tick (empty whenever the driver is off or tracing, so a stale reservation cannot
/// outlive a bed), read by `missions::tower`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParityReserved {
    /// Labelled tower tiles of the active scripted bed (`(room, x, y)`).
    pub tower_tiles: Vec<(RoomName, u8, u8)>,
}

impl ParityReserved {
    /// Is the tower standing at `pos` scripted this tick?
    pub fn reserves_tower(&self, pos: screeps::Position) -> bool {
        self.tower_tiles
            .iter()
            .any(|(room, x, y)| pos.room_name() == *room && pos.x().u8() == *x && pos.y().u8() == *y)
    }
}

/// Publish this tick's reservation (empty = nothing reserved).
fn publish_reserved(world: &mut World, reserved: ParityReserved) {
    let mut slot = world.entry::<ParityReserved>().or_insert_with(Default::default);
    if *slot != reserved {
        *slot = reserved;
    }
}

fn part_name(part: Part) -> &'static str {
    match part {
        Part::Move => "move",
        Part::Work => "work",
        Part::Carry => "carry",
        Part::Attack => "attack",
        Part::RangedAttack => "ranged_attack",
        Part::Tough => "tough",
        Part::Heal => "heal",
        Part::Claim => "claim",
        _ => "unknown",
    }
}

fn direction_from_name(name: &str) -> Option<Direction> {
    Some(match name {
        "top" => Direction::Top,
        "top_right" => Direction::TopRight,
        "right" => Direction::Right,
        "bottom_right" => Direction::BottomRight,
        "bottom" => Direction::Bottom,
        "bottom_left" => Direction::BottomLeft,
        "left" => Direction::Left,
        "top_left" => Direction::TopLeft,
        _ => return None,
    })
}

/// Run the driver for this tick. Called from the game loop before the systems (so the guarded-sink
/// records land in this tick's `IntentRecorder` digest and no job has consumed a creep's flags).
pub fn run(world: &mut World, features: &Features) {
    let flag = features.eval.parity_script;
    if flag.is_empty() {
        publish_reserved(world, ParityReserved::default());
        return;
    }
    let root = crate::memory_helper::root();
    let js = js_sys::Reflect::get(&root, &wasm_bindgen::JsValue::from_str(SCRIPT_MEMORY_KEY))
        .unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
    if js.is_undefined() || js.is_null() {
        warn!("parity: eval.parity_script={:?} but Memory.{} is absent", flag.as_str(), SCRIPT_MEMORY_KEY);
        return;
    }
    let script: ScriptMemory = match serde_wasm_bindgen::from_value(js) {
        Ok(s) => s,
        Err(e) => {
            warn!("parity: Memory.{} does not parse: {}", SCRIPT_MEMORY_KEY, e);
            return;
        }
    };
    if script.scenario != flag.as_str() {
        warn!(
            "parity: flag names {:?} but Memory.{} carries {:?} — driver idle",
            flag.as_str(),
            SCRIPT_MEMORY_KEY,
            script.scenario
        );
        return;
    }
    let now = game::time();
    let start = match script.start {
        Some(s) => s,
        None => {
            crate::memory_helper::path_set(&format!("{SCRIPT_MEMORY_KEY}.start"), now as f64);
            now
        }
    };
    if now < start {
        return; // the harness scheduled tick 0 in the future; both users wait for the same tick
    }
    let t = now - start;
    let Ok(room_name) = script.room.parse::<RoomName>() else {
        warn!("parity: bad room {:?}", script.room);
        return;
    };
    let prefix = format!("pv-{}-", script.scenario);

    // Reserve the scripted towers for the driver (none while tracing — the bot decides then).
    publish_reserved(
        world,
        ParityReserved {
            tower_tiles: if script.trace {
                Vec::new()
            } else {
                script.towers.iter().map(|l| (room_name, l.x, l.y)).collect()
            },
        },
    );

    // Visible scripted creeps: mine (from Game.creeps) + hostile (a room find), name-sorted.
    let mut visible: BTreeMap<String, (Creep, bool)> = BTreeMap::new();
    for creep in game::creeps().values() {
        let name = creep.name();
        if name.starts_with(&prefix) && creep.pos().room_name() == room_name {
            visible.insert(name, (creep, true));
        }
    }
    let room = game::rooms().get(room_name);
    let mut structures: Vec<StructureObject> = Vec::new();
    if let Some(room) = &room {
        for creep in room.find(find::HOSTILE_CREEPS, None) {
            let name = creep.name();
            if name.starts_with(&prefix) {
                visible.entry(name).or_insert((creep, false));
            }
        }
        structures = room.find(find::STRUCTURES, None);
    }
    // A label resolves to the structure standing on its tile; the towers table only matches towers
    // and the structures table only non-towers (a rampart can share a tower's tile).
    let at_label = |l: &Labeled, want_tower: bool| -> Option<StructureObject> {
        structures
            .iter()
            .find(|s| {
                s.pos().x().u8() == l.x
                    && s.pos().y().u8() == l.y
                    && matches!(s, StructureObject::StructureTower(_)) == want_tower
            })
            .cloned()
    };
    let structure_at = |label: &str| -> Option<StructureObject> {
        script.structures.iter().find(|l| l.id == label).and_then(|l| at_label(l, false))
    };
    let tower_at = |label: &str| -> Option<StructureObject> {
        script.towers.iter().find(|l| l.id == label).and_then(|l| at_label(l, true))
    };

    // 1. Execute this tick's script for MY creeps (unless tracing).
    let mut did: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if !script.trace {
        if let Some(tick) = script.ticks.iter().find(|s| s.t == t) {
            let mut recorder = world.entry::<IntentRecorder>().or_insert_with(Default::default);
            for si in &tick.intents {
                let Some((creep, true)) = visible.get(&si.creep) else {
                    continue; // not mine (the other user's bot executes it) or not visible
                };
                let mut flags = SimultaneousActionFlags::UNSET;
                let mut done = Vec::new();
                for a in &si.actions {
                    // Sink-routed actions yield `Option<bool>` — `None` = nothing to call on,
                    // `Some(issued)` = the sink's answer — and `sink_mark` turns that into the
                    // `did` suffix; the raw controller call carries the API's own verdict.
                    let suffix = match a {
                        ScriptAction::Attack { target } => sink_mark(
                            visible
                                .get(target)
                                .map(|(tc, _)| intents::attack(creep, &mut flags, &mut recorder, tc, tc.pos())),
                        ),
                        ScriptAction::RangedAttack { target } => sink_mark(
                            visible
                                .get(target)
                                .map(|(tc, _)| intents::ranged_attack(creep, &mut flags, &mut recorder, tc, tc.pos())),
                        ),
                        ScriptAction::RangedMassAttack => {
                            sink_mark(Some(intents::ranged_mass_attack(creep, &mut flags, &mut recorder)))
                        }
                        ScriptAction::Heal { target } => sink_mark(
                            visible
                                .get(target)
                                .map(|(tc, _)| intents::heal(creep, &mut flags, &mut recorder, tc, tc.pos())),
                        ),
                        ScriptAction::RangedHeal { target } => sink_mark(
                            visible
                                .get(target)
                                .map(|(tc, _)| intents::ranged_heal(creep, &mut flags, &mut recorder, tc, tc.pos())),
                        ),
                        ScriptAction::Dismantle { target } => sink_mark(structure_at(target).and_then(|s| {
                            s.as_dismantleable()
                                .map(|d| intents::dismantle(creep, &mut flags, &mut recorder, d, s.pos()))
                        })),
                        ScriptAction::AttackStructure { target } => {
                            sink_mark(structure_at(target).or_else(|| tower_at(target)).and_then(|s| {
                                s.as_attackable()
                                    .map(|a| intents::attack(creep, &mut flags, &mut recorder, a, s.pos()))
                            }))
                        }
                        ScriptAction::RangedAttackStructure { target } => {
                            sink_mark(structure_at(target).or_else(|| tower_at(target)).and_then(|s| {
                                s.as_attackable()
                                    .map(|a| intents::ranged_attack(creep, &mut flags, &mut recorder, a, s.pos()))
                            }))
                        }
                        // No sink category exists for attackController (it is not in the
                        // IntentRecorder's table); a raw call, like movement.
                        ScriptAction::AttackController => room
                            .as_ref()
                            .and_then(|r| r.controller())
                            .map(|c| mark(creep.attack_controller(&c)))
                            .unwrap_or_else(|| MISSING.into()),
                    };
                    done.push(format!("{}{suffix}", action_label(a)));
                }
                if let Some(dir) = &si.mv {
                    match direction_from_name(dir) {
                        Some(d) => {
                            done.push(format!("move:{dir}{}", mark(creep.move_direction(d))));
                        }
                        None => warn!("parity: unknown direction {dir:?} for {}", si.creep),
                    }
                }
                if let Some(target) = &si.pull {
                    let m = visible.get(target).map(|(tc, _)| mark(creep.pull(tc))).unwrap_or_else(|| MISSING.into());
                    done.push(format!("pull:{target}{m}"));
                }
                did.insert(si.creep.clone(), done);
            }
            for ts in &tick.towers {
                let Some(StructureObject::StructureTower(tower)) = tower_at(&ts.tower) else {
                    continue;
                };
                if !tower.my() {
                    continue;
                }
                // Raw game-API calls carry the API's verdict: `!<ErrorCode>` on rejection (e.g.
                // `!RclNotEnough` for a tower whose owner does not hold the room controller).
                let m = match &ts.action {
                    TowerScriptAction::Attack { target } => visible.get(target).map(|(tc, _)| mark(tower.attack(tc))),
                    TowerScriptAction::Heal { target } => visible.get(target).map(|(tc, _)| mark(tower.heal(tc))),
                    TowerScriptAction::Repair { target } => {
                        structure_at(target).and_then(|s| s.as_repairable().map(|r| mark(tower.repair(r))))
                    }
                }
                .unwrap_or_else(|| MISSING.into());
                did.entry(ts.tower.clone())
                    .or_default()
                    .push(format!("tower:{}{m}", tower_label(&ts.action)));
            }
        }
    }

    // 2. One PV1 line per visible scripted creep (tick-START state), then the roster line.
    for (name, (creep, mine)) in &visible {
        let pos = creep.pos();
        let parts = creep
            .body()
            .iter()
            .map(|p| {
                let boost = p
                    .boost()
                    .and_then(|b| serde_json::to_value(b).ok())
                    .and_then(|v| v.as_str().map(str::to_string));
                (part_name(p.part()), p.hits(), boost)
            })
            .collect();
        let line = Pv1Creep {
            g: now,
            t,
            n: name,
            mine: *mine,
            x: pos.x().u8(),
            y: pos.y().u8(),
            hits: creep.hits(),
            hits_max: creep.hits_max(),
            fatigue: creep.fatigue(),
            parts,
            did: did.remove(name).unwrap_or_default(),
        };
        if let Ok(json) = serde_json::to_string(&line) {
            info!("{PV1_MARKER}{json}");
        }
    }
    // 3. One line per labelled structure / tower still standing (tick-START hits), then the roster
    //    line. Without vision nothing is known about the structures, so `gone` stays empty rather
    //    than declaring every label destroyed.
    let mut gone: Vec<&str> = Vec::new();
    if room.is_some() {
        for l in &script.structures {
            match at_label(l, false) {
                Some(s) => {
                    let line = Pv1Structure {
                        g: now,
                        t,
                        s: &l.id,
                        hits: s.as_structure().hits(),
                    };
                    if let Ok(json) = serde_json::to_string(&line) {
                        info!("{PV1_MARKER}{json}");
                    }
                }
                None => gone.push(&l.id),
            }
        }
        for l in &script.towers {
            match at_label(l, true) {
                Some(StructureObject::StructureTower(tower)) => {
                    let line = Pv1Tower {
                        g: now,
                        t,
                        tw: &l.id,
                        hits: tower.hits(),
                        energy: tower.store().get_used_capacity(Some(ResourceType::Energy)),
                        did: did.remove(l.id.as_str()).unwrap_or_default(),
                    };
                    if let Ok(json) = serde_json::to_string(&line) {
                        info!("{PV1_MARKER}{json}");
                    }
                }
                _ => gone.push(&l.id),
            }
        }
    }
    let absent: Vec<&str> = script
        .roster
        .iter()
        .filter(|n| !visible.contains_key(n.as_str()))
        .map(String::as_str)
        .collect();
    if let Ok(json) = serde_json::to_string(&Pv1Roster { g: now, t, absent, gone }) {
        info!("{PV1_MARKER}{json}");
    }
    if room.is_none() {
        debug!("parity: no vision of {} at t={t}", script.room);
    }
}

/// The `did` suffix for a raw game-API call: empty when accepted, `!<ErrorCode>` when rejected
/// (the code's Debug name — `Tired`, `RclNotEnough`, `NotEnoughEnergy`, ...), so a capture says
/// WHY an intent never reached the engine.
fn mark<E: std::fmt::Debug>(r: Result<(), E>) -> String {
    match r {
        Ok(()) => String::new(),
        Err(e) => format!("!{e:?}"),
    }
}

/// The `did` suffix when the scripted target is not visible at all (no call was made).
const MISSING: &str = "!Missing";

/// The `did` suffix when the guarded intent sink refused the call because an earlier scripted
/// action on the same creep already took that simultaneous-action pipeline this tick (a script
/// defect — the engine keeps one action per pipeline; no call was made).
const PIPELINE_TAKEN: &str = "!PipelineTaken";

/// The `did` suffix for an action routed through the guarded sink (`crate::intents`), whose `bool`
/// says only whether it ISSUED the call — the sink discards the game API's own verdict — so the
/// suffix names the one reason the driver itself knows: `None` = the scripted target was not
/// there to call on (`!Missing`, as for towers), `Some(false)` = the sink refused
/// (`!PipelineTaken`), `Some(true)` = issued (empty, like an accepted raw call). Never a bare `!`.
fn sink_mark(issued: Option<bool>) -> String {
    match issued {
        Some(true) => String::new(),
        Some(false) => PIPELINE_TAKEN.into(),
        None => MISSING.into(),
    }
}

fn action_label(a: &ScriptAction) -> String {
    match a {
        ScriptAction::Attack { target } => format!("attack:{target}"),
        ScriptAction::RangedAttack { target } => format!("ranged_attack:{target}"),
        ScriptAction::RangedMassAttack => "ranged_mass_attack".into(),
        ScriptAction::Heal { target } => format!("heal:{target}"),
        ScriptAction::RangedHeal { target } => format!("ranged_heal:{target}"),
        ScriptAction::Dismantle { target } => format!("dismantle:{target}"),
        ScriptAction::AttackStructure { target } => format!("attack_structure:{target}"),
        ScriptAction::RangedAttackStructure { target } => format!("ranged_attack_structure:{target}"),
        ScriptAction::AttackController => "attack_controller".into(),
    }
}

fn tower_label(a: &TowerScriptAction) -> String {
    match a {
        TowerScriptAction::Attack { target } => format!("attack:{target}"),
        TowerScriptAction::Heal { target } => format!("heal:{target}"),
        TowerScriptAction::Repair { target } => format!("repair:{target}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `did` suffix is `!<Reason>` or empty — never a bare `!`: raw calls carry the API's
    /// error code, sink-routed actions carry the one reason the driver knows (no target / pipeline
    /// already taken). RED before: a sink-routed action with a dead target printed `attack:pv-x-1!`
    /// while the same situation on a tower printed `tower:attack:pv-x-1!Missing`.
    #[test]
    fn did_suffixes_are_never_a_bare_bang() {
        assert_eq!(mark::<screeps::ErrorCode>(Ok(())), "");
        assert_eq!(mark(Err(screeps::ErrorCode::Tired)), "!Tired");
        assert_eq!(mark(Err(screeps::ErrorCode::RclNotEnough)), "!RclNotEnough");
        assert_eq!(sink_mark(Some(true)), "");
        assert_eq!(sink_mark(None), MISSING);
        assert_eq!(sink_mark(None), "!Missing");
        assert_eq!(sink_mark(Some(false)), "!PipelineTaken");
        for s in [mark(Err(screeps::ErrorCode::Busy)), sink_mark(None), sink_mark(Some(false))] {
            assert!(s.len() > 1 && s.starts_with('!'), "{s:?}");
        }
    }

    /// The flag type: `Copy`, string-shaped in Memory, empty = off, over-cap names truncated at a
    /// char boundary (never a deserialize error, which would reset EVERY feature to default).
    #[test]
    fn scenario_name_is_copy_string_shaped_and_bounded() {
        let n = ParityScriptName::new("melee-1v1");
        assert_eq!(n.as_str(), "melee-1v1");
        assert!(!n.is_empty());
        assert!(ParityScriptName::default().is_empty());
        assert_eq!(serde_json::to_string(&n).unwrap(), "\"melee-1v1\"");
        let back: ParityScriptName = serde_json::from_str("\"kite-r3\"").unwrap();
        assert_eq!(back.as_str(), "kite-r3");
        let long = "x".repeat(SCENARIO_NAME_CAP + 10);
        assert_eq!(ParityScriptName::new(&long).as_str().len(), SCENARIO_NAME_CAP);
        let multi = format!("{}é", "a".repeat(SCENARIO_NAME_CAP - 1));
        assert_eq!(ParityScriptName::new(&multi).as_str(), &"a".repeat(SCENARIO_NAME_CAP - 1));
        let copied = n; // Copy
        assert_eq!(copied, n);
    }

    /// The exact wire shape the engine's `parity::TickScript` serializes to (pinned identically in
    /// `screeps-combat-engine/src/parity.rs` `script_wire_shape_is_pinned`) parses here.
    #[test]
    fn script_wire_shape_matches_the_engine_pin() {
        let wire = r#"{"scenario":"x","room":"W1N1","roster":["pv-x-0","pv-x-1"],"towers":[{"id":"t0","x":20,"y":25}],
            "ticks":[{"t":3,"intents":[{"creep":"pv-x-0","actions":[{"kind":"ranged_attack","target":"pv-x-1"},{"kind":"ranged_mass_attack"}],"move":"left"}],"towers":[{"tower":"t0","action":{"kind":"attack","target":"pv-x-1"}}]}]}"#;
        let m: ScriptMemory = serde_json::from_str(wire).unwrap();
        assert_eq!(m.scenario, "x");
        assert!(m.start.is_none() && !m.trace);
        assert_eq!(m.roster.len(), 2);
        assert_eq!(m.towers[0].id, "t0");
        let tick = &m.ticks[0];
        assert_eq!(tick.t, 3);
        assert_eq!(tick.intents[0].mv.as_deref(), Some("left"));
        assert!(tick.intents[0].pull.is_none());
        assert!(matches!(tick.intents[0].actions[1], ScriptAction::RangedMassAttack));
        assert!(matches!(&tick.towers[0].action, TowerScriptAction::Attack { target } if target == "pv-x-1"));
        assert_eq!(action_label(&tick.intents[0].actions[0]), "ranged_attack:pv-x-1");
        assert_eq!(tower_label(&tick.towers[0].action), "attack:pv-x-1");
        assert_eq!(direction_from_name("bottom_left"), Some(Direction::BottomLeft));
        assert_eq!(direction_from_name("up"), None);
        assert_eq!(part_name(Part::RangedAttack), "ranged_attack");
    }

    /// The reservation seam the tower mission honours: a scripted tower's tile is reserved, any
    /// other tile (or the same tile in another room) is not, and the default reserves nothing —
    /// the state the driver publishes whenever it is off or tracing.
    #[test]
    fn reserved_towers_match_by_room_and_tile_only() {
        use screeps::{Position, RoomCoordinate};
        let room: RoomName = "W9N7".parse().unwrap();
        let other: RoomName = "W9N8".parse().unwrap();
        let at = |r: RoomName, x: u8, y: u8| {
            Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), r)
        };
        let reserved = ParityReserved { tower_tiles: vec![(room, 20, 25)] };
        assert!(reserved.reserves_tower(at(room, 20, 25)));
        assert!(!reserved.reserves_tower(at(room, 21, 25)));
        assert!(!reserved.reserves_tower(at(other, 20, 25)));
        assert!(!ParityReserved::default().reserves_tower(at(room, 20, 25)));
    }

    /// The structure / tower / roster line shapes `screeps-ibex-eval::parity::parse_pv1` tells
    /// apart by their required keys (`s` / `tw` / `absent`+`gone`) — pinned here as the wire
    /// text so a rename on either side is loud.
    #[test]
    fn structure_tower_and_roster_lines_have_the_pinned_wire_shape() {
        let s = Pv1Structure { g: 100, t: 3, s: "r0", hits: 4400 };
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"g":100,"t":3,"s":"r0","hits":4400}"#);
        let tw = Pv1Tower { g: 100, t: 3, tw: "t0", hits: 3000, energy: 990, did: vec!["tower:attack:pv-x-1!RclNotEnough".into()] };
        assert_eq!(
            serde_json::to_string(&tw).unwrap(),
            r#"{"g":100,"t":3,"tw":"t0","hits":3000,"energy":990,"did":["tower:attack:pv-x-1!RclNotEnough"]}"#
        );
        let r = Pv1Roster { g: 100, t: 3, absent: vec!["pv-x-1"], gone: vec!["r0"] };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"g":100,"t":3,"absent":["pv-x-1"],"gone":["r0"]}"#
        );
    }
}
