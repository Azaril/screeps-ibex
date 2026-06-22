use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

// ─── Reset flags (separate from feature flags) ────────────────────────────────
//
// Reset flags are one-shot triggers read *before* the feature cache is
// populated, so they have their own lightweight path that reads directly from
// `Memory._features.reset` without caching.

/// Read the one-shot reset flags directly from `Memory._features.reset`.
///
/// This is intentionally cheap (two JS reads) and independent of the feature
/// flag cache, because resets may destroy/recreate the environment before the
/// main features are loaded.
pub fn load_reset() -> ResetFlags {
    let root = crate::memory_helper::root();
    let features = js_get(&root, "_features");
    let reset = js_get(&features, "reset");
    ResetFlags {
        environment: js_bool(&reset, "environment"),
        memory: js_bool(&reset, "memory"),
        room_plans: js_bool(&reset, "room_plans"),
    }
}

/// Clear reset flags in Memory so they don't fire again.
pub fn clear_reset() {
    crate::memory_helper::path_set("_features.reset.environment", false);
    crate::memory_helper::path_set("_features.reset.memory", false);
    crate::memory_helper::path_set("_features.reset.room_plans", false);
}

#[derive(Debug, Clone, Copy)]
pub struct ResetFlags {
    pub environment: bool,
    pub memory: bool,
    pub room_plans: bool,
}

// ─── Feature flag structs ──────────────────────────────────────────────────────
//
// Every struct derives `Serialize + Deserialize` with `#[serde(default)]` so
// that `serde_wasm_bindgen` can round-trip them to/from the JS Memory tree.
// Missing keys in Memory automatically fall back to the struct's `Default` impl
// — this is the primary mechanism for providing safe defaults when the Memory
// schema changes across ticks.

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizeFeatures {
    pub on: bool,
}

impl Default for VisualizeFeatures {
    fn default() -> Self {
        Self { on: true }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ConstructionVisualizeFeatures {
    pub on: bool,
    pub planner: bool,
    pub planner_best: bool,
    pub plan: bool,
}

impl ConstructionVisualizeFeatures {
    /// Returns `planner && on`.
    pub fn planner(&self) -> bool {
        self.planner && self.on
    }

    /// Returns `planner_best && on`.
    pub fn planner_best(&self) -> bool {
        self.planner_best && self.on
    }

    /// Returns `plan && on`.
    pub fn plan(&self) -> bool {
        self.plan && self.on
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ConstructionFeatures {
    pub plan: bool,
    pub force_plan: bool,
    /// Reserved kill-switch for discretionary re-planning of rooms that already
    /// have a valid plan. Recovery of a plan-less room is NOT gated by this (S3):
    /// a room with no usable plan always re-plans (subject to backoff) so it
    /// regains construction and authoritative spawn approaches. Today only
    /// `force_plan` triggers re-planning of an already-valid room.
    pub allow_replan: bool,
    pub execute: bool,
    pub cleanup: bool,
    pub max_construction_sites: i32,
    pub visualize: ConstructionVisualizeFeatures,
    /// Per-tick CPU budget for room planning (screeps-foreman pipeline). Uses remaining CPU
    /// and never exceeds tick limit. Scales with GCL. Default: 20.0.
    #[serde(default = "default_room_plan_cpu_budget")]
    pub room_plan_cpu_budget: f64,
    /// Minimum bucket level for room planning to run. Planning is a burst activity; set to 0 to
    /// always allow. Kept low (1000) so a plan-less room can always recover a plan and resume
    /// construction — a high gate strands rooms with no plan (e.g. after a `reset.room_plans`)
    /// whenever the bucket can't climb back to the gate. Default: 1000.
    #[serde(default = "default_bucket_threshold")]
    pub bucket_threshold: i32,
}

fn default_room_plan_cpu_budget() -> f64 {
    20.0
}

fn default_bucket_threshold() -> i32 {
    1000
}

impl Default for ConstructionFeatures {
    fn default() -> Self {
        Self {
            plan: true,
            force_plan: false,
            allow_replan: false,
            execute: true,
            cleanup: true,
            max_construction_sites: 10,
            visualize: ConstructionVisualizeFeatures::default(),
            room_plan_cpu_budget: 20.0,
            bucket_threshold: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketFeatures {
    pub buy: bool,
    pub sell: bool,
    pub credit_reserve: f64,
    pub buy_energy: bool,
    pub buy_minerals: bool,
}

impl Default for MarketFeatures {
    fn default() -> Self {
        Self {
            buy: false,
            sell: false,
            credit_reserve: 10_000_000.0,
            buy_energy: false,
            buy_minerals: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct TransferVisualizeFeatures {
    pub on: bool,
    pub haul: bool,
    pub demand: bool,
    pub orders: bool,
}

impl TransferVisualizeFeatures {
    pub fn haul(&self) -> bool {
        self.haul && self.on
    }

    pub fn demand(&self) -> bool {
        self.demand && self.on
    }

    pub fn orders(&self) -> bool {
        self.orders && self.on
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct TransferFeatures {
    pub visualize: TransferVisualizeFeatures,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteMineFeatures {
    pub harvest: bool,
    pub reserve: bool,
}

impl Default for RemoteMineFeatures {
    fn default() -> Self {
        Self {
            harvest: true,
            reserve: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct PathingVisualizeFeatures {
    pub on: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct PathingFeatures {
    pub visualize: PathingVisualizeFeatures,
    pub custom: bool,
    pub reuse_path_length: u32,
    pub max_shove_depth: u32,
    /// Maximum tile distance (Chebyshev) from the creep for proximity-limited
    /// friendly creep avoidance (tier 1 stuck escalation). Only friendly
    /// creeps within this many tiles of the pathing creep are avoided; creeps
    /// further away are ignored since they will likely have moved by the time
    /// the pathing creep arrives. Works both across rooms and within the same
    /// room. Set to 0 to disable proximity limiting (all creeps avoided, old
    /// behaviour).
    pub friendly_creep_distance: u32,
    /// Maximum fraction of the tick CPU limit that the movement system may
    /// spend on pathfinding for stuck creeps. The actual budget is
    /// `min(tick_limit * pct, remaining_cpu)`. Creeps with no path at all
    /// still pathfind unconditionally regardless of budget.
    pub movement_cpu_budget_pct: f64,
    /// CPU budget for non-stuck path expiry repathing per tick. Paths older
    /// than `reuse_path_length` are eligible for re-evaluation but only if
    /// this budget has not been exhausted. Set to 0 to disable expiry
    /// repathing entirely.
    pub repath_cpu_budget: f64,
    /// Per-tick pathfinding ops budget in CPU (1 op ≈ 0.001 CPU). All pathfinding
    /// (including first-time paths) deducts from this; once exhausted, further
    /// pathfinding returns PathNotFound. Caps total movement CPU to avoid timeouts.
    #[serde(default = "default_pathfinding_cpu_budget")]
    pub pathfinding_cpu_budget: f64,
    /// Hard cap on CPU the movement system may use per tick. Movement stops once
    /// (get_cpu() - start_cpu) >= this value, even if tick limit allows more. Default: 80.0.
    #[serde(default = "default_movement_max_cpu")]
    pub movement_max_cpu: f64,
    /// Bucket level at or above which movement budgets expand from limit() to tick_limit().
    /// Set to 0 to always allow burst (old behavior). Default: 9500.
    #[serde(default = "default_bucket_burst_threshold")]
    pub bucket_burst_threshold: i32,
}

fn default_pathfinding_cpu_budget() -> f64 {
    20.0
}

fn default_movement_max_cpu() -> f64 {
    80.0
}

fn default_bucket_burst_threshold() -> i32 {
    9500
}

impl Default for PathingFeatures {
    fn default() -> Self {
        Self {
            visualize: PathingVisualizeFeatures::default(),
            custom: true,
            reuse_path_length: 20,
            max_shove_depth: 10,
            friendly_creep_distance: 15,
            movement_cpu_budget_pct: 0.3,
            repath_cpu_budget: 5.0,
            pathfinding_cpu_budget: 20.0,
            movement_max_cpu: 80.0,
            bucket_burst_threshold: 9500,
        }
    }
}

impl PathingVisualizeFeatures {
    /// Returns `on && global visualize.on`.
    pub fn enabled(&self, global_visualize: bool) -> bool {
        self.on && global_visualize
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RoomVisualizeFeatures {
    pub on: bool,
}

impl Default for RoomVisualizeFeatures {
    fn default() -> Self {
        Self { on: true }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RoomFeatures {
    pub visualize: RoomVisualizeFeatures,
}

impl RoomVisualizeFeatures {
    /// Returns `on && global visualize.on`.
    pub fn enabled(&self, global_visualize: bool) -> bool {
        self.on && global_visualize
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct MilitaryVisualizeFeatures {
    pub on: bool,
    pub threat_map: bool,
    pub squads: bool,
}

impl MilitaryVisualizeFeatures {
    pub fn threat_map(&self, global_visualize: bool) -> bool {
        self.on && self.threat_map && global_visualize
    }

    pub fn squads(&self, global_visualize: bool) -> bool {
        self.on && self.squads && global_visualize
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct MilitaryFeatures {
    /// Enable defensive operations (squad defense, wall repair, remote defense).
    pub defense: bool,
    /// Master switch for all offensive operations. When false, no attacks are
    /// launched or executed regardless of sub-flags.
    pub offense: bool,
    /// Allow attacking player-owned rooms (resource denial, expansion contests).
    /// Requires `offense` to be enabled.
    pub attack_players: bool,
    /// Allow attacking invader cores, strongholds, and invader bases.
    /// Requires `offense` to be enabled.
    pub attack_invaders: bool,
    /// Request boosts for military creeps.
    pub boost_military: bool,
    /// Allow safe mode activation as last resort.
    pub safe_mode: bool,
    /// Enable nuke defense mission.
    pub nuke_defense: bool,
    /// Enable verbose debug logging for war system (target selection, threat
    /// intel, defense decisions). Useful for diagnosing why attacks are or
    /// aren't being launched.
    pub debug_log: bool,
    /// Visualization settings.
    pub visualize: MilitaryVisualizeFeatures,
}

impl Default for MilitaryFeatures {
    fn default() -> Self {
        Self {
            defense: true,
            // ON by default: live MMO deploy runs offense (invader cores / strongholds
            // / derelict raid+dismantle / SK). `attack_players` stays OFF (no PvP war
            // until the W + identity tracks land). Override via `Memory._features` to
            // disable live without a redeploy. See combat-overhaul-plan.md §4D.
            offense: true,
            attack_players: false,
            attack_invaders: true,
            boost_military: false,
            safe_mode: true,
            nuke_defense: true,
            debug_log: false,
            visualize: MilitaryVisualizeFeatures::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaimFeatures {
    /// Enable the claim operation.
    pub on: bool,
    /// Enable claim debug visualization (panel + map).
    pub visualize: bool,
    /// Maximum number of concurrent room claim missions. 0 = no limit (capped by
    /// GCL/CPU). Default: 2 — allows the pipeline to pursue a second-best
    /// candidate when the top pick is blocked.
    pub max_concurrent_missions: u32,
    /// Maximum score difference (0.0–1.0) between the best candidate and any
    /// additional candidates that may be claimed in the same select cycle.
    /// Prevents picking vastly inferior rooms just to fill the mission cap.
    /// Default: 0.15.
    pub max_score_delta: f32,
    /// Weight applied to the room plan score (from screeps-foreman) when
    /// scoring claim candidates. A good room with a poor layout should be
    /// penalised. Default: 2.0.
    pub plan_score_weight: f32,
    /// Ticks between full BFS re-discovery cycles. Room topology is static and
    /// ownership changes slowly, so this can be long. Default: 500.
    pub discover_interval: u32,
    /// Ticks to wait after discovery for scouts/observers to provide visibility
    /// before selecting claim targets. Must be long enough for scout spawning
    /// (3 ticks) + travel to distance-4 rooms (~200 ticks on swamp). Default: 200.
    pub scouting_window: u32,
    /// Ticks between spawn_remote_build checks. Independent of the claim
    /// pipeline. Default: 50.
    pub remote_build_interval: u32,

    // ── Self-tuning CPU room cap (Workstream A) ─────────────────────────
    /// Fraction of the sustainable CPU limit the empire may project itself
    /// to (incl. one more room) before the dynamic cap stops growth. Leaves
    /// headroom for per-tick fluctuation. Default: 0.85.
    pub cpu_headroom_factor: f32,
    /// Per-room CPU cost assumed before the measured model is warm (or while
    /// the empire is too small for `used / rooms` to be meaningful).
    /// Default: 10.0 (the legacy `ESTIMATED_ROOM_CPU_COST`).
    pub fallback_room_cpu_cost: f32,
    /// Bucket level above which (with `tier==Normal` and a non-negative
    /// trend) the cap allows "probe one more room" beyond the static
    /// estimate — the expand-when-affordable signal. Default: 8000.
    pub healthy_bucket_floor: i32,
    /// Hard floor on the room cap (safety). Default: 1.
    pub min_room_cap: u32,
    /// Hard ceiling on the room cap (safety, independent of GCL/CPU).
    /// Default: 50.
    pub max_room_cap: u32,

    // ── Adaptive radius + cannibalization scoring (Workstream B) ────────
    /// Tightest claim search radius (BFS room-hops) and the policy floor on
    /// claim distance. Derived, not arbitrary: remote mining is radius 1, so
    /// two colonies need ~`2*1 + 2 = 4` hops to avoid overlapping remote
    /// rings. Default: 4. (No max — the upper bound is dynamic via build
    /// feasibility.)
    pub min_search_radius: u32,
    /// Weight of the distance sub-score in candidate scoring. Higher = the
    /// frontier stays tighter. Default: 2.0 (up from the old 0.5).
    pub distance_score_weight: f32,
    /// Multiplicative penalty applied to a distance-1 candidate's total
    /// score — a sourced room one hop away is one an existing room could
    /// remote-mine, so claiming it cannibalizes. Default: 0.3.
    pub adjacent_claim_penalty: f32,

    // ── Threat-aware expansion lifecycle (ADR 0017) ─────────────────────
    /// Master kill-switch for the pre-claim safety gate, the builder threat
    /// guard, and the claimer death-abort. Default: TRUE.
    pub safety_gate: bool,
    /// How recent (ticks) a clean intel read must be to commit a claimer —
    /// "absence of fresh intel is not safety". Default: 250 (> scouting_window
    /// so a candidate scouted during the window passes).
    pub intel_freshness_ticks: u32,
    /// Claimers lost reaching a target before the claim mission aborts it as a
    /// losing battle. Default: 2.
    pub max_claimer_deaths: u32,
    /// Master kill-switch for the colony no-win abort (un-claim of a losing
    /// contested claim). Default: TRUE.
    pub abort_on_contest: bool,
    /// How long (ticks) a sustained player-hostile presence must hold on a
    /// spawnless colony before it is abandoned (anti-flap). Default: 50.
    pub abort_persistence_ticks: u32,
    /// How long (ticks) an abandoned/failed claim target is avoided before it
    /// may be re-selected. Default: 5000.
    pub avoid_cooldown_ticks: u32,
}

impl Default for ClaimFeatures {
    fn default() -> Self {
        Self {
            on: true,
            visualize: false,
            max_concurrent_missions: 2,
            max_score_delta: 0.15,
            plan_score_weight: 2.0,
            discover_interval: 500,
            scouting_window: 200,
            remote_build_interval: 50,
            cpu_headroom_factor: 0.85,
            fallback_room_cpu_cost: 10.0,
            healthy_bucket_floor: 8000,
            min_room_cap: 1,
            max_room_cap: 50,
            min_search_radius: 4,
            distance_score_weight: 2.0,
            adjacent_claim_penalty: 0.3,
            safety_gate: true,
            intel_freshness_ticks: 250,
            max_claimer_deaths: 2,
            abort_on_contest: true,
            abort_persistence_ticks: 50,
            avoid_cooldown_ticks: 5000,
        }
    }
}

/// Treatment of "derelict" rooms: claimed by another player but militarily
/// dead — no spawns, no armed towers, no hostile combat creeps. Consumed by
/// the movement cost callback, the claim / mining-outpost expansion BFS, and
/// the mining-outpost salvage pipeline.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct DerelictFeatures {
    /// Master switch: when false, derelict rooms are treated exactly like any
    /// other hostile-owned room (pathing avoids them, expansion stops at them,
    /// no salvage).
    pub on: bool,
    /// A hostile-owned room must have been observed derelict, without an
    /// intervening militarised sighting, for this many ticks before it is
    /// treated as safe. Guards against trusting a single snapshot (e.g. the
    /// owner's defenders were momentarily elsewhere). Default: 2000 (more than
    /// one creep lifetime).
    pub confirm_ticks: u32,
    /// Maximum intel age (ticks since last visibility) for pathing and
    /// expansion BFS to keep trusting a derelict classification; staler intel
    /// falls back to hostile. Default: 10_000.
    pub path_max_age: u32,
    /// Maximum intel age for committing creeps to act inside the room
    /// (raid/dismantle decisions). Tighter than `path_max_age` because the
    /// creeps will dwell there. Default: 5_000.
    pub action_max_age: u32,
    /// EV margin for spawning dismantlers into a derelict room: estimated
    /// recoverable energy must exceed margin × the energy cost of the
    /// dismantler bodies expected to do the work. Default: 2.0.
    pub dismantle_margin: f32,
    /// Skip dismantle targets with more hits than this. Huge walls/ramparts
    /// would otherwise pin the cleanup phase ~forever and block the
    /// mining-outpost handoff. 0 = no limit. Default: 2_000_000
    /// (~2000 ticks of work for a 20-WORK dismantler).
    pub max_structure_hits: u32,
    /// Maximum linear room distance from a home room for salvage targets.
    /// Default: 2.
    pub salvage_max_range: u32,
    /// Maximum concurrent salvage missions. Default: 1.
    pub salvage_max_missions: u32,
    /// Ticks before a room that failed the salvage EV gate is re-evaluated
    /// (bounds scout/EV churn on stripped or worthless rooms). Default: 10_000.
    pub reject_cooldown: u32,
    /// Allow the de-claimer role in salvage missions: CLAIM creeps that
    /// `attackController` a strategic derelict room's controller to drive it
    /// to neutral, so the waiting mining outpost can take it over (the
    /// controller does not free on its own except by slow natural downgrade).
    /// Kill-switch like `raid`/`dismantle`: off zeroes the role; live
    /// de-claimers finish naturally. Only acts on hostile-owned derelict rooms
    /// that have sources (worth taking over). Default: true.
    pub declaim: bool,
    /// Emit per-room salvage-candidacy diagnostics to the log each scan: for
    /// every hostile/neutral room within `salvage_max_range`, why it is or is
    /// not an admitted salvage target (derelict? confirmed? intel age?
    /// rejected-cooldown?). Off by default — flip on to debug "why isn't this
    /// derelict room being salvaged". Default: false.
    pub diagnostics: bool,
    /// Allow breaching a strategic takeover room's controller even when the
    /// sealing walls/ramparts exceed `max_structure_hits` (the normal dismantle
    /// horizon excludes them, which would otherwise leave the controller
    /// permanently unreachable and the room un-takeable). Breach dismantlers
    /// run at the LOWEST spawn priority and only when the home has surplus
    /// energy and an idle spawn, so the (energy-negative) wall-chewing consumes
    /// only spare capacity. Default: true.
    pub breach_sealed: bool,
    /// Minimum home stored energy (storage+terminal+containers) before breach
    /// dismantlers may spawn — the "excess energy" gate for the breach
    /// campaign. Default: 100_000.
    pub breach_min_home_energy: u32,
}

impl Default for DerelictFeatures {
    fn default() -> Self {
        Self {
            on: true,
            confirm_ticks: 2_000,
            path_max_age: 10_000,
            action_max_age: 5_000,
            dismantle_margin: 2.0,
            max_structure_hits: 2_000_000,
            salvage_max_range: 2,
            salvage_max_missions: 1,
            reject_cooldown: 10_000,
            declaim: true,
            diagnostics: false,
            breach_sealed: true,
            breach_min_home_energy: 100_000,
        }
    }
}

/// Source Keeper room exploitation (ADR 0018): clear/suppress the keepers in an
/// adjacent SK room and mine around them. Default OFF until the sim + a
/// private-server soak validate it (the duo kite, the suppression↔mining gate).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceKeeperFeatures {
    /// Master kill-switch for SK-room farming (clearing + mining). Default **true**
    /// (operator 2026-06-18 — the combat-overhaul behaviors run on by default; we
    /// validate in Docker/sim before an MMO deploy). Set false to disable.
    pub farming: bool,
    /// Max concurrent SK farms (each a suppression duo + miners). Default 1.
    pub max_concurrent_farms: u32,
    /// Max linear room distance (hops) from a home room for an SK candidate. Default 2.
    pub max_range: u32,
    /// Emit per-candidate ROI diagnostics to the log each scan. Default false.
    pub diagnostics: bool,
}

impl Default for SourceKeeperFeatures {
    fn default() -> Self {
        Self {
            farming: true,
            max_concurrent_farms: 1,
            max_range: 2,
            diagnostics: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct VisibilityFeatures {
    /// Enable visibility queue debug visualization (panel).
    pub visualize: bool,
}

/// Harness-only knobs (P1.A5): set from the eval harness via console
/// injection (`Memory._features.eval.* = …`), never by gameplay code.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EvalFeatures {
    /// Synthetic CPU burn per tick, in ms (0 = off). The harness's
    /// pressure scenarios set this to drain the bucket on demand
    /// (component-test-plans §15.4 candidate (i)); the burn happens at
    /// the top of the tick (game_loop) so the governor and the
    /// shedding it drives see honest pressure.
    pub cpu_burn_ms: u32,
    /// Deliberate panic when `game::time() == panic_at_tick` (0 = off)
    /// — the P1.C2 containment acceptance probe. Keyed to an ABSOLUTE
    /// tick rather than a clear-on-fire flag: the panicking tick's
    /// Memory writes are lost with the abort, but time moves past the
    /// trigger during the halt/reload cycle, so it self-disarms.
    pub panic_at_tick: u32,
}

// ─── Top-level features ────────────────────────────────────────────────────────

/// All feature flags, loaded once per tick from `Memory._features`.
///
/// Reset flags are intentionally excluded — they live in a separate path
/// ([`load_reset`] / [`clear_reset`]) because they are consumed before the
/// feature cache is populated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
    pub visualize: VisualizeFeatures,
    pub construction: ConstructionFeatures,
    pub market: MarketFeatures,
    pub transfer: TransferFeatures,
    pub remote_mine: RemoteMineFeatures,
    pub pathing: PathingFeatures,
    pub room: RoomFeatures,
    pub military: MilitaryFeatures,
    /// Allow the raider (loot) role in salvage missions. Turning this off
    /// zeroes the role: no new raiders spawn, live ones finish naturally, and
    /// missions complete once no enabled work remains. Default: true.
    pub raid: bool,
    pub claim: ClaimFeatures,
    pub derelict: DerelictFeatures,
    pub source_keeper: SourceKeeperFeatures,
    pub visibility: VisibilityFeatures,
    /// Allow the dismantler role in salvage missions; semantics as `raid`.
    /// Default: true.
    pub dismantle: bool,
    /// Log per-system CPU timing for each ECS system in the game loop.
    /// When enabled, each system's CPU cost is measured and logged at info level.
    pub system_timing: bool,
    /// Harness-only fault-injection knobs (P1.A5).
    pub eval: EvalFeatures,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            visualize: VisualizeFeatures::default(),
            construction: ConstructionFeatures::default(),
            market: MarketFeatures::default(),
            transfer: TransferFeatures::default(),
            remote_mine: RemoteMineFeatures::default(),
            pathing: PathingFeatures::default(),
            room: RoomFeatures::default(),
            military: MilitaryFeatures::default(),
            raid: true,
            claim: ClaimFeatures::default(),
            derelict: DerelictFeatures::default(),
            source_keeper: SourceKeeperFeatures::default(),
            visibility: VisibilityFeatures::default(),
            dismantle: true,
            system_timing: false,
            eval: EvalFeatures::default(),
        }
    }
}

// ─── JS helpers (private) ──────────────────────────────────────────────────────

#[inline]
fn js_get(parent: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(parent, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

#[inline]
fn js_bool(parent: &JsValue, key: &str) -> bool {
    js_get(parent, key).as_bool().unwrap_or(false)
}

/// Deserialize the `_features` object from Memory, falling back to defaults for
/// any missing or malformed values.
fn features_from_memory() -> Features {
    let root = crate::memory_helper::root();
    let js_features = js_get(&root, "_features");

    if js_features.is_undefined() || js_features.is_null() {
        Features::default()
    } else {
        serde_wasm_bindgen::from_value(js_features).unwrap_or_default()
    }
}

// ─── Prepare / Load ────────────────────────────────────────────────────────────

/// Ensure `Memory._features` exists with sensible defaults.
///
/// Reads the existing `_features` tree (deserializing with defaults for any
/// missing keys), then serializes the complete struct back to Memory. This
/// means any keys that were absent are now visible in the console with their
/// default values, while existing operator overrides are preserved.
///
/// Called once during environment creation.  The same read-merge-write logic
/// also runs every tick inside [`load`], so user edits to Memory between ticks
/// are always picked up.
pub fn prepare() {
    let _ = load();
}

/// Load all feature flags from `Memory._features` and return them.
///
/// Called once at the start of every game tick, after resets have been
/// handled; the game loop inserts the returned [`Features`] into the world
/// as a Resource (statics-review M5 — the thread-local cache is gone;
/// systems fetch `Read<Features>`, mission/operation code reads the copy on
/// its execution system data). The resolved flags (with defaults filled in
/// for any missing keys) are written back to Memory so the user can always
/// see and modify the complete set of feature flags in the console between
/// ticks.
#[must_use]
pub fn load() -> Features {
    let root = crate::memory_helper::root();
    let flags = features_from_memory();

    // Write the fully-resolved struct back so new/missing keys are visible in
    // Memory for the user to inspect and modify between ticks.
    if let Ok(js_val) = serde_wasm_bindgen::to_value(&flags) {
        let _ = js_sys::Reflect::set(&root, &JsValue::from_str("_features"), &js_val);
    }

    // Ensure reset sub-object also exists with defaults.
    let js_features = js_get(&root, "_features");
    let reset = js_get(&js_features, "reset");
    if reset.is_undefined() || reset.is_null() {
        let obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("environment"), &JsValue::from_bool(false));
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("memory"), &JsValue::from_bool(false));
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("room_plans"), &JsValue::from_bool(false));
        let _ = js_sys::Reflect::set(js_features.as_ref(), &JsValue::from_str("reset"), &obj);
    }

    flags
}
