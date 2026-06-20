//! Pure per-tile pricing for squad kite/flee positioning (P2.G3-tail, ADR 0008 §4.1 / §5).
//!
//! This is the **combat pricing** the pathfinding system consumes — NOT a search. The search is
//! `screeps-rover`'s pure `LocalPathfinder` (`search_scored`); the squad runs **one** bounded
//! search pricing each reached tile with [`score_tile`], so "stay cohesive" and "find a safe,
//! higher-expected-value position" are **one** scoring function (the cohesion term lives in the
//! score, not a separate per-creep clamp — the operator's "don't be disorganized with squads").
//!
//! `score_tile` returns a **cost (lower = better)** so it drops straight into the min-scored search
//! with no sign juggling. Pure over `screeps::Position` — no `game::*`, no serialization. The weights
//! are tunable params (the ADR 0008a EXP-* loop tunes them on the sim).

use screeps::local::{LocalCostMatrix, RoomXY};
use screeps::{Position, RoomName};
use screeps_rover::{LocalPathfinder, ReachSource, ReachabilityMap};

/// Op budget for the per-squad kite search — a local ~window flood, not a full-room path. Bounded
/// so it costs ~one cheap search per squad per kiting tick (and degrades gracefully on exhaustion).
pub const MAX_KITE_OPS: u32 = 400;

/// Survival-veto horizon (ADR 0019 Guard 4): a tile is *lethal* — and rejected as a goal — if the net
/// incoming damage there would kill the squad's most-fragile member in fewer than this many ticks
/// (`net * H_MIN > fragile_hits`). Even an ENGAGE objective never positions onto a suicide tile.
pub const SURVIVAL_HORIZON: i32 = 3;
/// Cost added to a tile that fails the survival veto — large enough to dominate any normal score so a
/// lethal tile is only chosen when every reachable tile is lethal (best-effort least-bad fallback).
const LETHAL_TILE_PENALTY: i64 = 1_000_000_000;

const FIELD_DIM: usize = 50;
fn field_idx(x: u8, y: u8) -> usize {
    x as usize * FIELD_DIM + y as usize
}

/// The per-(room, tick) **integer threat field** (ADR 0019 §2.1, Stage 3b): raw incoming hits/tick a
/// creep would take standing on each tile, from enemy creeps (melee stamped over range 1, ranged over
/// range 3) and towers (range falloff). All `i32` — powers are integer engine constants, so the field
/// is exact + order-independent. A *layer* in [`PositionLayers`], built once and read by the survival
/// veto + (next) the normalized safety term — shared across every position consumer this tick.
///
/// v1 stamps **unboosted** output (working ATTACK×30 / RANGED_ATTACK×10 / tower falloff) and does NOT
/// model boosted-TOUGH damage reduction (a lategame follow-up needing the boost field) — correct for
/// the common unboosted case, conservative (over-counts) for a boosted TOUGH defender.
pub struct ThreatField {
    dmg: Box<[i32; FIELD_DIM * FIELD_DIM]>,
}

impl ThreatField {
    /// Stamp the field from the threats' weapon output + the towers' falloff (pure geometry — Screeps
    /// ranged/tower damage needs range, not line-of-sight, so walls don't gate it).
    pub fn build(threats: &[KiteThreat], towers: &[KiteTower]) -> Self {
        let mut dmg = Box::new([0i32; FIELD_DIM * FIELD_DIM]);
        let stamp = |dmg: &mut [i32; FIELD_DIM * FIELD_DIM], cx: i32, cy: i32, range: i32, power: i32| {
            for x in (cx - range).max(0)..=(cx + range).min(FIELD_DIM as i32 - 1) {
                for y in (cy - range).max(0)..=(cy + range).min(FIELD_DIM as i32 - 1) {
                    dmg[field_idx(x as u8, y as u8)] += power;
                }
            }
        };
        for t in threats {
            let (tx, ty) = (t.pos.x().u8() as i32, t.pos.y().u8() as i32);
            if t.attack_power > 0 {
                stamp(&mut dmg, tx, ty, 1, t.attack_power as i32);
            }
            if t.ranged_power > 0 {
                stamp(&mut dmg, tx, ty, 3, t.ranged_power as i32);
            }
        }
        // Tower falloff is range-only → a per-range LUT (computed once) makes each of the per-tower
        // whole-room stamps an array read instead of a function call + clamp.
        if !towers.is_empty() {
            let lut: [i32; FIELD_DIM] =
                std::array::from_fn(|r| screeps_combat_engine::damage::tower_attack_damage_at_range(r as u32) as i32);
            for tw in towers {
                let (wx, wy) = (tw.pos.x().u8() as i32, tw.pos.y().u8() as i32);
                for x in 0..FIELD_DIM as i32 {
                    for y in 0..FIELD_DIM as i32 {
                        let r = (x - wx).abs().max((y - wy).abs()) as usize;
                        dmg[field_idx(x as u8, y as u8)] += lut[r];
                    }
                }
            }
        }
        Self { dmg }
    }

    /// Raw incoming hits/tick at `tile`.
    pub fn raw_at(&self, tile: Position) -> i32 {
        self.dmg[field_idx(tile.x().u8(), tile.y().u8())]
    }
}

/// How a hostile threatens a tile — drives the safety "danger reach".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreatKind {
    /// Melee-only (must be kept beyond range 1; we keep beyond `reach` to be safe).
    MeleeOnly,
    /// Has ranged attack (dangerous out to ~range 3).
    Ranged,
}

/// A hostile as the kite scorer sees it.
#[derive(Clone, Copy, Debug)]
pub struct KiteThreat {
    pub pos: Position,
    pub kind: ThreatKind,
    /// Danger reach (tiles): stand beyond this. Melee kiters keep ~3 from a melee-only chaser;
    /// ranged threats reach ~3.
    pub reach: u32,
    /// Fatigue cadence: ticks this threat spends to step one tile on plain terrain (`>=1`; lower =
    /// faster). `None` ⇒ immobile (no working MOVE) → not a chaser, so it seeds no reachability wave
    /// (ADR 0019 Guard 5), though it still contributes present danger via `reach`.
    pub step_ticks: Option<u32>,
    /// Melee attack output (working ATTACK parts × power) — stamped over range 1 into the [`ThreatField`].
    pub attack_power: u32,
    /// Ranged attack output (working RANGED_ATTACK parts × power) — stamped over range 3.
    pub ranged_power: u32,
}

/// A hostile tower (its damage falls off with range — see the engine's
/// `tower_attack_damage_at_range`, the single source of truth for the tower curve).
#[derive(Clone, Copy, Debug)]
pub struct KiteTower {
    pub pos: Position,
}

/// Tunable weights for [`score_tile`] (ADR 0019 Stage 3b final shape). Every term the weight scales
/// is **normalized to `[0, SCALE]`** first, so a weight is a pure dimensionless mixing ratio and the
/// objective preset (kite vs engage) is just a different weight vector — *flee* and *stand* emerge
/// from one scorer. Seeds only; the EXP-* sim loop is the sanctioned tuner.
#[derive(Clone, Copy, Debug)]
pub struct KiteScoreParams {
    /// Present **incoming damage** (the threat field, net of self-heal, per fragile HP).
    pub w_taken: f32,
    /// **Future exposure** — how soon a mobile chaser reaches the tile (reachability).
    pub w_future: f32,
    /// **Cohesion** — wall-aware distance from the squad centroid, steepening past K.
    pub w_cohesion: f32,
    /// **Proximity** — the focus being beyond optimal weapon range `r*` (pull into range).
    pub w_prox: f32,
    /// **Openness** — dead-end avoidance (fewer walkable neighbours).
    pub w_openness: f32,
    /// **Edge-trap** — penalize tiles near a room edge while threatened (anti-corner for the scored
    /// path; complements the per-creep flee's edge repulsors).
    pub w_edge: f32,
    /// **Focus-damage reward** (negative cost) — a tile in weapon range of the focus. Kite weights it
    /// 0 (safety → flee); engage weights it high (commit to dealing damage → stand and fight).
    pub w_dmg: f32,
    /// Cohesion radius K: beyond this distance from the centroid the penalty steepens (×3/tile).
    pub max_cohesion_radius: u32,
}

impl Default for KiteScoreParams {
    /// The **kite / retreat** preset: safety dominates → flee danger; the future term pushes off tiles
    /// a chaser reaches imminently; the proximity term holds the focus in weapon range (so the kiter
    /// stands off at `r*`, shooting, rather than fleeing past shooting range).
    fn default() -> Self {
        Self {
            w_taken: 2.0,
            w_future: 1.0,
            w_cohesion: 0.3,
            w_prox: 0.5,
            w_openness: 0.05,
            w_edge: 0.4,
            w_dmg: 0.0,
            max_cohesion_radius: 2,
        }
    }
}

impl KiteScoreParams {
    /// The **engage** preset: the SAME scorer reweighted to *stand and fight* — safety still matters
    /// but no longer dominates, the focus-damage reward dominates (commit to dealing damage), and the
    /// future-threat (kite-away) term is off.
    ///
    /// `w_prox` is the **advance-to-damage** layer and is deliberately the strongest term: the kite
    /// search is a *bounded* flood, so a focus beyond its horizon is never reached — the squad must
    /// march toward it. A dominant proximity weight makes the flood's best-effort tile the one closest
    /// to the focus (the squad advances each tick), and because proximity is **0 once inside `r*`** it
    /// vanishes on arrival, handing positioning to the safety/cohesion/DMG terms. So ONE search both
    /// closes the distance AND picks the engage tile — no separate "approach vs position" branch (the
    /// survival veto still forbids marching onto a lethal tile). Seeds; the EXP-* loop tunes them.
    pub fn engage() -> Self {
        Self {
            w_taken: 0.5,
            w_future: 0.0,
            w_cohesion: 0.3,
            w_prox: 1.5,
            w_openness: 0.05,
            w_edge: 0.1,
            w_dmg: 2.0,
            max_cohesion_radius: 2,
        }
    }
}

/// The squad's two position-scoring presets — the **tunable seam for the EXP-\* sweep loop** (ADR 0019
/// Stage 4, measure-first). `Default` returns the shipped presets ([`KiteScoreParams::default`] for
/// kiting/fleeing, [`KiteScoreParams::engage`] for standing and fighting); the live bot always uses
/// `Default`, while the sim sweep injects custom weight vectors to tune them empirically against the
/// EXP metrics before any new defaults are baked in.
#[derive(Clone, Copy, Debug)]
pub struct SquadTacticParams {
    /// Weights for the kite/flee/retreat scored search (`decide_squad_with_pathing`'s kite branch).
    pub kite: KiteScoreParams,
    /// Weights for the engage/advance scored search (the engage branch).
    pub engage: KiteScoreParams,
}

impl Default for SquadTacticParams {
    fn default() -> Self {
        Self { kite: KiteScoreParams::default(), engage: KiteScoreParams::engage() }
    }
}

/// How many ticks ahead the future-threat term looks: a tile a chaser reaches in `>= FUTURE_HORIZON`
/// ticks is "far enough" and gets no future penalty; nearer arrivals scale up linearly. Kept small (a
/// chaser is only an imminent danger a couple ticks out) so a kiter HOLDS weapon range rather than
/// fleeing to the horizon — durable standoff at `r*`, not retreat. The proximity term pulls back in.
pub const FUTURE_HORIZON: u32 = 3;

/// The normalized band every score term is mapped into before weighting (ADR 0019 §1.2): a weight is
/// then a pure dimensionless mixing ratio across comparable [0, SCALE] terms.
const SCALE_F: f32 = 1000.0;
/// Max in-room Chebyshev distance — normalizes the distance-based terms (cohesion, proximity).
const ROOM_DIAM_F: f32 = 49.0;
/// "Very dangerous" reference for the no-field reach-depth+tower safety proxy (term-isolation tests).
const SAFETY_PROXY_REF: f32 = 8.0;
/// Tiles from a room edge under which the edge-trap term applies (matches the per-creep flee repulsor).
pub const EDGE_THRESH: u32 = 6;
/// Min distance from `(x,y)` to any room edge (0 on an edge, up to 24 at the centre).
fn dist_to_edge(x: u8, y: u8) -> u32 {
    (x.min(49 - x)).min(y.min(49 - y)) as u32
}

/// Weight of the proximity beeline tie-break (perpendicular deviation from the centroid→focus line).
/// Small so it only resolves the flat Chebyshev iso-range plateau toward the approach line — never
/// overrides the dominant Chebyshev distance ordering (a closer ring always beats a farther one).
const PROX_BEELINE_WEIGHT: f32 = 0.25;
/// Cap (tiles) on the perpendicular deviation fed to the beeline tie-break: with the weight above this
/// bounds its contribution well under one Chebyshev ring step, so cross-ring ordering stays intact
/// (gross off-line deviation is the cohesion term's job, not proximity's).
const PROX_BEELINE_CAP: f32 = 3.0;

/// Perpendicular distance (tiles) of `tile` from the line through `from` toward `to` — 0 on the line,
/// growing off it. The proximity term's beeline tie-break: among tiles at equal Chebyshev range to the
/// focus, the one on the centroid→focus approach line is preferred, so the advance heads straight at
/// the focus (at any angle) instead of drifting to an iso-range corner. 0 when `from == to`.
fn perp_offset(from: Position, to: Position, tile: Position) -> f32 {
    let (cx, cy) = (from.x().u8() as f32, from.y().u8() as f32);
    let (dx, dy) = (to.x().u8() as f32 - cx, to.y().u8() as f32 - cy);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON {
        return 0.0;
    }
    // |cross((to-from), (tile-from))| / |to-from| — the point-to-line distance.
    (dx * (tile.y().u8() as f32 - cy) - dy * (tile.x().u8() as f32 - cx)).abs() / len
}

/// The shared per-room maps [`plan_kite_anchor`] builds once and every priced tile reads (ADR 0019
/// Stage 2). All-`None` ⇒ the pre-Stage-2 behavior (Chebyshev cohesion, no future term) — exactly
/// byte-identical, which the unit tests rely on.
#[derive(Default, Clone, Copy)]
pub struct KiteFields<'a> {
    /// Incoming hits/tile (creeps + towers) → the **safety** term (net of self-heal, per fragile HP).
    /// `None` ⇒ the reach-depth+tower proxy (the pre-Stage-3b safety, for term-isolation unit tests).
    pub threat_field: Option<&'a ThreatField>,
    /// Soonest a mobile chaser reaches each tile → the **future-threat** term. `None` ⇒ omitted.
    pub threat_reach: Option<&'a ReachabilityMap>,
    /// Wall-aware path distance (in tiles) from the squad centroid to this tile → the **cohesion**
    /// term. `None` ⇒ Chebyshev fallback. The scored search floods *from the centroid*, so it already
    /// computes this as its path-cost `g`: [`plan_kite_anchor`] threads that `g` in here per tile
    /// (ADR 0019 Stage 3b flood-dedup — no separate centroid flood). In open terrain `g` equals
    /// Chebyshev (8-dir uniform steps); it differs — correctly — only around walls.
    pub cohesion_dist: Option<u32>,
}

/// The per-(room, tick) **cached layer set** (ADR 0019 Stage 3a/3b, operator architecture): the
/// expensive shared computation — the threat-chaser reachability flood — built **once** and **reused
/// across every position consumer** this tick. Different objectives *and uses* (kite / attack-
/// positioning / defend) are weight vectors over the SAME layers, so the flood runs once regardless of
/// how many scorers read it — a scorer borrows a [`KiteFields`] view via [`PositionLayers::fields`]
/// and threads the *per-tile* cohesion distance from its own search `g` (Stage 3b flood-dedup — no
/// separate centroid flood). (Stage 3b adds the integer threat field + focus-damage layers here.)
pub struct PositionLayers {
    threat_reach: Option<ReachabilityMap>,
    threat_field: ThreatField,
}

impl PositionLayers {
    /// Build the layers once for a squad facing `threats` (+ `towers`), over the room's cost matrix.
    /// Use-agnostic (only the shared inputs) so the attack-positioning path reuses the exact same
    /// instance: the reachability flood (future-threat) + the integer [`ThreatField`] (incoming hits)
    /// are built once and read by every consumer this tick.
    pub fn build(threats: &[KiteThreat], towers: &[KiteTower], room: RoomName, matrix: &LocalCostMatrix, max_ops: u32) -> Self {
        let mut cb = |_r: RoomName| Some(matrix.clone());
        // THREAT reachability: mobile chasers only (Guard 5 — an immobile threat seeds no wave).
        let sources: Vec<ReachSource> = threats
            .iter()
            .filter_map(|t| t.step_ticks.map(|st| ReachSource { pos: t.pos, step_ticks: st }))
            .collect();
        let threat_reach =
            (!sources.is_empty()).then(|| LocalPathfinder.reachability_from(&sources, room, &mut cb, max_ops));
        let threat_field = ThreatField::build(threats, towers);
        Self { threat_reach, threat_field }
    }

    /// The shared integer threat field (incoming hits/tile) — the survival-veto + safety input.
    pub fn threat_field(&self) -> &ThreatField {
        &self.threat_field
    }

    /// The shared threat-reachability layer (the future-threat input).
    pub fn threat_reach(&self) -> Option<&ReachabilityMap> {
        self.threat_reach.as_ref()
    }

    /// A borrowing [`KiteFields`] view over the cached layers. `cohesion_dist` is left `None` — the
    /// caller threads the per-tile search `g` in (the search floods from the centroid, so `g` *is* the
    /// wall-aware cohesion distance).
    pub fn fields(&self) -> KiteFields<'_> {
        KiteFields { threat_field: Some(&self.threat_field), threat_reach: self.threat_reach.as_ref(), cohesion_dist: None }
    }
}

/// The squad's offensive capacity against its shared focus, for the **actual-hits** DMG reward (ADR
/// 0019 Stage 3b focus_damage richness). The flat "in weapon range → full reward" was range-blind and
/// heal-blind; with this the DMG term rewards a tile by the *net hits the squad would actually land on
/// the focus from there* — melee output only lands at range 1, ranged within `r*`, and the focus's
/// nearby heal is subtracted — scaled up by **kill-priority** (a near-dead focus is worth committing
/// to). `None` ⇒ the flat in-range reward (the term-isolation unit tests + the sim path that doesn't
/// resolve focus HP), so those stay byte-identical.
#[derive(Clone, Copy, Debug)]
pub struct FocusDamage {
    /// The squad's melee output/tick (working ATTACK parts × power) — lands only at range 1.
    pub melee_power: u32,
    /// The squad's ranged output/tick (working RANGED_ATTACK parts × power) — lands within `r*`.
    pub ranged_power: u32,
    /// The focus's current hits — kill-priority denominator (net/hits → 1 = killable this tick).
    pub focus_hits: u32,
    /// Aggregate heal/tick reaching the focus (enemy healers near it) — subtracted from dealt.
    pub focus_heal: u32,
}

/// The squad's kite-scoring context — its centroid (cohesion anchor), the threats/towers to avoid,
/// and the shared focus the value term keeps shootable.
pub struct SquadKiteView<'a> {
    pub centroid: Position,
    pub threats: &'a [KiteThreat],
    pub towers: &'a [KiteTower],
    /// Shared focus position; the value term keeps it within shooting range 3.
    pub focus: Option<Position>,
    /// The actual-hits inputs for the DMG reward (ADR 0019 focus_damage richness). `None` ⇒ the flat
    /// in-`r*` reward (byte-identical to the pre-richness behavior, for unit tests + the basic sim).
    pub focus_damage: Option<FocusDamage>,
    pub params: KiteScoreParams,
    /// Hits of the squad's most-fragile member (ADR 0019 #2/#4) — the safety/veto denominator.
    pub fragile_hits: u32,
    /// The squad's total heal output per tick — the sustain subtracted from incoming damage.
    pub squad_heal: u32,
    /// Optimal weapon range `r*` (3 ranged, 1 melee) — the proximity + focus-damage terms use it so a
    /// melee/siege block closes to range 1 while a ranged block holds range 3.
    pub weapon_range: u32,
}

/// Cost of the squad standing its block on `tile` — **LOWER is better** (composes directly with
/// rover's min-scored search). The ADR 0019 Stage-3b unified utility: every term is normalized to
/// `[0, SCALE]` so the objective-preset weights are dimensionless mixing ratios, and *flee* (kite
/// weights) vs *stand* (engage weights) emerge from this ONE function. Terms:
/// - **TAKEN** (safety): the threat field's net incoming hits per fragile-HP (creeps + towers; #2/#3);
/// - **FUTURE**: how soon a mobile chaser reaches the tile (reachability);
/// - **COHESION**: wall-aware distance from the centroid, steepening past K;
/// - **PROXIMITY**: the focus beyond optimal weapon range `r*`;
/// - **OPENNESS**: dead-end avoidance;
/// - **EDGE**: edge proximity while threatened (anti-corner);
/// - **DMG** (reward): in weapon range of the focus (engage commits to damage).
///
/// `fields` carries the shared per-(room,tick) layers. With no `threat_field` (term-isolation tests)
/// the safety term falls back to the reach-depth+tower proxy (also normalized).
pub fn score_tile(view: &SquadKiteView, tile: Position, walkable_neighbors: u8, fields: &KiteFields) -> i64 {
    let p = &view.params;
    let r_star = view.weapon_range.max(1);

    // TAKEN (safety) — incoming damage, normalized to [0,SCALE]. With the threat field (live path):
    // net hits (raw − self-heal) as a fraction of the most-fragile member's HP (#2) — so RANGED
    // threats and TOWERS shape positioning, which the reach-depth proxy ignored (ranged has reach 0).
    // Without the field (unit tests): the reach-depth+tower proxy, normalized by SAFETY_PROXY_REF.
    let safety = match fields.threat_field {
        Some(tf) if view.fragile_hits > 0 => {
            let net = (tf.raw_at(tile) - view.squad_heal as i32).max(0);
            (net as f32 / view.fragile_hits as f32 * SCALE_F).min(SCALE_F)
        }
        _ => {
            let mut raw = 0.0f32;
            for t in view.threats {
                let r = tile.get_range_to(t.pos);
                if r <= t.reach {
                    raw += (t.reach + 1 - r) as f32;
                }
            }
            for tw in view.towers {
                let r = tile.get_range_to(tw.pos);
                raw += screeps_combat_engine::damage::tower_attack_damage_at_range(r) as f32 / 150.0;
            }
            (raw / SAFETY_PROXY_REF * SCALE_F).min(SCALE_F)
        }
    };

    // FUTURE — penalize tiles a mobile chaser reaches soon (low ticks-to-reach), normalized.
    let future = match fields.threat_reach {
        Some(map) => {
            let ttr = map.ticks_xy(tile.x().u8(), tile.y().u8());
            if ttr < FUTURE_HORIZON {
                (FUTURE_HORIZON - ttr) as f32 / FUTURE_HORIZON as f32 * SCALE_F
            } else {
                0.0
            }
        }
        None => 0.0,
    };

    // COHESION — wall-aware distance from the centroid (the search's `g` when present, else Chebyshev),
    // steepening past K, normalized.
    let d = fields.cohesion_dist.unwrap_or_else(|| tile.get_range_to(view.centroid));
    let coh_raw = if d <= p.max_cohesion_radius {
        d as f32
    } else {
        p.max_cohesion_radius as f32 + (d - p.max_cohesion_radius) as f32 * 3.0
    };
    let cohesion = (coh_raw / ROOM_DIAM_F * SCALE_F).min(SCALE_F);

    // PROXIMITY — the focus being beyond optimal weapon range `r*` (the **advance-to-damage** layer:
    // pull the block toward the focus until it's in weapon range). The in-range test is Chebyshev (the
    // engine's weapon range). The distance is **Chebyshev** — Screeps charges equal cost for diagonal
    // and cardinal steps, so ticks-to-reach / weapon range / `get_range_to` are all Chebyshev (NOT
    // euclidean). But Chebyshev's square iso-range rings are flat plateaus: the deterministic tie-break
    // would walk the squad to a ring corner, drifting it perpendicular to the focus (and, with terrain,
    // off a corridor's mouth). So add a SMALL perpendicular-deviation tie-break from the centroid→focus
    // line — it beelines the advance along the real approach line at any angle, while the dominant
    // Chebyshev term keeps the (movement-correct) cross-ring ordering. 0 once in range.
    let prox = match view.focus {
        Some(f) => {
            let cheb = tile.get_range_to(f);
            if cheb <= r_star {
                0.0
            } else {
                let perp = perp_offset(view.centroid, f, tile).min(PROX_BEELINE_CAP);
                (((cheb - r_star) as f32 + PROX_BEELINE_WEIGHT * perp) / ROOM_DIAM_F * SCALE_F).min(SCALE_F)
            }
        }
        None => 0.0,
    };

    // OPENNESS — penalize dead-ends, normalized.
    let openness = (8 - walkable_neighbors.min(8)) as f32 / 8.0 * SCALE_F;

    // EDGE — near a room edge while a threat is present → anti-corner for the scored path (the per-
    // creep flee uses repulsors; the squad scored search uses this term). Off when no threats.
    let edge = if view.threats.is_empty() {
        0.0
    } else {
        let de = dist_to_edge(tile.x().u8(), tile.y().u8());
        if de < EDGE_THRESH {
            (EDGE_THRESH - de) as f32 / EDGE_THRESH as f32 * SCALE_F
        } else {
            0.0
        }
    };

    // DMG (reward) — committing to deal damage to the focus (engage). With FocusDamage (live path):
    // the ACTUAL net hits landed from this tile — melee output only at range 1, ranged within `r*`,
    // minus the focus's nearby heal — as a fraction of the squad's max output (so a melee block is
    // pulled to range 1 where it lands more), plus a kill-priority bonus (net relative to the focus's
    // hits → saturates the reward when the focus is killable this tick). 0 when out-healed (don't
    // commit to an unkillable target → safety dominates → reposition). Without it (tests / basic sim):
    // the flat in-`r*` reward (byte-identical to pre-richness). 0 in the kite preset (`w_dmg == 0`).
    let focus_dmg = match (view.focus, view.focus_damage) {
        (Some(f), Some(fd)) => {
            let r = tile.get_range_to(f);
            let dealt = if r <= 1 {
                fd.melee_power + fd.ranged_power
            } else if r <= r_star {
                fd.ranged_power
            } else {
                0
            };
            if dealt == 0 {
                0.0
            } else {
                let net = dealt.saturating_sub(fd.focus_heal);
                let best = (fd.melee_power + fd.ranged_power).max(1);
                // EFFECTIVENESS: net as a fraction of our max output (range-1 beats range-3 when melee
                // > 0 → pulls a melee block in). KILL-PRIORITY: net relative to the focus's hits, capped
                // at 1 (killable-this-tick). Convex blend → stays in [0, SCALE]; both → 0 when out-
                // healed (net 0) so safety dominates and the squad disengages an unkillable target.
                let eff = (net as f32 / best as f32).min(1.0);
                let kill = (net as f32 / fd.focus_hits.max(1) as f32).min(1.0);
                ((0.6 * eff + 0.4 * kill) * SCALE_F).min(SCALE_F)
            }
        }
        (Some(f), None) => {
            if tile.get_range_to(f) <= r_star {
                SCALE_F
            } else {
                0.0
            }
        }
        _ => 0.0,
    };

    (p.w_taken * safety
        + p.w_future * future
        + p.w_cohesion * cohesion
        + p.w_prox * prox
        + p.w_openness * openness
        + p.w_edge * edge
        - p.w_dmg * focus_dmg)
        .round() as i64
}

/// A planned kite/flee goal for the whole squad — the single tile every in-cohesion member targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KitePlan {
    pub goal: Position,
}

/// Count the walkable (not-impassable) neighbours of `tile` in the cost matrix (0–8) — the openness
/// input to [`score_tile`] (fewer exits ⇒ nearer a dead-end).
fn walkable_neighbors(cm: &LocalCostMatrix, tile: Position) -> u8 {
    let (x, y) = (tile.x().u8() as i32, tile.y().u8() as i32);
    let mut n = 0u8;
    for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
        let (nx, ny) = (x + dx, y + dy);
        if (0..50).contains(&nx) && (0..50).contains(&ny) {
            if let Ok(xy) = RoomXY::checked_new(nx as u8, ny as u8) {
                if cm.get(xy) != u8::MAX {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Plan ONE kite/flee goal for the whole squad: a single bounded `LocalPathfinder::search_scored`
/// from the centroid, pricing each reached tile with [`score_tile`] (safety + future-threat +
/// cohesion + value + openness). `None` ⇒ holding the centroid is already optimal (members
/// hold/shoot). This is the squad's ONE bounded search per kiting tick (members reuse the goal via
/// their own move request) — combat supplies the pricing, rover owns the search (no one-off rule).
///
/// First builds the shared per-room **reachability map** (ADR 0019 Stage 2) — one multi-source flood
/// seeded by the mobile chasers (`step_ticks.is_some()`) — so the future-threat term can prefer tiles
/// with durable standoff. Both the flood and the goal search run over the *same* matrix, live and sim.
///
/// `room_callback` supplies the room's movement cost matrix (terrain walls baked in): the **same**
/// `LocalPathfinder` runs live and in the sim — only the matrix source differs (the live
/// `CostMatrixSystem` vs the sim's synthetic). Kiting is single-room by nature (cross-room travel is
/// the separate `MoveToRoom` phase), so the headless local search is the right tool for both.
pub fn plan_kite_anchor(
    view: &SquadKiteView,
    shared: Option<&PositionLayers>,
    room_callback: &mut dyn FnMut(RoomName) -> Option<LocalCostMatrix>,
    max_ops: u32,
) -> Option<KitePlan> {
    let room = view.centroid.room_name();
    let matrix = room_callback(room)?;
    // The per-(room,tick) cached layers (threat field + reachability). When the caller built them once
    // for the room (ADR 0019 Stage 3b build-once-per-room sharing), reuse that instance across every
    // squad in the room; otherwise build them here (the sim / tests). Cohesion comes free from the
    // search's own `g` below (the flood-dedup), so it stays per-squad regardless.
    let owned;
    let layers = match shared {
        Some(l) => l,
        None => {
            owned = PositionLayers::build(view.threats, view.towers, room, &matrix, max_ops);
            &owned
        }
    };
    let threat_reach = layers.threat_reach();
    let threat_field = layers.threat_field();
    // SURVIVAL VETO (ADR 0019 Guard 4 / #4): a tile whose net incoming would kill the most-fragile
    // member in < SURVIVAL_HORIZON ticks is lethal — price it astronomically so the search avoids it
    // unless every tile is lethal (then the least-bad is the best-effort fallback). Even ENGAGE never
    // walks onto a suicide tile. `0` fragile_hits (no members info) ⇒ veto disabled.
    let veto = |tile: Position| -> bool {
        if view.fragile_hits == 0 {
            return false;
        }
        let net = (threat_field.raw_at(tile) - view.squad_heal as i32).max(0);
        net > 0 && net * SURVIVAL_HORIZON > view.fragile_hits as i32
    };
    // `g` is the search's path-cost from the centroid = the wall-aware cohesion distance for this tile.
    let cost = |tile: Position, g: u32| -> i64 {
        let fields = KiteFields { threat_field: Some(threat_field), threat_reach, cohesion_dist: Some(g) };
        let base = score_tile(view, tile, walkable_neighbors(&matrix, tile), &fields);
        if veto(tile) {
            base.saturating_add(LETHAL_TILE_PENALTY)
        } else {
            base
        }
    };
    // Feed the search the already-fetched matrix (so the openness lookup + the search agree).
    let mut cb = |_r: RoomName| Some(matrix.clone());
    let result = LocalPathfinder.search_scored(view.centroid, &mut cb, max_ops, 1, &cost);
    result.path.last().copied().map(|goal| KitePlan { goal })
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{RoomCoordinate, RoomName};

    /// ADR 0019 Stage 1 — kiting now delegates the tower curve to the engine (the local duplicate was
    /// deleted after a bit-identity proof over all in-room ranges 0..=49). This pins the values the
    /// kite safety term depends on, so a future engine-curve change that would shift kiting is caught.
    #[test]
    fn kite_uses_the_engine_tower_curve() {
        use screeps_combat_engine::damage::tower_attack_damage_at_range as t;
        assert_eq!(t(0), 600, "point-blank = optimal");
        assert_eq!(t(5), 600, "optimal range edge");
        assert_eq!(t(10), 450, "mid falloff");
        assert_eq!(t(20), 150, "min-damage floor");
        assert_eq!(t(49), 150, "beyond falloff stays at the floor");
    }

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }
    fn view<'a>(centroid: Position, threats: &'a [KiteThreat], towers: &'a [KiteTower], focus: Option<Position>) -> SquadKiteView<'a> {
        // fragile_hits 0 ⇒ the survival veto is disabled (the term-isolation tests don't want it).
        SquadKiteView { centroid, threats, towers, focus, focus_damage: None, params: KiteScoreParams::default(), fragile_hits: 0, squad_heal: 0, weapon_range: 3 }
    }
    fn melee(x: u8, y: u8, reach: u32) -> KiteThreat {
        KiteThreat { pos: pos(x, y), kind: ThreatKind::MeleeOnly, reach, step_ticks: Some(1), attack_power: 30, ranged_power: 0 }
    }


    #[test]
    fn safety_penalizes_being_inside_a_threats_reach() {
        let c = pos(25, 25);
        let threats = [melee(25, 25, 3)];
        let v = view(c, &threats, &[], None);
        // Outside reach (range 4) vs inside (range 1): inside is worse.
        let outside = score_tile(&v, pos(29, 25), 8, &KiteFields::default());
        let inside = score_tile(&v, pos(26, 25), 8, &KiteFields::default());
        assert!(inside > outside, "inside reach must cost more: in={inside} out={outside}");
    }

    #[test]
    fn cohesion_penalizes_distance_and_steepens_past_k() {
        let c = pos(25, 25);
        let v = view(c, &[], &[], None);
        let d1 = score_tile(&v, pos(26, 25), 8, &KiteFields::default()); // d=1 (<=K)
        let d2 = score_tile(&v, pos(27, 25), 8, &KiteFields::default()); // d=2 (==K)
        let d3 = score_tile(&v, pos(28, 25), 8, &KiteFields::default()); // d=3 (>K, steepened)
        let d4 = score_tile(&v, pos(29, 25), 8, &KiteFields::default()); // d=4 (>K)
        assert!(d1 < d2 && d2 < d3 && d3 < d4, "monotonic in distance");
        // The marginal cost past K (d2→d3) exceeds the marginal cost within K (d1→d2).
        assert!((d3 - d2) > (d2 - d1), "cohesion penalty steepens past K");
    }

    #[test]
    fn value_keeps_the_focus_in_shooting_range() {
        let c = pos(25, 25);
        let focus = pos(25, 30);
        let v = view(c, &[], &[], Some(focus));
        // Both equidistant from the centroid (d=3); A keeps the focus in range 3, B does not.
        let in_range = score_tile(&v, pos(25, 28), 8, &KiteFields::default()); // range to focus = 2
        let out_range = score_tile(&v, pos(25, 22), 8, &KiteFields::default()); // range to focus = 8
        assert!(in_range < out_range, "keeping the focus shootable is preferred: in={in_range} out={out_range}");
    }

    #[test]
    fn openness_avoids_dead_ends() {
        let c = pos(25, 25);
        let v = view(c, &[], &[], None);
        let open = score_tile(&v, pos(26, 25), 8, &KiteFields::default()); // 8 walkable neighbours
        let pocket = score_tile(&v, pos(26, 25), 1, &KiteFields::default()); // 1 walkable neighbour (dead-end)
        assert!(pocket > open, "a dead-end pocket costs more");
    }

    #[test]
    fn safety_dominates_cohesion() {
        let c = pos(25, 25);
        let threats = [melee(25, 25, 3)];
        let v = view(c, &threats, &[], None);
        // A: safe (range 4 from the threat) but far from the centroid (d=5).
        let safe_far = score_tile(&v, pos(30, 25), 8, &KiteFields::default());
        // B: in the threat's reach (range 1) but right next to the centroid (d=1).
        let danger_close = score_tile(&v, pos(26, 25), 8, &KiteFields::default());
        assert!(safe_far < danger_close, "safety outweighs cohesion: safe_far={safe_far} danger_close={danger_close}");
    }

    // ── future-threat term (ADR 0019 Stage 2 reachability folding) ──
    #[test]
    fn future_term_prefers_tiles_a_chaser_reaches_later() {
        // Two tiles equidistant from the centroid and outside any present reach, but one is closer in
        // TICKS to a fast chaser than the other. The future term makes the soon-reached tile cost more
        // (durable standoff is preferred), with no present-threat/tower/focus terms to confound it.
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let chaser = pos(22, 25);
        let room: RoomName = "W1N1".parse().unwrap();
        let reach = LocalPathfinder.reachability_from(&[ReachSource { pos: chaser, step_ticks: 1 }], room, &mut cb, 2000);
        let c = pos(25, 25);
        let v = view(c, &[], &[], None);
        let f = KiteFields { threat_field: None, threat_reach: Some(&reach), cohesion_dist: None };
        // Two tiles EQUIDISTANT from the centroid (both d=1, so equal cohesion) but at different chaser
        // ranges: (24,25) is ttr 2 (< horizon → penalized), (26,25) is ttr 4 (>= horizon → 0).
        let near_in_time = score_tile(&v, pos(24, 25), 8, &f);
        let far_in_time = score_tile(&v, pos(26, 25), 8, &f);
        assert!(
            near_in_time > far_in_time,
            "the tile the chaser reaches sooner costs more: near={near_in_time} far={far_in_time}"
        );
        // With no reachability map the two are equal (future term omitted → byte-identical to pre-Stage-2).
        assert_eq!(score_tile(&v, pos(24, 25), 8, &KiteFields::default()), score_tile(&v, pos(26, 25), 8, &KiteFields::default()));
    }

    // ── wall-aware (g-cost) cohesion (ADR 0019 Stage 2-tail) ──
    #[test]
    fn cohesion_is_wall_aware_with_a_centroid_map() {
        let room: RoomName = "W1N1".parse().unwrap();
        // A wall column at x=26 with its only gap far away (y=40), so the tile just east of the
        // centroid is Chebyshev-close but reachable only by a long detour.
        let mut cb = |_r| {
            let mut cm = LocalCostMatrix::new();
            for y in 0..50u8 {
                if y != 40 {
                    cm.set(pos(26, y).xy(), u8::MAX);
                }
            }
            Some(cm)
        };
        let centroid = pos(25, 25);
        // The centroid flood gives the wall-aware path distance the SEARCH would thread in as `g`.
        let creach = LocalPathfinder.reachability_from(&[ReachSource { pos: centroid, step_ticks: 1 }], room, &mut cb, 3000);
        let v = view(centroid, &[], &[], None);
        let walled = pos(27, 25); // Chebyshev 2 from the centroid, but only reachable via the y=40 gap
        let wall_dist = creach.ticks_to_reach(walled).expect("reachable via the gap");
        let cheby = score_tile(&v, walled, 8, &KiteFields::default()); // Chebyshev cohesion (d=2)
        let aware = score_tile(&v, walled, 8, &KiteFields { threat_field: None, threat_reach: None, cohesion_dist: Some(wall_dist) });
        assert!(aware > cheby, "a tile behind a wall is correctly far for cohesion: aware={aware} cheby={cheby}");
        // In open terrain (west of the centroid, no wall) the g-cost equals Chebyshev → identical score.
        let open = pos(24, 25);
        let open_dist = creach.ticks_to_reach(open).expect("reachable");
        assert_eq!(
            score_tile(&v, open, 8, &KiteFields::default()),
            score_tile(&v, open, 8, &KiteFields { threat_field: None, threat_reach: None, cohesion_dist: Some(open_dist) }),
            "open-terrain g-cost equals Chebyshev"
        );
    }

    // ── unified objective: flee vs stand emerge from the weights (ADR 0019 Stage 3b) ──
    #[test]
    fn objective_weights_flip_flee_versus_stand() {
        // ONE scorer, two weight presets. Tile A is in weapon range of the focus but inside a melee
        // threat's reach; tile B is safe but out of the focus's range. The KITE preset prefers B
        // (flee — safety dominates); the ENGAGE preset prefers A (stand and fight — focus-damage
        // dominates). The flip is purely the objective weights.
        let centroid = pos(25, 25);
        let focus = pos(28, 25);
        let threats = [melee(27, 25, 3)]; // melee, reach 3, between the squad and the focus
        let a = pos(26, 25); // range 2 to focus (in range), range 1 to the threat (inside reach)
        let b = pos(22, 25); // range 6 to focus (out of range), range 5 to the threat (safe)

        let kite = SquadKiteView { centroid, threats: &threats, towers: &[], focus: Some(focus), focus_damage: None, params: KiteScoreParams::default(), fragile_hits: 0, squad_heal: 0, weapon_range: 3 };
        assert!(
            score_tile(&kite, b, 8, &KiteFields::default()) < score_tile(&kite, a, 8, &KiteFields::default()),
            "kite prefers the safe tile B (flee)"
        );

        let engage = SquadKiteView { centroid, threats: &threats, towers: &[], focus: Some(focus), focus_damage: None, params: KiteScoreParams::engage(), fragile_hits: 0, squad_heal: 0, weapon_range: 3 };
        assert!(
            score_tile(&engage, a, 8, &KiteFields::default()) < score_tile(&engage, b, 8, &KiteFields::default()),
            "engage prefers the in-range tile A (stand and fight)"
        );
    }

    // ── focus_damage actual-hits richness (ADR 0019 Stage 3b) ──
    fn engage_view<'a>(centroid: Position, focus: Position, fd: Option<FocusDamage>) -> SquadKiteView<'a> {
        SquadKiteView {
            centroid,
            threats: &[],
            towers: &[],
            focus: Some(focus),
            focus_damage: fd,
            params: KiteScoreParams::engage(),
            fragile_hits: 5000, // veto disabled at these damage levels
            squad_heal: 0,
            weapon_range: 3,
        }
    }

    #[test]
    fn focus_damage_pulls_a_melee_block_to_range_one() {
        // A melee+ranged squad lands MORE on the focus at range 1 (melee + ranged) than at range 3
        // (ranged only) → the DMG reward favors range 1 MORE than a pure-ranged squad does (whose
        // output is identical at either range). Isolated by comparing the r1-vs-r3 cost delta across
        // the two compositions (centroid == focus, so cohesion is common to both).
        let c = pos(25, 25);
        let r1 = pos(26, 25); // range 1 to focus
        let r3 = pos(28, 25); // range 3 to focus
        let mixed = engage_view(c, c, Some(FocusDamage { melee_power: 90, ranged_power: 30, focus_hits: 100_000, focus_heal: 0 }));
        let ranged = engage_view(c, c, Some(FocusDamage { melee_power: 0, ranged_power: 30, focus_hits: 100_000, focus_heal: 0 }));
        let mixed_pull = score_tile(&mixed, r3, 8, &KiteFields::default()) - score_tile(&mixed, r1, 8, &KiteFields::default());
        let ranged_pull = score_tile(&ranged, r3, 8, &KiteFields::default()) - score_tile(&ranged, r1, 8, &KiteFields::default());
        assert!(mixed_pull > ranged_pull, "melee composition pulls harder to range 1: mixed={mixed_pull} ranged={ranged_pull}");
    }

    #[test]
    fn focus_damage_zero_when_out_healed() {
        // The focus's heal/tick meets or exceeds our output → net 0 → the DMG reward vanishes → the
        // in-range tile is no more attractive than when we deal real damage (it costs MORE).
        let c = pos(25, 25);
        let tile = pos(27, 25); // range 2 to the focus (in r*)
        let focus = pos(25, 25);
        let dealing = engage_view(c, focus, Some(FocusDamage { melee_power: 0, ranged_power: 100, focus_hits: 100_000, focus_heal: 0 }));
        let out_healed = engage_view(c, focus, Some(FocusDamage { melee_power: 0, ranged_power: 100, focus_hits: 100_000, focus_heal: 100 }));
        assert!(
            score_tile(&out_healed, tile, 8, &KiteFields::default()) > score_tile(&dealing, tile, 8, &KiteFields::default()),
            "an out-healed target gives no engage reward"
        );
    }

    #[test]
    fn focus_damage_kill_priority_saturates_on_a_near_dead_focus() {
        // Same dealt damage, two focus HP: a near-dead focus (kill this tick) gets the saturated reward
        // → its in-range tile costs LESS than against a full-HP focus (commit to the kill).
        let c = pos(25, 25);
        let tile = pos(27, 25);
        let focus = pos(25, 25);
        let near_dead = engage_view(c, focus, Some(FocusDamage { melee_power: 0, ranged_power: 50, focus_hits: 40, focus_heal: 0 }));
        let full_hp = engage_view(c, focus, Some(FocusDamage { melee_power: 0, ranged_power: 50, focus_hits: 100_000, focus_heal: 0 }));
        assert!(
            score_tile(&near_dead, tile, 8, &KiteFields::default()) < score_tile(&full_hp, tile, 8, &KiteFields::default()),
            "a killable focus is worth committing to"
        );
    }

    // ── PositionLayers cache (ADR 0019 Stage 3a) — build once, reweight per use ──
    #[test]
    fn position_layers_built_once_reused_across_weightings() {
        // The operator's layer-cache point: the expensive floods are built ONCE, and different uses
        // (here: a kite weighting vs an attack-ish weighting that ignores future-threat) score against
        // the SAME cached layers — no rebuild, just different weights.
        let matrix = LocalCostMatrix::new();
        let centroid = pos(25, 25);
        let threats = [KiteThreat { pos: pos(22, 25), kind: ThreatKind::MeleeOnly, reach: 2, step_ticks: Some(1), attack_power: 30, ranged_power: 0 }];
        let layers = PositionLayers::build(&threats, &[], centroid.room_name(), &matrix, MAX_KITE_OPS);
        let fields = layers.fields();
        // The cache holds the shared threat-reachability layer (a mobile chaser is present).
        assert!(fields.threat_reach.is_some(), "the threat layer is built once and shared");

        let tile = pos(24, 25); // 2 tiles from the chaser → within FUTURE_HORIZON
        let kite = view(centroid, &threats, &[], None); // default weights (w_future = 5)
        let mut attack_params = KiteScoreParams::default();
        attack_params.w_future = 0.0; // a use that ignores durable-standoff
        let attack = SquadKiteView { centroid, threats: &threats, towers: &[], focus: None, focus_damage: None, params: attack_params, fragile_hits: 0, squad_heal: 0, weapon_range: 3 };

        let kite_score = score_tile(&kite, tile, 8, &fields);
        let attack_score = score_tile(&attack, tile, 8, &fields);
        assert!(
            kite_score > attack_score,
            "same cached layers, different weights → different scores: kite={kite_score} attack={attack_score}"
        );
    }

    // ── plan_kite_anchor (one bounded search per squad) ─────────────────
    #[test]
    fn plan_kite_anchor_flees_to_a_safe_tile_near_the_centroid() {
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let centroid = pos(25, 25);
        // A melee threat adjacent to the centroid → holding is unsafe; kite out of reach.
        let threats = [melee(24, 25, 3)];
        let v = view(centroid, &threats, &[], None);
        let plan = plan_kite_anchor(&v, None, &mut cb, MAX_KITE_OPS).expect("a safer tile than the centroid exists");
        assert!(plan.goal.get_range_to(pos(24, 25)) > 3, "escapes the melee reach: {:?}", plan.goal);
        assert!(plan.goal.get_range_to(centroid) <= 6, "stays near the squad (cohesion in the score): {:?}", plan.goal);
    }

    // ── ThreatField (ADR 0019 Stage 3b richer layer) + survival veto (#4) ──
    #[test]
    fn threat_field_stamps_creep_footprints_and_tower_falloff() {
        let melee_t = KiteThreat { pos: pos(10, 10), kind: ThreatKind::MeleeOnly, reach: 1, step_ticks: Some(1), attack_power: 30, ranged_power: 0 };
        let ranged_t = KiteThreat { pos: pos(30, 30), kind: ThreatKind::Ranged, reach: 3, step_ticks: Some(1), attack_power: 0, ranged_power: 70 };
        let tf = ThreatField::build(&[melee_t, ranged_t], &[]);
        assert_eq!(tf.raw_at(pos(11, 10)), 30, "melee threatens range 1");
        assert_eq!(tf.raw_at(pos(12, 10)), 0, "outside melee range 1");
        assert_eq!(tf.raw_at(pos(33, 30)), 70, "ranged threatens range 3");
        assert_eq!(tf.raw_at(pos(34, 30)), 0, "outside ranged range 3");

        let twf = ThreatField::build(&[], &[KiteTower { pos: pos(25, 25) }]);
        assert_eq!(twf.raw_at(pos(25, 25)), 600, "tower full damage at its own tile");
        assert_eq!(twf.raw_at(pos(25, 45)), 150, "tower min damage at range 20");
    }

    #[test]
    fn survival_veto_flees_a_lethal_ranged_zone() {
        // A fragile squad (200 hits) vs a strong ranged threat with reach 0 (so the normal safety term
        // does NOT push it away — ranged threats get reach 0). Only the survival veto (from the actual-
        // hits field: 100/tick × 3 = 300 > 200 → lethal within range 3) drives it outside the kill zone.
        let mut cb = |_r| Some(LocalCostMatrix::new());
        let centroid = pos(25, 25);
        let threat = KiteThreat { pos: pos(25, 25), kind: ThreatKind::Ranged, reach: 0, step_ticks: Some(1), attack_power: 0, ranged_power: 100 };
        let v = SquadKiteView { centroid, threats: &[threat], towers: &[], focus: None, focus_damage: None, params: KiteScoreParams::default(), fragile_hits: 200, squad_heal: 0, weapon_range: 3 };
        let plan = plan_kite_anchor(&v, None, &mut cb, MAX_KITE_OPS).expect("a non-lethal tile exists");
        assert!(plan.goal.get_range_to(pos(25, 25)) > 3, "fled outside the lethal ranged zone: {:?}", plan.goal);
    }

    #[test]
    fn plan_kite_anchor_holds_when_already_safe() {
        let mut cb = |_r| Some(LocalCostMatrix::new());
        // No threats/towers/focus → the centroid (cohesion 0) is the global min → hold (no goal).
        let v = view(pos(25, 25), &[], &[], None);
        assert!(plan_kite_anchor(&v, None, &mut cb, MAX_KITE_OPS).is_none(), "nothing to flee → hold");
    }
}
