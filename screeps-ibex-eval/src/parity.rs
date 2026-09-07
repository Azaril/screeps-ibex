//! The H5 sim-vs-server parity oracle — the **Docker-facing** half (ADR 0006 §B.4 layers 1 + 2;
//! WS-CLOSE lane (b3), decisions D5/D6/D8). The pure schema / replay / diff live in
//! `screeps_combat_engine::parity`; this module owns policy + orchestration:
//!
//! - the **scenario catalog** `parity/<name>.json` — golden vectors WITHOUT frames (initial world +
//!   script); `catalog_names` / `load_catalog`;
//! - `synth` — replay a catalog entry through the sim and write it as a **placeholder** vector into
//!   `screeps-combat-engine/tests/conformance/` (provenance says so; replaced by the first capture);
//! - `capture` (layer 1) — verify every owner runs the bot build (`verify_owner_code`), seed the
//!   catalog entry's creeps/structures into the warm private world through the kit's
//!   `cmd_insert_*` builders, switch every owner's runtime on (the server zeroes `users.active`
//!   for a user with no objects), hand the room controller to the tower owner for a tower bed
//!   (`utils.checkStructureAgainstController`), inject the script + the `eval.parity_script`
//!   flag into BOTH owners' Memory with one absolute start tick, run the kit capture on EVERY
//!   owner's console, merge the per-owner `did` into one trace (`LiveTrace::merge_owner`),
//!   refuse a trace whose script did not execute (`verify_script_executed`), put the room back
//!   (`BedCleanup`: the controller row as snapshotted, the seeded objects, and EVERY object not
//!   in the pre-bed snapshot — on success, failure and panic), cross-check the last frame against
//!   the DB, and write the vector with server provenance;
//! - `report` (layer 2) — the UNSCRIPTED bed: the same seeded world with the driver in `trace`
//!   mode (frames only; the bot's own systems decide), then the sim side = `run_engagement`
//!   (`IbexAgent` vs `IbexAgent`) from the first-contact frame over the identical world, diffed
//!   against the live frames and graded against `parity/parity-budget.json` (seeded REPORT-ONLY —
//!   ADR 0015's report-only → gating rule; promotion is an operator decision);
//! - `nightly` — every catalog entry's report in one command (D6: there is no CI; the operator
//!   schedules `parity nightly`; the `#[ignore]` test `parity_nightly_within_budget` is the lane).
//!
//! D8: none of these reset the world (`--keep-world` is the default here; `scenario`/`smoke` got
//! the same flag). Seeding is idempotent per scenario (the previous bed's objects are removed).

use anyhow::{anyhow, bail, Context, Result};
use futures_util::FutureExt;
use screeps_combat_engine::parity::{
    assess, build_world, diff, frame_of, replay, synthesize, BudgetVerdict, BuiltWorld, FrameCreep,
    FrameStructure, FrameTower, GoldenVector, Owner, ParityBudget, ParityDiff, Provenance,
    TowerScriptAction, VecCreep, VecFrame, VecPart, VecStructure, VecTower, SERVER_CAPTURE_PROVENANCE,
};
use screeps_combat_engine::CombatWorld;
use screeps_server_kit::capture::{self, ConsoleIdentity, ConsoleInjection, RunArtifacts};
use screeps_server_kit::config::{BotEndpoint, KitConfig};
use screeps_server_kit::server::{
    self, CliClient, CodeFingerprint, ControllerOwnership, LiveCreep, RoomObjectRef, SeedCreep, SeedPart, SeedStructure,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

/// The scenario catalog directory (`parity/<name>.json`).
pub const CATALOG_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/parity");
/// The layer-2 budget file.
pub const BUDGET_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/parity/parity-budget.json");
/// Where captured/synthesized vectors go, relative to the repo root.
pub const CONFORMANCE_DIR: &str = "screeps-combat-engine/tests/conformance";
/// The engine submodule, relative to the repo root (provenance commit).
pub const ENGINE_DIR: &str = "screeps-combat-engine";
/// Ticks between the injection and scenario tick 0 — clears the injection tick and the kit's
/// up-to-2 s sampling lag so both users see the flag before `start`.
pub const DEFAULT_LEAD_TICKS: u32 = 30;
/// Ticks captured past the scenario's end (the terminal frame + slack).
const TAIL_TICKS: u32 = 5;

// ───────────────────────────── catalog ─────────────────────────────

/// Catalog entry names (file stems), sorted; the budget file is not a scenario.
pub fn catalog_names() -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(CATALOG_DIR)
        .with_context(|| format!("reading catalog {CATALOG_DIR}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| s != "parity-budget")
        .collect();
    names.sort();
    Ok(names)
}

pub fn load_catalog(name: &str) -> Result<GoldenVector> {
    let path = Path::new(CATALOG_DIR).join(format!("{name}.json"));
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading catalog entry {}", path.display()))?;
    let v = GoldenVector::from_json(&raw).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    if v.scenario != name {
        bail!("{}: scenario field is {:?}, expected {name:?}", path.display(), v.scenario);
    }
    if v.creeps.is_empty() {
        bail!("{}: no creeps", path.display());
    }
    Ok(v)
}

pub fn load_budget() -> Result<ParityBudget> {
    let raw = std::fs::read_to_string(BUDGET_FILE)
        .with_context(|| format!("reading budget {BUDGET_FILE}"))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing budget {BUDGET_FILE}"))
}

/// Repo root for the Docker-free paths (the crate dir's parent).
pub fn repo_root_static() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives under the repo root")
        .to_path_buf()
}

fn repo_root(cfg: &KitConfig) -> Result<PathBuf> {
    cfg.source_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("config was not loaded from a file — cannot locate the repo root")
}

/// `git rev-parse --short HEAD` in `dir` (blocking; used for provenance stamps).
pub fn short_sha(dir: &Path) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Replay a catalog entry through the sim and write the PLACEHOLDER vector (D6: the lane is live
/// before the first Docker capture; the provenance says it is not server evidence).
pub fn synth(name: &str, out_dir: Option<&Path>) -> Result<PathBuf> {
    let root = repo_root_static();
    let v = load_catalog(name)?;
    let engine = short_sha(&root.join(ENGINE_DIR));
    let v = synthesize(v, &engine).map_err(|e| anyhow!("{name}: {e}"))?;
    let dir = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join(CONFORMANCE_DIR));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, v.to_json() + "\n").with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

// ───────────────────────────── PV1 console lines ─────────────────────────────

/// `[part, hits, boost]` as the driver prints it (front to back).
pub type Pv1Part = (String, u32, Option<String>);

/// One creep line printed by `screeps-ibex::eval_parity` (`PV1 {...}`).
#[derive(Debug, Clone, Deserialize)]
pub struct Pv1Creep {
    pub g: u32,
    pub t: u32,
    pub n: String,
    pub mine: bool,
    pub x: u8,
    pub y: u8,
    pub hits: u32,
    pub hits_max: u32,
    pub fatigue: u32,
    /// `[part, hits, boost]` front to back.
    pub parts: Vec<Pv1Part>,
    #[serde(default)]
    pub did: Vec<String>,
}

/// A labelled structure still standing this tick (`s` = the script label).
#[derive(Debug, Clone, Deserialize)]
pub struct Pv1Structure {
    pub g: u32,
    pub t: u32,
    pub s: String,
    pub hits: u32,
}

/// A labelled tower still standing this tick (`tw` = the script label). `did` = the tower intents
/// the printing bot issued for it (own towers only; a trailing `!<ErrorCode>` = the game API
/// rejected the call — e.g. `!RclNotEnough` for a tower in a room its owner does not hold).
#[derive(Debug, Clone, Deserialize)]
pub struct Pv1Tower {
    pub g: u32,
    pub t: u32,
    pub tw: String,
    pub hits: u32,
    pub energy: u32,
    #[serde(default)]
    pub did: Vec<String>,
}

/// The per-tick roster line (`absent` = scripted creeps not visible this tick; `gone` = labelled
/// structures/towers no longer standing at their tile).
#[derive(Debug, Clone, Deserialize)]
pub struct Pv1Roster {
    pub g: u32,
    pub t: u32,
    pub absent: Vec<String>,
    #[serde(default)]
    pub gone: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Pv1Line {
    Creep(Pv1Creep),
    Structure(Pv1Structure),
    Tower(Pv1Tower),
    Roster(Pv1Roster),
}

/// Parse a run's `console.jsonl` into the PV1 lines it carries (other lines ignored). The four
/// shapes are told apart by their required keys (`n` / `s` / `tw` / `absent`).
pub fn parse_pv1(console_jsonl: &str) -> Vec<Pv1Line> {
    let marker = crate::gates::PV1_MARKER;
    let mut out = Vec::new();
    for record in console_jsonl.lines() {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(record) else {
            continue;
        };
        let Some(line) = rec.get("line").and_then(|l| l.as_str()) else {
            continue;
        };
        let Some(idx) = line.find(marker) else {
            continue;
        };
        let json = &line[idx + marker.len()..];
        if let Ok(c) = serde_json::from_str::<Pv1Creep>(json) {
            out.push(Pv1Line::Creep(c));
        } else if let Ok(s) = serde_json::from_str::<Pv1Structure>(json) {
            out.push(Pv1Line::Structure(s));
        } else if let Ok(tw) = serde_json::from_str::<Pv1Tower>(json) {
            out.push(Pv1Line::Tower(tw));
        } else if let Ok(r) = serde_json::from_str::<Pv1Roster>(json) {
            out.push(Pv1Line::Roster(r));
        }
    }
    out
}

/// One scenario tick's lines, each keyed by name/label (so iteration is name-sorted).
#[derive(Debug, Default, Clone)]
pub struct TickLines {
    pub creeps: BTreeMap<String, Pv1Creep>,
    pub structures: BTreeMap<String, Pv1Structure>,
    pub towers: BTreeMap<String, Pv1Tower>,
}

/// The live trace: per scenario tick, the creep/structure/tower lines — the raw material for
/// frames and for the layer-2 first-contact world.
#[derive(Debug, Default, Clone)]
pub struct LiveTrace {
    pub ticks: BTreeMap<u32, TickLines>,
}

/// Every structure + tower label of a vector (the driver's `gone` roster domain), vector order.
pub fn structure_labels(v: &GoldenVector) -> Vec<String> {
    v.structures
        .iter()
        .map(|s| s.id.clone())
        .chain(v.towers.iter().map(|t| t.id.clone()))
        .collect()
}

impl LiveTrace {
    pub fn from_lines(lines: &[Pv1Line]) -> Self {
        let mut ticks: BTreeMap<u32, TickLines> = BTreeMap::new();
        for l in lines {
            match l {
                Pv1Line::Creep(c) => {
                    ticks.entry(c.t).or_default().creeps.insert(c.n.clone(), c.clone());
                }
                Pv1Line::Structure(s) => {
                    ticks.entry(s.t).or_default().structures.insert(s.s.clone(), s.clone());
                }
                Pv1Line::Tower(tw) => {
                    ticks.entry(tw.t).or_default().towers.insert(tw.tw.clone(), tw.clone());
                }
                Pv1Line::Roster(r) => {
                    ticks.entry(r.t).or_default();
                }
            }
        }
        Self { ticks }
    }

    /// Frames `0..=last` in the vector convention (the same one `replay()` emits): frame `t` = the
    /// tick-`t` lines — creeps, then the labelled structures/towers still standing; a roster creep
    /// present at `t-1` and absent at `t` died during `t-1`, and a label present at `t-1` and
    /// absent at `t` was destroyed during `t-1`. Missing ticks (a dropped console sample) are an
    /// error — a vector with holes is not byte-exact evidence.
    pub fn frames(&self, roster: &[String], labels: &[String], last: u32) -> Result<Vec<VecFrame>> {
        let mut frames = Vec::with_capacity(last as usize + 1);
        let mut prev_present: Vec<String> = roster.to_vec();
        let mut prev_standing: Vec<String> = labels.to_vec();
        for t in 0..=last {
            let Some(lines) = self.ticks.get(&t) else {
                bail!(
                    "live trace has no PV1 lines for scenario tick {t} (have {:?})",
                    self.ticks.keys().collect::<Vec<_>>()
                );
            };
            let present: Vec<String> = lines.creeps.keys().cloned().collect();
            let standing: Vec<String> = lines
                .structures
                .keys()
                .chain(lines.towers.keys())
                .cloned()
                .collect();
            let mut deaths: Vec<String> = prev_present
                .iter()
                .filter(|n| roster.contains(n) && !present.contains(n))
                .cloned()
                .collect();
            deaths.sort();
            let mut destroyed: Vec<String> = prev_standing
                .iter()
                .filter(|l| labels.contains(l) && !standing.contains(l))
                .cloned()
                .collect();
            destroyed.sort();
            if t > 0 {
                let f: &mut VecFrame = frames.last_mut().expect("t > 0");
                f.deaths = deaths;
                f.destroyed = destroyed;
            }
            let frame_creeps: Vec<FrameCreep> = lines
                .creeps
                .values()
                .map(|c| FrameCreep {
                    name: c.n.clone(),
                    x: c.x,
                    y: c.y,
                    hits: c.hits,
                    fatigue: c.fatigue,
                })
                .collect();
            let frame_structures: Vec<FrameStructure> = lines
                .structures
                .values()
                .map(|s| FrameStructure {
                    id: s.s.clone(),
                    hits: s.hits,
                })
                .collect();
            let frame_towers: Vec<FrameTower> = lines
                .towers
                .values()
                .map(|tw| FrameTower {
                    id: tw.tw.clone(),
                    energy: tw.energy,
                    hits: tw.hits,
                })
                .collect();
            frames.push(VecFrame {
                t,
                creeps: frame_creeps,
                structures: frame_structures,
                towers: frame_towers,
                ..Default::default()
            });
            prev_present = present;
            prev_standing = standing;
        }
        Ok(frames)
    }
}

impl LiveTrace {
    /// Merge a SECOND owner's console lines into this (the acting identity's) trace. Every bot in
    /// the bed prints the same tick-START state for every visible scripted creep, but `did` (the
    /// intents issued) only ever comes from the bot that OWNS the creep/tower — the server
    /// delivers a user's console to that user's socket alone, so without this merge the other
    /// owner's actors carry an empty `did` whether or not that bot acted, and a bed whose second
    /// owner has no code captures silently as "the other side stood still" (the first five
    /// captures did exactly that: kite-r3 / heal-race / tower-rampart). `mine` stays relative to
    /// the acting identity (player 0 — `first_contact` / `world_from_live` read it that way).
    ///
    /// The two views MUST agree on the state fields they both print (position / hits / fatigue /
    /// parts): a disagreement means the lines are not from the same tick and the evidence is not
    /// byte-exact — an error, not a warning.
    pub fn merge_owner(&mut self, owner: &str, lines: &[Pv1Line]) -> Result<()> {
        for l in lines {
            match l {
                Pv1Line::Creep(c) => {
                    let tick = self.ticks.entry(c.t).or_default();
                    match tick.creeps.get_mut(&c.n) {
                        Some(base) => {
                            let same = (base.x, base.y, base.hits, base.hits_max, base.fatigue) == (c.x, c.y, c.hits, c.hits_max, c.fatigue)
                                && base.parts == c.parts;
                            if !same {
                                bail!(
                                    "owner {owner:?} sees {} at t={} as ({},{}) {}/{}hp fatigue {} but the acting identity sees ({},{}) {}/{}hp fatigue {} — the two consoles are not describing the same tick",
                                    c.n, c.t, c.x, c.y, c.hits, c.hits_max, c.fatigue, base.x, base.y, base.hits, base.hits_max, base.fatigue
                                );
                            }
                            if c.mine {
                                base.did = c.did.clone();
                            }
                        }
                        None => {
                            let mut c = c.clone();
                            c.mine = false;
                            tick.creeps.insert(c.n.clone(), c);
                        }
                    }
                }
                Pv1Line::Structure(s) => {
                    let tick = self.ticks.entry(s.t).or_default();
                    match tick.structures.get(&s.s) {
                        Some(base) if base.hits != s.hits => bail!(
                            "owner {owner:?} sees structure {} at t={} with {} hits but the acting identity sees {}",
                            s.s, s.t, s.hits, base.hits
                        ),
                        Some(_) => {}
                        None => {
                            tick.structures.insert(s.s.clone(), s.clone());
                        }
                    }
                }
                Pv1Line::Tower(tw) => {
                    let tick = self.ticks.entry(tw.t).or_default();
                    match tick.towers.get_mut(&tw.tw) {
                        Some(base) => {
                            if (base.hits, base.energy) != (tw.hits, tw.energy) {
                                bail!(
                                    "owner {owner:?} sees tower {} at t={} with {} hits / {} energy but the acting identity sees {} / {}",
                                    tw.tw, tw.t, tw.hits, tw.energy, base.hits, base.energy
                                );
                            }
                            if !tw.did.is_empty() {
                                base.did = tw.did.clone();
                            }
                        }
                        None => {
                            tick.towers.insert(tw.tw.clone(), tw.clone());
                        }
                    }
                }
                Pv1Line::Roster(r) => {
                    self.ticks.entry(r.t).or_default();
                }
            }
        }
        Ok(())
    }
}

// ───────────────────────────── bed preconditions ─────────────────────────────

/// The scripted-execution gate: every scripted creep intent / tower action at a captured tick
/// whose actor was standing must show up as an ISSUED intent in the owning bot's `did`. An actor
/// with an empty `did` means its owner's driver never ran the script (no code deployed as that
/// identity, a stale build without the driver, a runtime the server switched off) — the frames
/// then do not describe the bed the script names, so the vector must not be written as server
/// evidence.
///
/// A `!<ErrorCode>` suffix means the game API refused the call (`!Missing` = the driver made no
/// call because the scripted target was not there — the dead-target tail of a script;
/// `!PipelineTaken` = the guarded intent sink refused a second action on one simultaneous-action
/// pipeline). For a CREEP that is script semantics the engine and the sim both model (a fatigued
/// creep's `move` → `ERR_TIRED`, the kite-r3 kiter's target resting on the swamp; a dead target
/// skipped on both sides), so it is returned as a warning and stays in the evidence — the
/// byte-exact replay is the judge. For a TOWER only `NotEnoughEnergy` is modelled
/// (the sim's `can_fire`); any other refusal — `RclNotEnough` for a tower whose owner does not
/// hold the room controller (the first tower-rampart capture), a missing target, a bare `!`
/// from a pre-error-code driver — is a bed defect and fails the capture.
pub fn verify_script_executed(v: &GoldenVector, trace: &LiveTrace, ticks: u32) -> Result<Vec<String>> {
    let owner_of_creep = |name: &str| -> String {
        v.creeps
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| v.owners.iter().find(|o| o.player == c.owner))
            .map(|o| o.user.clone())
            .unwrap_or_else(|| "?".into())
    };
    let owner_of_tower = |label: &str| -> String {
        v.towers
            .iter()
            .find(|t| t.id == label)
            .and_then(|t| v.owners.iter().find(|o| o.player == t.owner))
            .map(|o| o.user.clone())
            .unwrap_or_else(|| "?".into())
    };
    // A `did` entry the game API refused: `<label>!<ErrorCode>` (or a bare `!`).
    let refused = |d: &str| d.contains('!');
    // owner → actor → ticks with an empty did
    let mut silent: BTreeMap<String, BTreeMap<String, Vec<u32>>> = BTreeMap::new();
    // owner → refused entries (creeps: warned; towers: fatal unless the sim models the reason)
    let mut creep_refusals: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut tower_refusals: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in v.script.iter().filter(|s| s.t < ticks) {
        let Some(lines) = trace.ticks.get(&s.t) else { continue };
        for si in &s.intents {
            let Some(c) = lines.creeps.get(&si.creep) else { continue }; // dead / not standing
            if si.actions.is_empty() && si.mv.is_none() && si.pull.is_none() {
                continue;
            }
            if c.did.is_empty() {
                silent.entry(owner_of_creep(&si.creep)).or_default().entry(si.creep.clone()).or_default().push(s.t);
            }
            for d in c.did.iter().filter(|d| refused(d)) {
                creep_refusals.entry(owner_of_creep(&si.creep)).or_default().push(format!("{} t={} {d}", si.creep, s.t));
            }
        }
        for ts in &s.towers {
            let Some(tw) = lines.towers.get(&ts.tower) else { continue }; // destroyed
            if tw.did.is_empty() {
                silent.entry(owner_of_tower(&ts.tower)).or_default().entry(ts.tower.clone()).or_default().push(s.t);
            }
            // The script names its target; once that target is dead/destroyed the driver makes
            // no call (`!Missing`) — the sim skips the same way, so that one is evidence too.
            let target = match &ts.action {
                TowerScriptAction::Attack { target } | TowerScriptAction::Heal { target } | TowerScriptAction::Repair { target } => target,
            };
            let target_standing = lines.creeps.contains_key(target)
                || lines.structures.contains_key(target)
                || lines.towers.contains_key(target);
            for d in tw.did.iter().filter(|d| {
                refused(d) && !d.ends_with("!NotEnoughEnergy") && (!d.ends_with("!Missing") || target_standing)
            }) {
                tower_refusals.entry(owner_of_tower(&ts.tower)).or_default().push(format!("{} t={} {d}", ts.tower, s.t));
            }
        }
    }
    let warnings: Vec<String> = creep_refusals
        .iter()
        .map(|(owner, items)| {
            format!(
                "owner {owner:?}: the game API refused creep intents the script kept issuing ({}) — recorded as evidence (the engine and the sim both model the refusal; the replay decides)",
                items.join("; ")
            )
        })
        .collect();
    if silent.is_empty() && tower_refusals.is_empty() {
        return Ok(warnings);
    }
    let mut msg = String::new();
    for (owner, actors) in &silent {
        let list: Vec<String> = actors
            .iter()
            .map(|(a, ts)| format!("{a} (t={})", ts.iter().map(u32::to_string).collect::<Vec<_>>().join(",")))
            .collect();
        msg.push_str(&format!(
            "owner {owner:?} never executed its script — no intent issued for {} — is the bot deployed as that identity with the parity driver (`screeps-server-kit deploy --user {owner}`) and is its runtime active?\n",
            list.join(", ")
        ));
    }
    for (owner, items) in &tower_refusals {
        msg.push_str(&format!(
            "owner {owner:?}: the game API REJECTED scripted tower actions ({}) — the sim does not model that refusal (only NotEnoughEnergy), so the bed does not exercise what the script says (a tower needs its owner to hold the room controller at a level allowing towers)\n",
            items.join("; ")
        ));
    }
    bail!("{}", msg.trim_end())
}

/// The deployed-code gate: every bed owner must run a BOT build (the parity driver ships in the
/// bot's wasm — the same module set as the acting identity, not the kit's bootstrap `main`). A bed where the second owner only carries the bootstrap
/// `main` would otherwise capture silently with that side inert. Byte-identical builds are the
/// intent but not a hard requirement — two `deploy --user` runs from a tree being edited in
/// between differ in bytes — so a fingerprint mismatch is reported (warn) rather than refused;
/// whether the other build carries the driver is what the execution gate
/// ([`verify_script_executed`]) settles after the capture.
pub fn verify_owner_code(fps: &[CodeFingerprint], owners: &[(String, String)]) -> Result<Vec<String>> {
    let row = |user_id: &str| fps.iter().find(|f| f.user == user_id);
    let (primary_user, primary_id) = owners.first().context("a bed needs at least one owner")?;
    let primary = row(primary_id).with_context(|| format!("no code row for {primary_user:?}"))?;
    let Some(reference) = primary.fingerprint.as_deref() else {
        bail!("owner {primary_user:?} has no active-world code — deploy the bot first (`screeps-server-kit deploy --user {primary_user}`)");
    };
    if primary.modules.len() < 2 {
        bail!(
            "owner {primary_user:?} runs only {:?} — not a bot build (the parity driver ships in the wasm) — `screeps-server-kit deploy --user {primary_user}`",
            primary.modules
        );
    }
    let mut warnings = Vec::new();
    for (user, id) in owners {
        let f = row(id).with_context(|| format!("no code row for {user:?}"))?;
        let Some(fp) = f.fingerprint.as_deref() else {
            bail!(
                "owner {user:?} has no active-world code — the parity driver never runs for its creeps: `screeps-server-kit deploy --user {user}`"
            );
        };
        if f.modules != primary.modules {
            bail!(
                "owner {user:?} runs DIFFERENT code than {primary_user:?} (modules {:?} vs {:?}) — every bed owner must run the bot build: `screeps-server-kit deploy --user {user}`",
                f.modules, primary.modules
            );
        }
        // `active == 0` is NOT refused here: the driver zeroes it for any user with no objects and
        // the bed re-activates every owner right after seeding (`cmd_activate_user`).
        if fp != reference {
            warnings.push(format!(
                "owner {user:?} runs a different BUILD than {primary_user:?} (fingerprint {fp} vs {reference}) — same modules, so the driver is present if both were deployed from this tree; re-deploy both from one tree state if the difference matters"
            ));
        }
    }
    Ok(warnings)
}

/// The controller level that makes `n` towers ACTIVE for their owner — the engine's
/// `CONTROLLER_STRUCTURES.tower` ladder (3:1, 5:2, 7:3, 8:6); `utils.checkStructureAgainstController`
/// counts a tower active only when the room controller belongs to the tower's owner at a level
/// allowing that many towers. `None` when the bed has no towers.
pub fn rcl_for_towers(n: usize) -> Result<Option<u32>> {
    Ok(match n {
        0 => None,
        1 => Some(3),
        2 => Some(5),
        3 => Some(7),
        4..=6 => Some(8),
        _ => bail!("{n} towers exceed the RCL 8 allowance of 6"),
    })
}

/// The one owner every tower in the bed belongs to (a room controller has one holder, so a bed
/// cannot field active towers of two owners).
pub fn tower_owner(v: &GoldenVector) -> Result<Option<Owner>> {
    let mut players: Vec<u8> = v.towers.iter().map(|t| t.owner).collect();
    players.sort_unstable();
    players.dedup();
    match players.as_slice() {
        [] => Ok(None),
        [p] => v
            .owners
            .iter()
            .find(|o| o.player == *p)
            .cloned()
            .map(Some)
            .with_context(|| format!("tower owner player {p} is not in the scenario's owners")),
        many => bail!(
            "towers of {} different owners ({many:?}) — one room controller cannot activate towers for two users",
            many.len()
        ),
    }
}

// ───────────────────────────── injection ─────────────────────────────

fn js_str(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization is infallible")
}

/// The `Memory.parity_script` envelope the bot driver reads (`screeps-ibex::eval_parity`).
#[derive(Serialize)]
struct ScriptMemory<'a> {
    scenario: &'a str,
    room: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<u32>,
    trace: bool,
    roster: Vec<String>,
    structures: Vec<Labeled>,
    towers: Vec<Labeled>,
    ticks: &'a [screeps_combat_engine::parity::TickScript],
}

#[derive(Serialize)]
struct Labeled {
    id: String,
    x: u8,
    y: u8,
}

/// The JS that installs the script + arms the flag for one user, as a SEQUENCE of console
/// expressions: the server rejects any single expression over ~1000 characters ("expression size
/// is too large" — the same limit as MMO), and a script table is several KB, so the JSON is
/// streamed into `Memory._pv` in `ARM_CHUNK`-character pieces and parsed into `Memory.parity_script`
/// by the final expression, which also arms the flag. `start` is the absolute tick of scenario
/// tick 0 (both users get the same one); `trace` = frames only.
pub fn inject_expressions(v: &GoldenVector, room: &str, start: Option<u32>, trace: bool) -> Vec<String> {
    let mem = ScriptMemory {
        scenario: &v.scenario,
        room,
        start,
        trace,
        roster: v.roster(),
        structures: v
            .structures
            .iter()
            .map(|s| Labeled { id: s.id.clone(), x: s.x, y: s.y })
            .collect(),
        towers: v
            .towers
            .iter()
            .map(|t| Labeled { id: t.id.clone(), x: t.x, y: t.y })
            .collect(),
        ticks: &v.script,
    };
    let json = serde_json::to_string(&mem).expect("script memory serializes");
    let chars: Vec<char> = json.chars().collect();
    let mut out = vec!["Memory._pv=\"\";".to_owned()];
    for piece in chars.chunks(ARM_CHUNK) {
        let piece: String = piece.iter().collect();
        out.push(format!("Memory._pv+={};", js_str(&piece)));
    }
    out.push(format!(
        "Memory.parity_script=JSON.parse(Memory._pv);delete Memory._pv;{}",
        crate::scenario::feature_set("eval", &format!("parity_script={}", js_str(&v.scenario)))
    ));
    out
}

/// Raw JSON characters per arming chunk: escaped through `js_str` the piece stays well under the
/// console limit of ~1000 characters even when every character needs escaping.
const ARM_CHUNK: usize = 400;
/// Disarm: clear the flag and drop the script.
pub fn clear_expression() -> String {
    format!(
        "delete Memory.parity_script;{}",
        crate::scenario::feature_set("eval", "parity_script=\"\"")
    )
}

// ───────────────────────────── seeding ─────────────────────────────

fn seed_creeps_for(v: &GoldenVector, player: u8) -> Vec<SeedCreep> {
    v.creeps
        .iter()
        .filter(|c| c.owner == player)
        .map(|c| SeedCreep {
            name: c.name.clone(),
            x: c.x as u32,
            y: c.y as u32,
            body: c
                .parts
                .iter()
                .map(|p| SeedPart {
                    part: p.part.clone(),
                    boost: p.boost.clone(),
                })
                .collect(),
        })
        .collect()
}

fn seed_structures_for(v: &GoldenVector, player: u8) -> Vec<SeedStructure> {
    let mut out: Vec<SeedStructure> = v
        .structures
        .iter()
        .filter(|s| s.owner.map(|o| o == player).unwrap_or(player == 0))
        .map(|s| SeedStructure {
            kind: s.kind.clone(),
            x: s.x as u32,
            y: s.y as u32,
            hits: s.hits,
            hits_max: s.hits_max,
            owned: s.owner.is_some(),
            energy: 0,
        })
        .collect();
    out.extend(v.towers.iter().filter(|t| t.owner == player).map(|t| SeedStructure {
        kind: "tower".into(),
        x: t.x as u32,
        y: t.y as u32,
        hits: t.hits,
        hits_max: t.hits_max,
        owned: true,
        energy: t.energy,
    }));
    out
}

fn seeded_tiles(v: &GoldenVector) -> Vec<(u32, u32)> {
    v.structures
        .iter()
        .map(|s| (s.x as u32, s.y as u32))
        .chain(v.towers.iter().map(|t| (t.x as u32, t.y as u32)))
        .collect()
}

/// One owner's live identity: the kit bot entry + a signed-in client + the DB user id.
struct OwnerClient<'a> {
    owner: Owner,
    bot: &'a BotEndpoint,
    api: screeps_rest_api::Client,
    user_id: String,
}

async fn owner_clients<'a>(cfg: &'a KitConfig, v: &GoldenVector) -> Result<Vec<OwnerClient<'a>>> {
    if v.owners.is_empty() {
        bail!("scenario {} declares no owners (user ↔ player pairs)", v.scenario);
    }
    let mut out = Vec::new();
    for owner in &v.owners {
        let bot = cfg
            .bots
            .iter()
            .find(|b| b.name == owner.user)
            .ok_or_else(|| {
                anyhow!(
                    "scenario owner {:?} is not a `bots:` entry in config/local.yml (have {:?}) — \
                     the parity beds need every owner registered as a bot (README: bots: [private-server, ibex-2] — .screeps.yaml ENTRY names, not usernames)",
                    owner.user,
                    cfg.bots.iter().map(|b| b.name.as_str()).collect::<Vec<_>>()
                )
            })?;
        let api = screeps_server_kit::api::connect(&bot.endpoint).await?;
        let user_id = api.me().await?.id;
        out.push(OwnerClient {
            owner: owner.clone(),
            bot,
            api,
            user_id,
        });
    }
    Ok(out)
}

/// Options shared by `capture` and `report`.
#[derive(Debug, Clone)]
pub struct BedOptions {
    /// Override the catalog entry's room.
    pub room: Option<String>,
    /// Override the catalog entry's tick count (report beds usually run longer).
    pub ticks: Option<u32>,
    /// Skip `bootstrap --reset` (D8 — the default).
    pub keep_world: bool,
    /// Flag frozen rooms active and restart the stack before seeding (the guide's neutral-room
    /// freeze; the flag is only read at boot).
    pub activate_rooms: bool,
    pub lead_ticks: u32,
    /// Write the vector / report here instead of the default location.
    pub out: Option<PathBuf>,
}

impl Default for BedOptions {
    fn default() -> Self {
        Self {
            room: None,
            ticks: None,
            keep_world: true,
            activate_rooms: false,
            lead_ticks: DEFAULT_LEAD_TICKS,
            out: None,
        }
    }
}

/// What a seeded bed run leaves behind.
pub struct BedRun {
    pub vector: GoldenVector,
    pub room: String,
    pub run_dir: PathBuf,
    pub trace: LiveTrace,
    pub start_tick: u32,
    pub ticks: u32,
    /// The DB's creep rows at the end of the run (cross-check material).
    pub db_creeps: Vec<LiveCreep>,
}

// ───────────────────────────── bed cleanup ─────────────────────────────

/// The objects in `current` that were not in the pre-bed `snapshot` — everything the bed or a bot
/// created in the room while the bed ran: the seeded `pv-*` creeps/structures, and during a tower
/// bed's controller claim whatever the owning bot did with "its new colony" (the first tower-rampart
/// capture: 10 construction sites, a completed extension, containers). Identity is the DB `_id`;
/// order follows `current`. Objects that LEFT the room (a seeded creep that died) are not listed —
/// there is nothing to remove.
pub fn objects_not_in(snapshot: &[RoomObjectRef], current: &[RoomObjectRef]) -> Vec<RoomObjectRef> {
    let keep: BTreeSet<&str> = snapshot.iter().map(|o| o.id.as_str()).collect();
    current.iter().filter(|o| !keep.contains(o.id.as_str())).cloned().collect()
}

/// `type x count, ...` for a log line, type-sorted.
pub fn summarize_kinds(objects: &[RoomObjectRef]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for o in objects {
        *counts.entry(o.kind.as_str()).or_default() += 1;
    }
    counts.iter().map(|(k, n)| format!("{k}x{n}")).collect::<Vec<_>>().join(", ")
}

/// What `run_bed` does with the bed room's controller row it finds BEFORE claiming anything.
#[derive(Debug, PartialEq)]
pub enum StaleClaim<'a> {
    /// Nobody owns the room: nothing to release.
    Neutral,
    /// The row is a claim THIS harness made and never restored (the `parityClaim` marker with the
    /// claim's shape — an aborted run): put back the row the marker saved, then go on from it.
    Release { holder: &'a str, before: &'a ControllerOwnership },
    /// Somebody owns the room and it is not our claim: a tower bed refuses; a creep-only bed
    /// leaves the controller alone (it never needed it).
    Owned { holder: &'a str },
}

/// Only a row carrying the harness's own marker is ever released
/// ([`ControllerOwnership::parity_claim_before`]); an owned room without it is someone's colony.
pub fn stale_claim(row: &ControllerOwnership) -> StaleClaim<'_> {
    match (&row.user, row.parity_claim_before()) {
        (None, _) => StaleClaim::Neutral,
        (Some(holder), Some(before)) => StaleClaim::Release { holder, before },
        (Some(holder), None) => StaleClaim::Owned { holder },
    }
}

/// The refusal a tower bed gives an owned bed room (the text the operator sees).
pub fn owned_room_refusal(room: &str, holder: &str) -> String {
    format!(
        "bed room {room} is owned by user {holder} and carries no parity claim marker — a tower bed claims only a NEUTRAL room's controller and never touches a room a user holds (an aborted run's own claim is recognised by its `parityClaim` marker and released); pick a neutral bed room (`--room`) or release this one by hand"
    )
}

/// What a bed must put back when it ends — on success, on failure and on a panic in the bed body
/// (`run_bed` runs [`BedCleanup::run`] after `catch_unwind`): the controller row it claimed
/// (exactly as snapshotted), its seeded `pv-*` objects, and every object that appeared in the room
/// since the pre-bed snapshot.
struct BedCleanup {
    room: String,
    prefix: String,
    tiles: Vec<(u32, u32)>,
    /// `cmd_room_object_ids` of the room before the claim and the seeding.
    snapshot: Vec<RoomObjectRef>,
    /// `(claiming user id, the controller row before the claim)` once the claim is in.
    claimed: Option<(String, ControllerOwnership)>,
}

impl BedCleanup {
    /// Run every step even when one fails (the first failure is the returned error): the
    /// controller restore first — the owning bot stops treating the room as its colony the tick
    /// the row flips back — then the seeded objects, then the leftover report (what the bed did
    /// not seed but the room now holds), then everything not in the snapshot.
    async fn run(&self, cli: &CliClient) -> Result<()> {
        let mut first_err: Option<anyhow::Error> = None;
        let mut note = |label: &str, r: Result<String>| match r {
            Ok(reply) => tracing::info!("bed cleanup: {label}: {}", reply.trim()),
            Err(e) => {
                tracing::error!("bed cleanup: {label} FAILED: {e:#}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("bed cleanup: {label}")));
                }
            }
        };
        if let Some((owner_id, before)) = &self.claimed {
            note(
                "controller restored",
                cli.send(&server::cmd_restore_controller(&self.room, owner_id, before)).await,
            );
        }
        note(
            "seeded objects removed",
            cli.send(&server::cmd_remove_seeded(&self.room, &self.prefix, &self.tiles)).await,
        );
        match cli
            .send(&server::cmd_room_object_ids(&self.room))
            .await
            .and_then(|body| server::parse_room_object_ids(&body))
        {
            Ok(now) => {
                let extra = objects_not_in(&self.snapshot, &now);
                if !extra.is_empty() {
                    tracing::warn!(
                        "{}: {} object(s) the bed did not seed appeared during the run ({}) — removing them",
                        self.room,
                        extra.len(),
                        summarize_kinds(&extra)
                    );
                }
            }
            Err(e) => tracing::warn!("bed cleanup: leftover report skipped: {e:#}"),
        }
        let keep: Vec<String> = self.snapshot.iter().map(|o| o.id.clone()).collect();
        note(
            "room put back to the pre-bed snapshot",
            cli.send(&server::cmd_remove_objects_not_in(&self.room, &keep)).await,
        );
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Bring the stack up (no reset unless asked), seed the bed, arm both owners, capture.
async fn run_bed(cfg: &KitConfig, v: &GoldenVector, opts: &BedOptions, trace_only: bool) -> Result<BedRun> {
    let room = opts.room.clone().unwrap_or_else(|| v.room.clone());
    let ticks = opts.ticks.unwrap_or_else(|| v.tick_count());
    if ticks == 0 {
        bail!("scenario {} resolves zero ticks", v.scenario);
    }

    tracing::info!("parity bed '{}' 1/5: server up", v.scenario);
    screeps_server_kit::docker::up(&cfg.stack).await?;
    if !opts.keep_world {
        tracing::info!("parity bed 2/5: bootstrap --reset (world wipe requested)");
        let outcome = server::bootstrap(cfg, true).await?;
        tracing::info!("bootstrap complete:\n{outcome}");
    } else {
        tracing::info!("parity bed 2/5: keeping the warm world (D8)");
    }

    let cli = CliClient::new(cfg.stack.cli_port)?;
    let owners = owner_clients(cfg, v).await?;

    // Precondition: every owner runs the acting identity's build with a live runtime — the
    // parity driver ships in the bot, so an owner without it stands still all capture long.
    let ids: Vec<String> = owners.iter().map(|oc| oc.user_id.clone()).collect();
    let fps = server::parse_code_fingerprints(&cli.send(&server::cmd_code_fingerprints(&ids)).await?)?;
    let pairs: Vec<(String, String)> = owners.iter().map(|oc| (oc.owner.user.clone(), oc.user_id.clone())).collect();
    for w in verify_owner_code(&fps, &pairs)? {
        tracing::warn!("{w}");
    }
    tracing::info!(
        "owners run the bot build: {}",
        fps.iter().map(|f| format!("{} fp={}", f.user, f.fingerprint.as_deref().unwrap_or("-"))).collect::<Vec<_>>().join(", ")
    );

    tracing::info!("parity bed 3/5: seed {room}");
    server::pause(&cli).await?;
    let prefix = format!("pv-{}-", v.scenario);
    let removed = cli.send(&server::cmd_remove_seeded(&room, &prefix, &seeded_tiles(v))).await?;
    tracing::info!("cleanup: {}", removed.trim());
    // A controller claim an aborted run of THIS harness left behind is released now (every bed,
    // tower or not — the marker is ours); any other owned controller is left alone.
    let mut controller = server::parse_controller_ownership(&cli.send(&server::cmd_controller_ownership(&room)).await?)?;
    let tower_owner = tower_owner(v)?;
    if let Some(row) = &controller {
        match stale_claim(row) {
            StaleClaim::Neutral => {}
            StaleClaim::Release { holder, before } => {
                tracing::warn!("{room} still carries this harness's claim marker (held by {holder} since an aborted run) — restoring the controller row it saved");
                let r = cli.send(&server::cmd_restore_controller(&room, holder, before)).await?;
                tracing::info!("{}", r.trim());
                controller = Some(before.clone());
            }
            StaleClaim::Owned { holder } => {
                if tower_owner.is_some() {
                    bail!("{}", owned_room_refusal(&room, holder));
                }
                tracing::info!("{room} is owned by user {holder}; a creep-only bed leaves the controller alone");
            }
        }
    }
    // The pre-bed snapshot: the set of objects the room goes back to when the bed ends. Taken
    // after the stale-seed removal (so a previous bed's leftovers do not get "kept") and before
    // the claim (so nothing the owning bot builds in its ~60-tick "colony" survives).
    let snapshot = server::parse_room_object_ids(&cli.send(&server::cmd_room_object_ids(&room)).await?)?;
    if snapshot.is_empty() {
        bail!("bed room {room} reports no objects at all (a room always has its controller/sources) — refusing to run a bed whose end-of-bed cleanup would have nothing to keep");
    }
    tracing::info!("{room}: pre-bed snapshot of {} objects ({})", snapshot.len(), summarize_kinds(&snapshot));
    let mut cleanup = BedCleanup {
        room: room.clone(),
        prefix,
        tiles: seeded_tiles(v),
        snapshot,
        claimed: None,
    };
    // From here to the end of the capture the world is dirty (claim + seeds + whatever the bots
    // do): the body runs under `catch_unwind` so the cleanup runs whether it returns, fails or
    // panics, and a panic is re-raised afterwards.
    let ctx = BedContext {
        cfg,
        v,
        opts,
        trace_only,
        cli: &cli,
        owners: &owners,
        room: &room,
        ticks,
    };
    let body = AssertUnwindSafe(bed_body(&ctx, tower_owner, controller, &mut cleanup))
        .catch_unwind()
        .await;
    let put_back = cleanup.run(&cli).await;
    let (artifacts, start, db_creeps) = match body {
        Err(panic) => std::panic::resume_unwind(panic),
        Ok(body) => body?,
    };
    put_back?;

    let console = std::fs::read_to_string(artifacts.dir.join("console.jsonl"))
        .with_context(|| format!("reading {}", artifacts.dir.join("console.jsonl").display()))?;
    let mut trace = LiveTrace::from_lines(&parse_pv1(&console));
    for oc in &owners[1..] {
        let path = capture::extra_console_path(&artifacts.dir, &oc.owner.user);
        let console = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let lines = parse_pv1(&console);
        tracing::info!("{}: {} PV1 lines", oc.owner.user, lines.len());
        trace.merge_owner(&oc.owner.user, &lines)?;
    }
    if !trace_only {
        for w in verify_script_executed(v, &trace, ticks)? {
            tracing::warn!("{w}");
        }
    }
    Ok(BedRun {
        vector: v.clone(),
        room,
        run_dir: artifacts.dir,
        trace,
        start_tick: start,
        ticks,
        db_creeps,
    })
}

/// What [`bed_body`] works with (all borrowed from `run_bed`).
#[derive(Clone, Copy)]
struct BedContext<'a> {
    cfg: &'a KitConfig,
    v: &'a GoldenVector,
    opts: &'a BedOptions,
    trace_only: bool,
    cli: &'a CliClient,
    owners: &'a [OwnerClient<'a>],
    room: &'a str,
    ticks: u32,
}

/// The dirty part of a bed — claim, seed, arm, capture, disarm, read the end-of-run creep rows —
/// run under `run_bed`'s cleanup guard. Returns the capture artifacts, the absolute start tick
/// and the DB creep rows (read BEFORE the cleanup removes them: the cross-check material).
async fn bed_body(
    ctx: &BedContext<'_>,
    tower_owner: Option<Owner>,
    controller: Option<ControllerOwnership>,
    cleanup: &mut BedCleanup,
) -> Result<(RunArtifacts, u32, Vec<LiveCreep>)> {
    let BedContext {
        cfg,
        v,
        opts,
        trace_only,
        cli,
        owners,
        room,
        ticks,
    } = *ctx;
    // Tower beds: the engine only lets a tower act when its owner holds the room controller at a
    // level allowing towers (`utils.checkStructureAgainstController` → `ERR_RCL_NOT_ENOUGH`
    // otherwise), so the bed room's controller is handed to the tower owner for the capture and
    // the snapshotted row put back by the cleanup.
    if let Some(tower_owner) = tower_owner {
        let level = rcl_for_towers(v.towers.len())?.expect("towers present");
        let owner_id = owners
            .iter()
            .find(|oc| oc.owner.player == tower_owner.player)
            .map(|oc| oc.user_id.clone())
            .expect("tower owner is a scenario owner");
        let before = controller.with_context(|| format!("bed room {room} has no controller — a tower bed needs one"))?;
        let r = cli.send(&server::cmd_claim_controller(room, &owner_id, level, &before)).await?;
        tracing::info!("{}: {} (controller was {:?}; restored by the cleanup)", tower_owner.user, r.trim(), before);
        cleanup.claimed = Some((owner_id, before));
    }
    for oc in owners {
        let creeps = seed_creeps_for(v, oc.owner.player);
        if !creeps.is_empty() {
            let r = cli.send(&server::cmd_insert_creeps(&oc.user_id, room, &creeps)).await?;
            tracing::info!("{}: creeps {}", oc.owner.user, r.trim());
        }
        let structures = seed_structures_for(v, oc.owner.player);
        if !structures.is_empty() {
            let r = cli.send(&server::cmd_insert_structures(&oc.user_id, room, &structures)).await?;
            tracing::info!("{}: structures {}", oc.owner.user, r.trim());
        }
    }
    // Every owner's runtime back on: the server's driver zeroes `users.active` (and builds no
    // runtime) for a user with no room objects, and only a code upload sets it again — a
    // bed-only identity is off between beds and stays off when its seeded creeps appear, so
    // its driver would never execute the script (the first captures' silent second owner).
    for oc in owners {
        let r = cli.send(&server::cmd_activate_user(&oc.user_id)).await?;
        tracing::info!("{}: runtime {}", oc.owner.user, r.trim());
    }
    if opts.activate_rooms {
        let r = cli.send(&server::cmd_activate_rooms()).await?;
        tracing::info!("activate rooms: {} — restarting the stack (flag is read at boot)", r.trim());
        screeps_server_kit::docker::down().await?;
        screeps_server_kit::docker::up(&cfg.stack).await?;
    }
    server::resume(cli).await?;

    tracing::info!("parity bed 4/5: arm both owners (lead {} ticks)", opts.lead_ticks);
    let now = owners[0].api.game_time().await?.time as u32;
    let start = now + opts.lead_ticks;
    let exprs = inject_expressions(v, room, Some(start), trace_only);
    for oc in owners {
        for expr in &exprs {
            oc.api
                .console(expr)
                .await
                .with_context(|| format!("arming {}", oc.owner.user))?;
        }
    }

    tracing::info!("parity bed 5/5: capture {} ticks from {start}", ticks);
    let mut spec = crate::gates::capture_spec();
    // Disarm at the end so the next ordinary run does not carry the driver.
    spec.console_injections = vec![ConsoleInjection {
        at_observed_tick: (opts.lead_ticks + ticks + TAIL_TICKS) as u64,
        expression: clear_expression(),
        label: "parity driver off".into(),
    }];
    let label = if trace_only { "parity-bed" } else { "parity-capture" };
    // Every owner's console is recorded: each bot prints `did` only for ITS OWN actors.
    let extra: Vec<ConsoleIdentity<'_>> = owners[1..]
        .iter()
        .map(|oc| ConsoleIdentity {
            label: &oc.owner.user,
            endpoint: &oc.bot.endpoint,
        })
        .collect();
    let captured = capture::run_with_consoles(
        cfg,
        (opts.lead_ticks + ticks + TAIL_TICKS + 2) as u64,
        &format!("{label}-{}", v.scenario),
        &spec,
        &extra,
    )
    .await;
    for oc in owners {
        let _ = oc.api.console(&clear_expression()).await;
    }
    let artifacts = captured?;
    // The DB cross-check wants the bed's creeps still standing, so their rows are read here,
    // before the cleanup takes the room back to its snapshot — a bed must not leave ANYTHING
    // behind for the next one (a leftover tower-rampart tower at (20,25) blocked every later
    // kite-r3 kiter's last step and looked like a movement divergence).
    let db_creeps = server::parse_room_creeps(&cli.send(&server::cmd_room_creeps(room)).await?)
        .unwrap_or_default();
    Ok((artifacts, start, db_creeps))
}

/// Layer 1: capture a scripted golden vector from the live server and write it into the
/// conformance directory (replacing any placeholder of the same name).
pub async fn capture(cfg: &KitConfig, name: &str, opts: &BedOptions) -> Result<PathBuf> {
    let root = repo_root(cfg)?;
    let v = load_catalog(name)?;
    let bed = run_bed(cfg, &v, opts, false).await?;
    let frames = bed.trace.frames(&v.roster(), &structure_labels(&v), bed.ticks)?;

    // DB cross-check of the terminal frame (best effort — the console lines are the vector).
    let last = frames.last().expect("ticks > 0");
    for c in &bed.db_creeps {
        if let Some(f) = last.creeps.iter().find(|f| f.name == c.name) {
            if (f.x as u32, f.y as u32, f.hits) != (c.x, c.y, c.hits) {
                tracing::warn!(
                    "DB cross-check: {} console ({},{}) {}hp vs db ({},{}) {}hp — ticks advanced past the capture",
                    c.name, f.x, f.y, f.hits, c.x, c.y, c.hits
                );
            }
        }
    }

    let cli = CliClient::new(cfg.stack.cli_port)?;
    let snapshot = server::parse_room_snapshot(&cli.send(&server::cmd_room_snapshot(&bed.room)).await?)?;
    let mut out = v;
    out.room = bed.room.clone();
    out.terrain = snapshot.terrain.unwrap_or_default();
    out.ticks = bed.ticks;
    out.frames = frames;
    out.provenance = Provenance {
        source: SERVER_CAPTURE_PROVENANCE.to_string(),
        engine_commit: short_sha(&root.join(ENGINE_DIR)),
        image_tag: cfg.stack.image.name.clone(),
        tick_ms: cfg.stack.tick_ms,
        captured_at: chrono::Utc::now().to_rfc3339(),
    };
    let dir = opts
        .out
        .clone()
        .unwrap_or_else(|| root.join(CONFORMANCE_DIR));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, out.to_json() + "\n")?;
    tracing::info!(
        "captured {} frames from {} (start tick {}) → {}",
        out.frames.len(),
        bed.run_dir.display(),
        bed.start_tick,
        path.display()
    );
    // Immediate verdict: does the sim reproduce what was just captured?
    let d = screeps_combat_engine::parity::check(&out).map_err(|e| anyhow!("{e}"))?;
    println!("{}", d.summary());
    Ok(path)
}

// ───────────────────────────── layer 2 ─────────────────────────────

/// The layer-2 artifact (`report.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParityReport {
    pub v: u32,
    pub scenario: String,
    pub run_dir: String,
    pub git_sha: String,
    pub engine_commit: String,
    /// Scenario tick the sim was seeded from (first frame where both sides are within range 3).
    pub contact_tick: u32,
    pub ticks_compared: u32,
    pub budget: ParityBudget,
    pub verdict: BudgetVerdict,
    pub diff: ParityDiff,
    pub live_frames: Vec<VecFrame>,
    pub sim_frames: Vec<VecFrame>,
}

/// First scenario tick at which a `mine` and a non-`mine` creep are within Chebyshev range 3 —
/// where the tactical decision starts mattering. `None` when the sides never meet.
pub fn first_contact(trace: &LiveTrace) -> Option<u32> {
    trace.ticks.iter().find_map(|(t, lines)| {
        let mine: Vec<&Pv1Creep> = lines.creeps.values().filter(|c| c.mine).collect();
        let theirs: Vec<&Pv1Creep> = lines.creeps.values().filter(|c| !c.mine).collect();
        let met = mine.iter().any(|a| {
            theirs
                .iter()
                .any(|b| a.x.abs_diff(b.x).max(a.y.abs_diff(b.y)) <= 3)
        });
        met.then_some(*t)
    })
}

/// Build the sim's initial world from a live tick's lines — creeps as observed (parts/hits/boost/
/// fatigue; `mine` → player 0, the rest → player 1) and the labelled structures/towers still
/// standing (kind/owner/tile/hits_max from the catalog entry, hits/energy as observed) — through
/// the engine's own `build_world`, so the layer-2 sim frames carry the same structure/tower fields
/// the live frames do.
pub fn world_from_live(tick: &TickLines, catalog: &GoldenVector, room: &str, terrain: &str) -> Result<BuiltWorld> {
    let creeps: Vec<VecCreep> = tick
        .creeps
        .values()
        .map(|c| VecCreep {
            name: c.n.clone(),
            owner: if c.mine { 0 } else { 1 },
            x: c.x,
            y: c.y,
            parts: c
                .parts
                .iter()
                .map(|(p, h, b)| VecPart {
                    part: p.clone(),
                    hits: *h,
                    boost: b.clone(),
                })
                .collect(),
            hits: Some(c.hits),
            fatigue: c.fatigue,
        })
        .collect();
    let structures: Vec<VecStructure> = tick
        .structures
        .values()
        .map(|s| {
            let cat = catalog
                .structures
                .iter()
                .find(|c| c.id == s.s)
                .ok_or_else(|| anyhow!("live structure label {:?} is not in the {} catalog entry", s.s, catalog.scenario))?;
            Ok(VecStructure {
                hits: s.hits,
                ..cat.clone()
            })
        })
        .collect::<Result<_>>()?;
    let towers: Vec<VecTower> = tick
        .towers
        .values()
        .map(|tw| {
            let cat = catalog
                .towers
                .iter()
                .find(|c| c.id == tw.tw)
                .ok_or_else(|| anyhow!("live tower label {:?} is not in the {} catalog entry", tw.tw, catalog.scenario))?;
            Ok(VecTower {
                hits: tw.hits,
                energy: tw.energy,
                ..cat.clone()
            })
        })
        .collect::<Result<_>>()?;
    let derived = GoldenVector {
        v: catalog.v,
        scenario: catalog.scenario.clone(),
        room: room.to_string(),
        terrain: terrain.to_string(),
        creeps,
        structures,
        towers,
        safe_mode_owner: catalog.safe_mode_owner,
        ..Default::default()
    };
    build_world(&derived).map_err(|e| anyhow!("{}: {e}", catalog.scenario))
}

/// Run the bot's real agent (`IbexAgent`) on both sides from `built` for up to `ticks` (stopping
/// early once a side is gone, like `run_engagement`) and return frames in the vector convention
/// numbered from `first_t` — the terminal frame included, which is why this is its own loop over
/// the agent crate's public per-tick pieces rather than `run_engagement` (whose recording has no
/// post-tick world to snapshot). Frames are the engine's `frame_of` (creeps + standing
/// structures/towers, name-sorted) plus the tick's deaths and destroyed labels.
pub fn sim_frames(mut built: BuiltWorld, first_t: u32, ticks: u32) -> Vec<VecFrame> {
    use screeps_combat_agent::opponents::tower_intents;
    use screeps_combat_agent::{agent_intents, IbexAgent, SimView};
    use screeps_combat_engine::resolve_tick;
    let room = built.room;
    let center = |w: &CombatWorld, owner: u8| {
        w.movement
            .creeps
            .iter()
            .find(|c| c.owner == owner)
            .map(|c| c.pos)
            .unwrap_or_else(|| {
                screeps::Position::new(
                    screeps::RoomCoordinate::new(25).unwrap(),
                    screeps::RoomCoordinate::new(25).unwrap(),
                    room,
                )
            })
    };
    let gone = |w: &CombatWorld, owner: u8| {
        !w.movement.creeps.iter().any(|c| c.owner == owner)
            && !w.towers.iter().any(|t| t.is_alive() && t.owner == owner)
    };
    let (ca, cb) = (center(&built.world, 0), center(&built.world, 1));
    let mut frames = Vec::with_capacity(ticks as usize + 1);
    for i in 0..ticks {
        if gone(&built.world, 0) || gone(&built.world, 1) {
            break;
        }
        let mut frame = frame_of(&built, first_t + i);
        let sva = SimView::from_world(&built.world, 0, ca, room);
        let mut intents = agent_intents(&built.world, &sva, &mut IbexAgent);
        let svb = SimView::from_world(&built.world, 1, cb, room);
        let ib = agent_intents(&built.world, &svb, &mut IbexAgent);
        intents.creeps.extend(ib.creeps);
        intents.moves.extend(ib.moves);
        tower_intents(&built.world, &mut intents);
        let report = resolve_tick(&mut built.world, &intents);
        let mut deaths: Vec<String> = report
            .deaths
            .iter()
            .filter_map(|id| built.creep_names.get(id).cloned())
            .collect();
        deaths.sort();
        let mut destroyed: Vec<String> = report
            .destroyed_structures
            .iter()
            .filter_map(|id| built.structure_names.get(id).cloned())
            .collect();
        destroyed.sort();
        frame.deaths = deaths;
        frame.destroyed = destroyed;
        frames.push(frame);
    }
    let last_t = first_t + frames.len() as u32;
    frames.push(frame_of(&built, last_t));
    frames
}

/// What `grade` produces: the report's core fields.
#[derive(Debug, Clone)]
pub struct GradeOutcome {
    /// Scenario tick the sim was seeded from (first frame where both sides are within range 3).
    pub contact_tick: u32,
    /// Live frames from contact to the end of the trace.
    pub live_frames: Vec<VecFrame>,
    /// Sim frames from contact (stops early once a side is gone).
    pub sim_frames: Vec<VecFrame>,
    pub diff: ParityDiff,
    pub verdict: BudgetVerdict,
}

/// Grade a live trace against the sim from first contact (pure; the report's core). `catalog`
/// supplies the roster + structure labels and the structure/tower kinds the live lines lack.
pub fn grade(
    trace: &LiveTrace,
    catalog: &GoldenVector,
    ticks: u32,
    room: &str,
    terrain: &str,
    budget: &ParityBudget,
) -> Result<GradeOutcome> {
    let scenario = &catalog.scenario;
    let live = trace.frames(&catalog.roster(), &structure_labels(catalog), ticks)?;
    let contact = first_contact(trace)
        .ok_or_else(|| anyhow!("{scenario}: the sides never came within range 3 — no engagement to compare"))?;
    let seed = trace
        .ticks
        .get(&contact)
        .ok_or_else(|| anyhow!("contact tick {contact} has no lines"))?;
    let built = world_from_live(seed, catalog, room, terrain)?;
    let remaining = ticks.saturating_sub(contact);
    let sim = sim_frames(built, contact, remaining);
    let live_from_contact: Vec<VecFrame> = live.iter().filter(|f| f.t >= contact).cloned().collect();
    // The sim stops early when a side is gone; compare only the overlap it produced.
    let live_overlap: Vec<VecFrame> = live_from_contact.iter().take(sim.len()).cloned().collect();
    let d = diff(&live_overlap, &sim);
    let verdict = assess(&d, budget);
    Ok(GradeOutcome {
        contact_tick: contact,
        live_frames: live_from_contact,
        sim_frames: sim,
        diff: d,
        verdict,
    })
}

/// Layer 2: the unscripted bed (or an existing `--run` dir) graded against the budget.
pub async fn report(
    cfg: &KitConfig,
    name: &str,
    run: Option<&Path>,
    opts: &BedOptions,
) -> Result<PathBuf> {
    let root = repo_root(cfg)?;
    let v = load_catalog(name)?;
    let budget = load_budget()?;
    let (trace, run_dir, room, ticks, terrain) = match run {
        Some(dir) => {
            let console = std::fs::read_to_string(dir.join("console.jsonl"))
                .with_context(|| format!("reading {}", dir.join("console.jsonl").display()))?;
            let trace = LiveTrace::from_lines(&parse_pv1(&console));
            let ticks = opts.ticks.unwrap_or_else(|| trace.ticks.keys().max().copied().unwrap_or(0));
            (trace, dir.to_path_buf(), opts.room.clone().unwrap_or_else(|| v.room.clone()), ticks, v.terrain.clone())
        }
        None => {
            let bed = run_bed(cfg, &v, opts, true).await?;
            let cli = CliClient::new(cfg.stack.cli_port)?;
            let snapshot = server::parse_room_snapshot(&cli.send(&server::cmd_room_snapshot(&bed.room)).await?)?;
            let terrain = snapshot.terrain.unwrap_or_default();
            (bed.trace, bed.run_dir, bed.room, bed.ticks, terrain)
        }
    };
    let GradeOutcome {
        contact_tick: contact,
        live_frames: live,
        sim_frames: sim,
        diff: d,
        verdict,
    } = grade(&trace, &v, ticks, &room, &terrain, &budget)?;
    let sha = short_sha(&root);
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let out_dir = opts
        .out
        .clone()
        .unwrap_or_else(|| root.join("runs").join(format!("parity-{sha}-{stamp}")));
    std::fs::create_dir_all(&out_dir)?;
    let rep = ParityReport {
        v: 1,
        scenario: name.to_string(),
        run_dir: run_dir.display().to_string(),
        git_sha: sha,
        engine_commit: short_sha(&root.join(ENGINE_DIR)),
        contact_tick: contact,
        ticks_compared: d.frames_compared,
        budget: budget.clone(),
        verdict: verdict.clone(),
        diff: d.clone(),
        live_frames: live,
        sim_frames: sim,
    };
    let path = out_dir.join(format!("report-{name}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&rep)?)?;
    std::fs::write(
        out_dir.join("parity-budget.json"),
        serde_json::to_string_pretty(&budget)?,
    )?;
    println!("{}", d.summary());
    println!(
        "budget ({}): {} — worst position {} / hits {} / death-tick {} / presence {}",
        budget.mode,
        if verdict.within { "WITHIN" } else { "OVER" },
        verdict.worst_position,
        verdict.worst_hits,
        verdict.worst_death_tick,
        verdict.presence_mismatches
    );
    println!("report: {}", path.display());
    if budget.is_gating() && !verdict.within {
        bail!("{name}: parity over the GATING budget");
    }
    Ok(path)
}

/// Every catalog entry's layer-2 report. Fails only under a gating budget (D6).
pub async fn nightly(cfg: &KitConfig, opts: &BedOptions) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut over = Vec::new();
    let budget = load_budget()?;
    for name in catalog_names()? {
        match report(cfg, &name, None, opts).await {
            Ok(p) => out.push(p),
            Err(e) => {
                tracing::error!("{name}: {e:#}");
                over.push(name);
            }
        }
    }
    if budget.is_gating() && !over.is_empty() {
        bail!("nightly: {} scenario(s) over budget / failed: {over:?}", over.len());
    }
    Ok(out)
}

// ───────────────────────────── replay helper for the CLI ─────────────────────────────

/// Docker-free: replay a committed vector (or catalog entry) and print the diff — the operator's
/// one-liner to inspect a divergence the conformance test reported.
pub fn check_file(path: &Path) -> Result<ParityDiff> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let v = GoldenVector::from_json(&raw).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    if v.frames.is_empty() {
        let frames = replay(&v).map_err(|e| anyhow!("{e}"))?;
        println!("{}: no frames (catalog entry) — sim produces {} frames", path.display(), frames.len());
        return Ok(ParityDiff::default());
    }
    screeps_combat_engine::parity::check(&v).map_err(|e| anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every catalog entry parses, replays through the sim without error, and its script names only
    /// creeps/structures it declares — the Docker-free half of "the bed is well-formed".
    #[test]
    fn catalog_entries_replay_in_the_sim() {
        let names = catalog_names().unwrap();
        assert!(names.len() >= 5, "minimal set: {names:?}");
        for name in &names {
            let v = load_catalog(name).unwrap();
            assert!(v.frames.is_empty(), "{name}: catalog entries carry no frames");
            assert_eq!(v.owners.len(), 2, "{name}: two owners");
            assert!(v.creeps.iter().all(|c| c.name.starts_with(&format!("pv-{name}-"))), "{name}: roster prefix");
            let frames = replay(&v).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(frames.len() as u32, v.tick_count() + 1);
            assert!(frames[0].creeps.len() == v.creeps.len());
        }
    }

    /// The budget file is the seeded REPORT-ONLY one (D6 / ADR 0015): promotion to gating is an
    /// operator edit, never a code default.
    #[test]
    fn budget_is_seeded_report_only() {
        let b = load_budget().unwrap();
        assert_eq!(b.v, 1);
        assert_eq!(b.mode, "report-only");
        assert!(!b.is_gating());
    }

    /// PV1 lines → frames: deaths are attributed to the tick BEFORE the absence, and holes fail.
    #[test]
    fn pv1_lines_become_frames_with_deaths_attributed() {
        let mk = |t: u32, n: &str, hits: u32| {
            format!(
                r#"{{"ts_ms":0,"tick":null,"kind":"log","line":"(INFO) screeps_ibex::eval_parity: PV1 {{\"g\":{},\"t\":{t},\"n\":\"{n}\",\"mine\":true,\"x\":24,\"y\":25,\"hits\":{hits},\"hits_max\":200,\"fatigue\":0,\"parts\":[[\"attack\",100,null],[\"move\",100,\"ZO\"]],\"did\":[]}}"}}"#,
                100 + t
            )
        };
        // `absent` names are quoted inside the JSON-encoded `line` string, hence the escaping.
        let roster = |t: u32, absent: &[&str]| {
            let names: Vec<String> = absent.iter().map(|n| format!("\\\"{n}\\\"")).collect();
            format!(
                r#"{{"ts_ms":0,"tick":null,"kind":"log","line":"(INFO) screeps_ibex::eval_parity: PV1 {{\"g\":{},\"t\":{t},\"absent\":[{}]}}"}}"#,
                100 + t,
                names.join(",")
            )
        };
        let console = [
            mk(0, "pv-x-0", 200),
            mk(0, "pv-x-1", 200),
            roster(0, &[]),
            "not a pv line".to_string(),
            mk(1, "pv-x-0", 170),
            mk(1, "pv-x-1", 140),
            roster(1, &[]),
            mk(2, "pv-x-0", 170),
            roster(2, &["pv-x-1"]),
        ]
        .join("\n");
        let lines = parse_pv1(&console);
        assert_eq!(lines.len(), 8);
        let trace = LiveTrace::from_lines(&lines);
        let names = vec!["pv-x-0".to_string(), "pv-x-1".to_string()];
        let frames = trace.frames(&names, &[], 2).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1].creeps[1].hits, 140);
        assert_eq!(frames[1].deaths, vec!["pv-x-1".to_string()], "died during tick 1");
        assert_eq!(frames[2].creeps.len(), 1);
        assert!(frames.iter().all(|f| f.structures.is_empty() && f.towers.is_empty() && f.destroyed.is_empty()));
        assert!(trace.frames(&names, &[], 3).is_err(), "a hole is not evidence");
        // Parts/boosts survive into the layer-2 world.
        let catalog = GoldenVector {
            scenario: "x".into(),
            ..Default::default()
        };
        let built = world_from_live(&trace.ticks[&0], &catalog, "W1N1", "").unwrap();
        assert_eq!(built.world.movement.creeps.len(), 2);
        assert_eq!(built.creep_names[&1], "pv-x-0");
        assert_eq!(built.world.movement.creeps[0].body.parts[1].boost, screeps_combat_engine::BoostTier::T1);
        assert_eq!(first_contact(&trace), None, "no hostile lines → no contact");
    }

    /// PV1 structure (`s`) / tower (`tw`) lines become `frame.structures` / `frame.towers`, and a
    /// label standing at `t-1` but absent at `t` lands in frame `t-1`'s `destroyed` — the same
    /// convention `replay()` emits, so a server capture and the sim are comparable field for field
    /// (the tower-rampart bed's rampart redirect + break is only evidence if this holds).
    #[test]
    fn pv1_structure_and_tower_lines_become_frames_with_destroyed_attributed() {
        let mk = |t: u32, body: &str| {
            format!(
                r#"{{"ts_ms":0,"tick":null,"kind":"log","line":"(INFO) screeps_ibex::eval_parity: PV1 {{\"g\":{},\"t\":{t},{body}}}"}}"#,
                100 + t
            )
        };
        let console = [
            mk(0, r#"\"n\":\"pv-x-0\",\"mine\":true,\"x\":24,\"y\":25,\"hits\":100,\"hits_max\":100,\"fatigue\":0,\"parts\":[[\"attack\",100,null]],\"did\":[]"#),
            mk(0, r#"\"s\":\"r0\",\"hits\":5000"#),
            mk(0, r#"\"tw\":\"t0\",\"hits\":3000,\"energy\":1000"#),
            mk(0, r#"\"absent\":[],\"gone\":[]"#),
            mk(1, r#"\"n\":\"pv-x-0\",\"mine\":true,\"x\":24,\"y\":25,\"hits\":100,\"hits_max\":100,\"fatigue\":0,\"parts\":[[\"attack\",100,null]],\"did\":[]"#),
            mk(1, r#"\"s\":\"r0\",\"hits\":4400"#),
            mk(1, r#"\"tw\":\"t0\",\"hits\":3000,\"energy\":990"#),
            mk(1, r#"\"absent\":[],\"gone\":[]"#),
            mk(2, r#"\"n\":\"pv-x-0\",\"mine\":true,\"x\":24,\"y\":25,\"hits\":100,\"hits_max\":100,\"fatigue\":0,\"parts\":[[\"attack\",100,null]],\"did\":[]"#),
            mk(2, r#"\"tw\":\"t0\",\"hits\":3000,\"energy\":980"#),
            mk(2, r#"\"absent\":[],\"gone\":[\"r0\"]"#),
        ]
        .join("\n");
        let lines = parse_pv1(&console);
        assert_eq!(lines.len(), 11, "every shape parses: {lines:?}");
        assert!(matches!(&lines[1], Pv1Line::Structure(s) if s.s == "r0" && s.hits == 5000));
        assert!(matches!(&lines[2], Pv1Line::Tower(tw) if tw.tw == "t0" && tw.energy == 1000));
        assert!(matches!(&lines[10], Pv1Line::Roster(r) if r.gone == vec!["r0".to_string()]));
        let trace = LiveTrace::from_lines(&lines);
        let roster = vec!["pv-x-0".to_string()];
        let labels = vec!["r0".to_string(), "t0".to_string()];
        let frames = trace.frames(&roster, &labels, 2).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames[0].structures,
            vec![FrameStructure { id: "r0".into(), hits: 5000 }],
            "a PV1 structure line becomes frame.structures"
        );
        assert_eq!(
            frames[1].towers,
            vec![FrameTower { id: "t0".into(), energy: 990, hits: 3000 }],
            "a PV1 tower line becomes frame.towers"
        );
        assert_eq!(frames[1].structures[0].hits, 4400);
        assert_eq!(frames[1].destroyed, vec!["r0".to_string()], "absent at t=2 ⇒ destroyed during t=1");
        assert!(frames[0].destroyed.is_empty() && frames[2].destroyed.is_empty());
        assert!(frames[2].structures.is_empty(), "a destroyed label is not listed as standing");
        assert_eq!(frames[2].towers.len(), 1);
        // The layer-2 world seeds the standing structures/towers from the catalog kinds + live hits,
        // so its frames carry the same fields.
        let catalog = load_catalog("tower-rampart").unwrap();
        let mut seed = trace.ticks[&1].clone();
        seed.creeps.clear();
        for c in &catalog.creeps {
            seed.creeps.insert(
                c.name.clone(),
                Pv1Creep {
                    g: 101,
                    t: 1,
                    n: c.name.clone(),
                    mine: c.owner == 0,
                    x: c.x,
                    y: c.y,
                    hits: c.parts.iter().map(|p| p.hits).sum(),
                    hits_max: 0,
                    fatigue: 0,
                    parts: c.parts.iter().map(|p| (p.part.clone(), p.hits, p.boost.clone())).collect(),
                    did: vec![],
                },
            );
        }
        let built = world_from_live(&seed, &catalog, &catalog.room, "").unwrap();
        let f = frame_of(&built, 1);
        assert_eq!(f.structures, vec![FrameStructure { id: "r0".into(), hits: 4400 }]);
        assert_eq!(f.towers, vec![FrameTower { id: "t0".into(), energy: 990, hits: 3000 }]);
        assert_eq!(built.world.structures[0].kind, screeps_combat_engine::StructureKind::Rampart);
        // A label the catalog does not declare is an error, not a silent drop.
        seed.structures.insert("ghost".into(), Pv1Structure { g: 101, t: 1, s: "ghost".into(), hits: 1 });
        assert!(world_from_live(&seed, &catalog, &catalog.room, "").is_err());
    }

    /// The injection carries the flag, the absolute start, the roster and the script verbatim.
    #[test]
    fn injection_arms_flag_and_script_for_one_user() {
        let v = load_catalog("melee-1v1").unwrap();
        let parts = inject_expressions(&v, "W8N7", Some(1234), false);
        assert!(parts.len() >= 3, "prelude + at least one chunk + the arm: {parts:?}");
        assert!(parts.iter().all(|e| e.len() < 1000), "every arming expression stays under the console limit: {parts:?}");
        assert_eq!(parts[0], "Memory._pv=\"\";");
        assert!(parts.last().unwrap().starts_with("Memory.parity_script=JSON.parse(Memory._pv);delete Memory._pv;"), "{parts:?}");
        let expr: String = parts[1..parts.len() - 1]
            .iter()
            .map(|e| serde_json::from_str::<String>(&e["Memory._pv+=".len()..e.len() - 1]).unwrap())
            .collect();
        assert!(expr.starts_with("{"), "{expr}");
        assert!(expr.contains(r#""scenario":"melee-1v1""#), "{expr}");
        assert!(expr.contains(r#""room":"W8N7""#), "{expr}");
        assert!(expr.contains(r#""start":1234"#), "{expr}");
        assert!(expr.contains(r#""trace":false"#), "{expr}");
        assert!(expr.contains(r#""roster":["pv-melee-1v1-0","pv-melee-1v1-1"]"#), "{expr}");
        assert!(expr.contains(r#""ticks":[{"t":0,"intents":[{"creep":"pv-melee-1v1-0","actions":[{"kind":"attack","target":"pv-melee-1v1-1"}]}]}"#), "{expr}");
        assert!(parts.last().unwrap().ends_with(r#"Memory._features.eval.parity_script="melee-1v1";"#), "{parts:?}");
        let off = clear_expression();
        assert!(off.starts_with("delete Memory.parity_script;"));
        assert!(off.ends_with(r#"parity_script="";"#));
        // Seeding splits by owner and keeps boosts.
        let a = seed_creeps_for(&v, 0);
        let b = seed_creeps_for(&v, 1);
        assert_eq!((a.len(), b.len()), (1, 1));
        assert_eq!(a[0].body[0].part, "tough");
    }

    /// A creep line as the driver prints it, from either owner's console (`mine` = printed by the
    /// owning bot, which is the only one that fills `did`).
    fn creep_line(t: u32, n: &str, mine: bool, x: u8, hits: u32, fatigue: u32, did: &[&str]) -> Pv1Line {
        Pv1Line::Creep(Pv1Creep {
            g: 100 + t,
            t,
            n: n.into(),
            mine,
            x,
            y: 25,
            hits,
            hits_max: 200,
            fatigue,
            parts: vec![("attack".into(), 100, None), ("move".into(), 100, None)],
            did: did.iter().map(|d| d.to_string()).collect(),
        })
    }

    fn tower_line(t: u32, label: &str, energy: u32, did: &[&str]) -> Pv1Line {
        Pv1Line::Tower(Pv1Tower {
            g: 100 + t,
            t,
            tw: label.into(),
            hits: 3000,
            energy,
            did: did.iter().map(|d| d.to_string()).collect(),
        })
    }

    /// THE root-cause pin for the first captures (kite-r3 / heal-race / tower-rampart): the acting
    /// identity prints the other owner's actors with an empty `did` whether or not that bot acted,
    /// so a trace built from ONE console cannot tell "the other bot moved" from "the other bot has
    /// no code". The merge takes `did` from the owning bot's console (state fields must agree),
    /// and the execution gate refuses a trace whose scripted actor stayed silent or whose intent
    /// the API rejected. RED before the fix: the kite-r3 capture (pv-kite-r3-1 `did:[]`, x=25,
    /// fatigue 0 on every tick) was written as server evidence.
    #[test]
    fn merge_takes_did_from_the_owning_bot_and_the_gate_rejects_a_silent_owner() {
        let v = load_catalog("kite-r3").unwrap();
        // The acting identity's console: its own kiter acts, the other owner's creep shows did:[].
        let primary: Vec<Pv1Line> = (0..3)
            .flat_map(|t| {
                vec![
                    creep_line(t, "pv-kite-r3-0", true, 22 - t as u8, 400, 0, &["ranged_attack:pv-kite-r3-1", "move:left"]),
                    creep_line(t, "pv-kite-r3-1", false, 25, 180, 0, &[]),
                ]
            })
            .collect();
        let mut trace = LiveTrace::from_lines(&primary);
        // Exactly what the first capture recorded: the other owner silent → the gate refuses.
        let err = verify_script_executed(&v, &trace, 3).unwrap_err().to_string();
        assert!(err.contains("owner \"ibex-2\" never executed its script"), "{err}");
        assert!(err.contains("pv-kite-r3-1 (t=0,1,2)"), "{err}");
        assert!(err.contains("deploy --user ibex-2"), "{err}");

        // The other owner's console: the same state, its own creep with the issued move.
        let theirs: Vec<Pv1Line> = (0..3)
            .flat_map(|t| {
                vec![
                    creep_line(t, "pv-kite-r3-0", false, 22 - t as u8, 400, 0, &[]),
                    creep_line(t, "pv-kite-r3-1", true, 25, 180, 0, &["move:left"]),
                ]
            })
            .collect();
        trace.merge_owner("ibex-2", &theirs).unwrap();
        let c = &trace.ticks[&1].creeps["pv-kite-r3-1"];
        assert!(!c.mine, "`mine` stays relative to the acting identity");
        assert_eq!(c.did, vec!["move:left".to_string()]);
        assert_eq!(trace.ticks[&1].creeps["pv-kite-r3-0"].did.len(), 2, "the acting identity's did is kept");
        assert!(verify_script_executed(&v, &trace, 3).unwrap().is_empty());

        // A creep intent the API refused (the real kite-r3 recapture: `move:left!Tired` while the
        // kiter's target rests off the swamp step) is script semantics both engines model — kept
        // as evidence, surfaced as a warning, never a failure.
        let mut tired = trace.clone();
        tired.ticks.get_mut(&2).unwrap().creeps.get_mut("pv-kite-r3-1").unwrap().did = vec!["move:left!Tired".into()];
        let warnings = verify_script_executed(&v, &tired, 3).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("pv-kite-r3-1 t=2 move:left!Tired") && warnings[0].contains("recorded as evidence"), "{warnings:?}");

        // A dead actor is exempt (no line at that tick), and ticks past the capture are ignored.
        let mut dead = trace.clone();
        dead.ticks.get_mut(&2).unwrap().creeps.remove("pv-kite-r3-1");
        verify_script_executed(&v, &dead, 3).unwrap();
        verify_script_executed(&v, &LiveTrace::from_lines(&primary[..2]), 1).unwrap_err();

        // The two consoles must agree on the state they both print.
        let disagree = vec![creep_line(1, "pv-kite-r3-1", true, 24, 180, 8, &["move:left"])];
        let err = trace.merge_owner("ibex-2", &disagree).unwrap_err().to_string();
        assert!(err.contains("not describing the same tick"), "{err}");
    }

    /// The tower half of the same pin: the tower-rampart capture showed t0 at 1000 energy on every
    /// frame with no trace of the rejected `attack` — the driver's tower line carried no `did`.
    /// Now a tower whose owner does not hold the room prints `tower:attack:<target>!` and the
    /// gate names the controller rule; a silent tower (no did at all) is refused too.
    #[test]
    fn tower_execution_gate_sees_rejected_and_silent_towers() {
        let v = load_catalog("tower-rampart").unwrap();
        let mut trace = LiveTrace::from_lines(&[
            creep_line(0, "pv-tower-rampart-0", false, 25, 600, 0, &[]),
            tower_line(0, "t0", 1000, &["tower:attack:pv-tower-rampart-0!RclNotEnough"]),
            creep_line(1, "pv-tower-rampart-0", false, 25, 600, 0, &[]),
            tower_line(1, "t0", 1000, &["tower:attack:pv-tower-rampart-0!RclNotEnough"]),
        ]);
        let err = verify_script_executed(&v, &trace, 2).unwrap_err().to_string();
        assert!(err.contains("owner \"private-server\": the game API REJECTED"), "{err}");
        assert!(err.contains("t0 t=0 tower:attack:pv-tower-rampart-0!RclNotEnough"), "{err}");
        assert!(err.contains("room controller"), "{err}");

        // A pre-error-code driver's bare `!` is refused the same way.
        fn set(trace: &mut LiveTrace, did: &[&str]) {
            for t in 0..2 {
                trace.ticks.get_mut(&t).unwrap().towers.get_mut("t0").unwrap().did = did.iter().map(|d| d.to_string()).collect();
            }
        }
        set(&mut trace, &["tower:attack:pv-tower-rampart-0!"]);
        assert!(verify_script_executed(&v, &trace, 2).unwrap_err().to_string().contains("REJECTED"));

        set(&mut trace, &[]);
        let err = verify_script_executed(&v, &trace, 2).unwrap_err().to_string();
        assert!(err.contains("never executed its script") && err.contains("t0 (t=0,1)"), "{err}");

        // Out of energy is the one refusal the sim models (`can_fire`): evidence, not a defect.
        set(&mut trace, &["tower:attack:pv-tower-rampart-0!NotEnoughEnergy"]);
        assert!(verify_script_executed(&v, &trace, 2).unwrap().is_empty());

        set(&mut trace, &["tower:attack:pv-tower-rampart-0"]);
        assert!(verify_script_executed(&v, &trace, 2).unwrap().is_empty());

        // `!Missing` while the target still stands is a defect; once the target is dead (no line
        // for it that tick — the real recapture killed the creep at t=9 and the script keeps
        // naming it) the driver makes no call and neither does the sim: evidence.
        set(&mut trace, &["tower:attack:pv-tower-rampart-0!Missing"]);
        assert!(verify_script_executed(&v, &trace, 2).unwrap_err().to_string().contains("!Missing"));
        trace.ticks.get_mut(&1).unwrap().creeps.remove("pv-tower-rampart-0");
        let err = verify_script_executed(&v, &trace, 2).unwrap_err().to_string();
        assert!(err.contains("t0 t=0") && !err.contains("t0 t=1"), "only the standing-target tick is refused: {err}");
        trace.ticks.get_mut(&0).unwrap().creeps.remove("pv-tower-rampart-0");
        assert!(verify_script_executed(&v, &trace, 2).unwrap().is_empty());
        // The other owner's console carries the tower line without did; the merge keeps ours.
        trace.merge_owner("ibex-2", &[tower_line(0, "t0", 1000, &[])]).unwrap();
        assert_eq!(trace.ticks[&0].towers["t0"].did.len(), 1);
        // The parser accepts a tower line without `did` (older driver) and with it.
        let lines = parse_pv1(concat!(
            r#"{"ts_ms":0,"tick":null,"kind":"log","line":"(INFO) screeps_ibex::eval_parity: PV1 {\"g\":1,\"t\":0,\"tw\":\"t0\",\"hits\":3000,\"energy\":1000}"}"#,
            "\n",
            r#"{"ts_ms":0,"tick":null,"kind":"log","line":"(INFO) screeps_ibex::eval_parity: PV1 {\"g\":1,\"t\":1,\"tw\":\"t0\",\"hits\":3000,\"energy\":990,\"did\":[\"tower:attack:pv-x-0\"]}"}"#,
        ));
        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[0], Pv1Line::Tower(t) if t.did.is_empty()));
        assert!(matches!(&lines[1], Pv1Line::Tower(t) if t.did == vec!["tower:attack:pv-x-0".to_string()]));
    }

    /// Tower beds hand the bed room's controller to the tower owner at the engine's
    /// `CONTROLLER_STRUCTURES.tower` level for that many towers; two tower owners cannot share one
    /// controller.
    #[test]
    fn tower_beds_need_the_controller_at_the_tower_ladder_level() {
        assert_eq!(rcl_for_towers(0).unwrap(), None);
        assert_eq!(rcl_for_towers(1).unwrap(), Some(3));
        assert_eq!(rcl_for_towers(2).unwrap(), Some(5));
        assert_eq!(rcl_for_towers(3).unwrap(), Some(7));
        assert_eq!(rcl_for_towers(6).unwrap(), Some(8));
        assert!(rcl_for_towers(7).is_err());
        let v = load_catalog("tower-rampart").unwrap();
        assert_eq!(tower_owner(&v).unwrap().unwrap().user, "private-server");
        assert_eq!(tower_owner(&load_catalog("melee-1v1").unwrap()).unwrap(), None);
        let mut two = v.clone();
        two.towers.push(VecTower { id: "t1".into(), owner: 1, x: 30, y: 25, energy: 1000, hits: 3000, hits_max: 3000 });
        assert!(tower_owner(&two).unwrap_err().to_string().contains("different owners"));
    }

    /// The "remove everything new" set: what the room holds at bed end minus the pre-bed snapshot,
    /// by DB id — the seeded pv-* objects AND whatever the owning bot did with its ~60-tick
    /// "colony" during a tower bed's controller claim. RED before: the bed's cleanup removed
    /// only pv-* creeps and the seeded rampart/wall/tower tiles, so the first tower-rampart
    /// capture left a spawn construction site, six extension sites + a completed extension and
    /// two container sites standing in W9N7.
    #[test]
    fn bed_end_removes_every_object_not_in_the_pre_bed_snapshot() {
        let obj = |id: &str, kind: &str| RoomObjectRef { id: id.into(), kind: kind.into() };
        // Before the claim: the room's intrinsics + the primary bot's remote-mining container/creep.
        let snapshot = vec![obj("c0", "controller"), obj("s0", "source"), obj("s1", "source"), obj("k0", "container"), obj("h0", "creep")];
        // At bed end: the seeded pv-* creeps/tower, the bot's construction sites, a completed
        // extension, its builder — and the remote hauler `h0` has left the room (nothing to do).
        let current = vec![
            obj("c0", "controller"),
            obj("s0", "source"),
            obj("s1", "source"),
            obj("k0", "container"),
            obj("pv0", "creep"),
            obj("t0", "tower"),
            obj("cs0", "constructionSite"),
            obj("cs1", "constructionSite"),
            obj("e0", "extension"),
            obj("b0", "creep"),
        ];
        let doomed = objects_not_in(&snapshot, &current);
        let ids: Vec<&str> = doomed.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["pv0", "t0", "cs0", "cs1", "e0", "b0"], "everything new, in room order; nothing from the snapshot");
        assert_eq!(summarize_kinds(&doomed), "constructionSitex2, creepx2, extensionx1, towerx1");
        assert!(objects_not_in(&snapshot, &snapshot).is_empty(), "an untouched room removes nothing");
        assert!(objects_not_in(&snapshot, &[]).is_empty(), "objects that left the room are not listed");
        assert_eq!(objects_not_in(&[], &current).len(), current.len(), "the helper is a plain set difference — the EMPTY-snapshot refusal lives in run_bed and the builder");
        // The removal builder gets exactly the snapshot ids as its keep set.
        let keep: Vec<String> = snapshot.iter().map(|o| o.id.clone()).collect();
        let cmd = server::cmd_remove_objects_not_in("W9N7", &keep);
        assert!(cmd.contains(r#"new Set(["c0","s0","s1","k0","h0"])"#), "{cmd}");
    }

    /// The stale-claim rule (RED before: ANY controller held by a bed owner id was forced to
    /// neutral — the primary bot's own colony rooms included, since `private-server` is a bed
    /// owner): only a row carrying this harness's `parityClaim` marker with the claim's shape is
    /// released, and to the row the marker saved (a reservation comes back, not a bare neutral);
    /// an owned room without the marker is refused with the text that says so.
    #[test]
    fn only_a_marked_parity_claim_is_released_and_owned_rooms_are_refused() {
        let saved = ControllerOwnership {
            level: 0,
            reservation: Some(serde_json::json!({"user": "u1", "endTime": 19131303})),
            ..Default::default()
        };
        let live = ControllerOwnership {
            user: Some("u1".into()),
            level: 3,
            progress: 0,
            downgrade_time: Some(1_019_131_400),
            reservation: None,
            safe_mode: None,
            safe_mode_cooldown: None,
            safe_mode_available: Some(0),
            parity_claim: Some(Box::new(saved.clone())),
        };
        assert_eq!(stale_claim(&live), StaleClaim::Release { holder: "u1", before: &saved });
        // The release restores what was saved — the reservation included — not a bare neutral.
        let StaleClaim::Release { holder, before } = stale_claim(&live) else { unreachable!() };
        let restore = server::cmd_restore_controller("W9N7", holder, before);
        assert!(restore.contains(r#""reservation":{"endTime":19131303,"user":"u1"}"#), "{restore}");
        assert!(restore.contains(r#""$unset":{"downgradeTime":true,"parityClaim":true,"safeMode":true,"safeModeAvailable":true,"safeModeCooldown":true,"user":true}"#), "{restore}");

        // A bed owner's REAL colony (no marker, a real downgrade timeline): refused, never released.
        let colony = ControllerOwnership {
            user: Some("u1".into()),
            level: 6,
            progress: 40_000,
            downgrade_time: Some(19_200_000),
            ..Default::default()
        };
        assert_eq!(stale_claim(&colony), StaleClaim::Owned { holder: "u1" });
        let text = owned_room_refusal("W9N8", "u1");
        assert!(text.contains("owned by user u1 and carries no parity claim marker"), "{text}");
        assert!(text.contains("never touches a room a user holds"), "{text}");
        assert!(text.contains("--room"), "{text}");
        // Neutral (reserved or not): nothing to release.
        assert_eq!(stale_claim(&saved), StaleClaim::Neutral);
        assert_eq!(stale_claim(&ControllerOwnership::default()), StaleClaim::Neutral);
        // A marker on a row that no longer has the claim's shape is not trusted either.
        let mut odd = live.clone();
        odd.downgrade_time = Some(19_200_000);
        assert_eq!(stale_claim(&odd), StaleClaim::Owned { holder: "u1" });
    }

    /// The deployed-code gate: the second owner with only the kit's bootstrap `main` (what the
    /// warm world had for ibex-2 when the first captures ran), a different build, or an inactive
    /// runtime is refused with the deploy command; identical active builds pass.
    #[test]
    fn owner_code_gate_requires_the_same_active_build() {
        let bot = |user: &str, fp: Option<&str>, modules: &[&str], active: u64| CodeFingerprint {
            user: user.into(),
            branch: Some("default".into()),
            modules: modules.iter().map(|m| m.to_string()).collect(),
            fingerprint: fp.map(str::to_string),
            active,
        };
        let owners = vec![("private-server".to_string(), "u1".to_string()), ("ibex-2".to_string(), "u2".to_string())];
        let wasm = ["main", "screeps_ibex", "screeps_ibex_bg"];
        let ok = [bot("u1", Some("abc"), &wasm, 10000), bot("u2", Some("abc"), &wasm, 10000)];
        assert!(verify_owner_code(&ok, &owners).unwrap().is_empty());
        // Same modules, different bytes (a tree edited between the two deploys): a warning, not a refusal.
        let differ = [bot("u1", Some("abc"), &wasm, 10000), bot("u2", Some("def"), &wasm, 10000)];
        let warnings = verify_owner_code(&differ, &owners).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("different BUILD") && warnings[0].contains("def vs abc"), "{warnings:?}");

        let empty = [bot("u1", Some("abc"), &wasm, 10000), bot("u2", Some("000"), &["main"], 0)];
        let err = verify_owner_code(&empty, &owners).unwrap_err().to_string();
        assert!(err.contains("\"ibex-2\" runs DIFFERENT code") && err.contains("deploy --user ibex-2"), "{err}");

        let none = [bot("u1", Some("abc"), &wasm, 10000), bot("u2", None, &[], 0)];
        let err = verify_owner_code(&none, &owners).unwrap_err().to_string();
        assert!(err.contains("no active-world code") && err.contains("deploy --user ibex-2"), "{err}");

        // An inactive runtime (the driver's no-objects rule) is not a code problem — the bed
        // re-activates every owner after seeding — so it passes here.
        let inactive = [bot("u1", Some("abc"), &wasm, 10000), bot("u2", Some("abc"), &wasm, 0)];
        assert!(verify_owner_code(&inactive, &owners).unwrap().is_empty());

        let bare = [bot("u1", Some("abc"), &["main"], 10000), bot("u2", Some("abc"), &["main"], 10000)];
        assert!(verify_owner_code(&bare, &owners).unwrap_err().to_string().contains("not a bot build"));
    }

    /// The nightly lane (D6): `cargo test -p screeps-ibex-eval -- --ignored parity_nightly`. Needs the
    /// Docker stack + `.screeps.yaml`; report-only until the budget file says `gating`.
    #[test]
    #[ignore = "Docker lane: runs every catalog bed on the private server (parity nightly)"]
    fn parity_nightly_within_budget() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = KitConfig::load(None, None).expect("kit config (.screeps.yaml + config/local.yml)");
        let reports = rt.block_on(nightly(&cfg, &BedOptions::default())).expect("nightly ran");
        assert!(!reports.is_empty(), "no catalog entries reported");
    }

    /// Layer-2 grading on a synthetic trace: the sim from first contact vs a live trace that IS the
    /// sim's own output diffs to zero (self-consistency) — pins the frame numbering from contact.
    #[test]
    fn grade_is_self_consistent_on_a_sim_trace() {
        let v = load_catalog("melee-1v1").unwrap();
        // Build the live trace from the sim itself: run IbexAgent both sides from the catalog world.
        let frames = sim_frames(build_world(&v).unwrap(), 0, 8);
        let mut trace = LiveTrace::default();
        let owner_of: BTreeMap<&str, u8> = v.creeps.iter().map(|c| (c.name.as_str(), c.owner)).collect();
        let parts_of: BTreeMap<&str, Vec<Pv1Part>> = v
            .creeps
            .iter()
            .map(|c| (c.name.as_str(), c.parts.iter().map(|p| (p.part.clone(), p.hits, p.boost.clone())).collect()))
            .collect();
        for f in &frames {
            let mut m = TickLines::default();
            for c in &f.creeps {
                m.creeps.insert(
                    c.name.clone(),
                    Pv1Creep {
                        g: 1000 + f.t,
                        t: f.t,
                        n: c.name.clone(),
                        mine: owner_of[c.name.as_str()] == 0,
                        x: c.x,
                        y: c.y,
                        hits: c.hits,
                        hits_max: 0,
                        fatigue: c.fatigue,
                        parts: parts_of[c.name.as_str()].clone(),
                        did: vec![],
                    },
                );
            }
            trace.ticks.insert(f.t, m);
        }
        let last = frames.last().unwrap().t;
        let budget = load_budget().unwrap();
        let g = grade(&trace, &v, last, &v.room, "", &budget).unwrap();
        assert_eq!(g.contact_tick, 0, "adjacent at t0");
        assert_eq!(g.live_frames.len(), g.sim_frames.len());
        assert!(g.diff.is_zero(), "{}", g.diff.summary());
        assert!(g.verdict.within);
    }
}
