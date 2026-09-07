//! The H5 sim-vs-server parity oracle — the **Docker-facing** half (ADR 0006 §B.4 layers 1 + 2;
//! WS-CLOSE lane (b3), decisions D5/D6/D8). The pure schema / replay / diff live in
//! `screeps_combat_engine::parity`; this module owns policy + orchestration:
//!
//! - the **scenario catalog** `parity/<name>.json` — golden vectors WITHOUT frames (initial world +
//!   script); `catalog_names` / `load_catalog`;
//! - `synth` — replay a catalog entry through the sim and write it as a **placeholder** vector into
//!   `screeps-combat-engine/tests/conformance/` (provenance says so; replaced by the first capture);
//! - `capture` (layer 1) — seed the catalog entry's creeps/structures into the warm private world
//!   through the kit's `cmd_insert_*` builders, inject the script + the `eval.parity_script` flag
//!   into BOTH owners' Memory with one absolute start tick, run the kit capture, parse the bot's
//!   per-tick `PV1 ` console lines into frames, cross-check the last frame against the DB, and
//!   write the vector with server provenance;
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
use screeps_combat_engine::parity::{
    assess, build_world, diff, frame_of, replay, synthesize, BudgetVerdict, BuiltWorld, FrameCreep,
    FrameStructure, FrameTower, GoldenVector, Owner, ParityBudget, ParityDiff, Provenance,
    VecCreep, VecFrame, VecPart, VecStructure, VecTower, SERVER_CAPTURE_PROVENANCE,
};
use screeps_combat_engine::CombatWorld;
use screeps_server_kit::capture::{self, ConsoleInjection};
use screeps_server_kit::config::KitConfig;
use screeps_server_kit::server::{
    self, CliClient, LiveCreep, SeedCreep, SeedPart, SeedStructure,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

/// A labelled tower still standing this tick (`tw` = the script label).
#[derive(Debug, Clone, Deserialize)]
pub struct Pv1Tower {
    pub g: u32,
    pub t: u32,
    pub tw: String,
    pub hits: u32,
    pub energy: u32,
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

/// The JS that installs the script + arms the flag for one user. `start` is the absolute tick of
/// scenario tick 0 (both users get the same one); `trace` = frames only.
pub fn inject_expression(v: &GoldenVector, room: &str, start: Option<u32>, trace: bool) -> String {
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
    format!(
        "Memory.parity_script={json};{}",
        crate::scenario::feature_set("eval", &format!("parity_script={}", js_str(&v.scenario)))
    )
}

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
struct OwnerClient {
    owner: Owner,
    api: screeps_rest_api::Client,
    user_id: String,
}

async fn owner_clients(cfg: &KitConfig, v: &GoldenVector) -> Result<Vec<OwnerClient>> {
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
                     the parity beds need every owner registered as a bot (README: bots: [ibex, ibex-2])",
                    owner.user,
                    cfg.bots.iter().map(|b| b.name.as_str()).collect::<Vec<_>>()
                )
            })?;
        let api = screeps_server_kit::api::connect(&bot.endpoint).await?;
        let user_id = api.me().await?.id;
        out.push(OwnerClient {
            owner: owner.clone(),
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

    tracing::info!("parity bed 3/5: seed {room}");
    server::pause(&cli).await?;
    let prefix = format!("pv-{}-", v.scenario);
    let removed = cli.send(&server::cmd_remove_seeded(&room, &prefix, &seeded_tiles(v))).await?;
    tracing::info!("cleanup: {}", removed.trim());
    for oc in &owners {
        let creeps = seed_creeps_for(v, oc.owner.player);
        if !creeps.is_empty() {
            let r = cli.send(&server::cmd_insert_creeps(&oc.user_id, &room, &creeps)).await?;
            tracing::info!("{}: creeps {}", oc.owner.user, r.trim());
        }
        let structures = seed_structures_for(v, oc.owner.player);
        if !structures.is_empty() {
            let r = cli.send(&server::cmd_insert_structures(&oc.user_id, &room, &structures)).await?;
            tracing::info!("{}: structures {}", oc.owner.user, r.trim());
        }
    }
    if opts.activate_rooms {
        let r = cli.send(&server::cmd_activate_rooms()).await?;
        tracing::info!("activate rooms: {} — restarting the stack (flag is read at boot)", r.trim());
        screeps_server_kit::docker::down().await?;
        screeps_server_kit::docker::up(&cfg.stack).await?;
    }
    server::resume(&cli).await?;

    tracing::info!("parity bed 4/5: arm both owners (lead {} ticks)", opts.lead_ticks);
    let now = owners[0].api.game_time().await?.time as u32;
    let start = now + opts.lead_ticks;
    let expr = inject_expression(v, &room, Some(start), trace_only);
    for oc in &owners {
        oc.api
            .console(&expr)
            .await
            .with_context(|| format!("arming {}", oc.owner.user))?;
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
    let artifacts = capture::run(
        cfg,
        (opts.lead_ticks + ticks + TAIL_TICKS + 2) as u64,
        &format!("{label}-{}", v.scenario),
        &spec,
    )
    .await?;
    for oc in &owners {
        let _ = oc.api.console(&clear_expression()).await;
    }

    let console = std::fs::read_to_string(artifacts.dir.join("console.jsonl"))
        .with_context(|| format!("reading {}", artifacts.dir.join("console.jsonl").display()))?;
    let trace = LiveTrace::from_lines(&parse_pv1(&console));
    let db_creeps = server::parse_room_creeps(&cli.send(&server::cmd_room_creeps(&room)).await?)
        .unwrap_or_default();
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
        let expr = inject_expression(&v, "W8N7", Some(1234), false);
        assert!(expr.starts_with("Memory.parity_script={"), "{expr}");
        assert!(expr.contains(r#""scenario":"melee-1v1""#), "{expr}");
        assert!(expr.contains(r#""room":"W8N7""#), "{expr}");
        assert!(expr.contains(r#""start":1234"#), "{expr}");
        assert!(expr.contains(r#""trace":false"#), "{expr}");
        assert!(expr.contains(r#""roster":["pv-melee-1v1-0","pv-melee-1v1-1"]"#), "{expr}");
        assert!(expr.contains(r#""ticks":[{"t":0,"intents":[{"creep":"pv-melee-1v1-0","actions":[{"kind":"attack","target":"pv-melee-1v1-1"}]}]}"#), "{expr}");
        assert!(expr.ends_with(r#"Memory._features.eval.parity_script="melee-1v1";"#), "{expr}");
        let off = clear_expression();
        assert!(off.starts_with("delete Memory.parity_script;"));
        assert!(off.ends_with(r#"parity_script="";"#));
        // Seeding splits by owner and keeps boosts.
        let a = seed_creeps_for(&v, 0);
        let b = seed_creeps_for(&v, 1);
        assert_eq!((a.len(), b.len()), (1, 1));
        assert_eq!(a[0].body[0].part, "tough");
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
