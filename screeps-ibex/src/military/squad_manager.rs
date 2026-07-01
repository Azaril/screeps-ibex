//! `SquadManager` — the single combat squad lifecycle owner (ADR 0008 §3, P2.G2).
//!
//! A perpetual ECS system (like `ScoutOperation` / the visibility queue's systems)
//! that is the **one** layer owning squad state for objective-driven combat. Each
//! tick it reconciles the [`CombatObjectiveQueue`](super::objective_queue) against
//! the live squads:
//!
//! 1. **Reconcile** existing manager-owned squads (those whose `SquadContext`
//!    carries an `objective_id`): retire — delete the squad entity — when the
//!    objective has been withdrawn (the producer stopped re-asserting → TTL lapse,
//!    or it was explicitly withdrawn); otherwise re-establish the ephemeral claim
//!    (self-heals the claim map after a VM reset, where claims are not serialized).
//! 2. **Field rosters** — spawn any unfilled composition slot for a live squad,
//!    broadcasting one shared spawn token to the in-range home rooms (the proven
//!    `AttackMission` pattern). Members are `SquadCombatJob`s that **self-drive** to
//!    the target room and engage (status-log (ac)); the manager need not push
//!    per-tick movement (job-owns-movement, ADR 0008 §5 ⚑).
//! 3. **Claim new objectives** up to a global cap, minting a `SquadContext` bound to
//!    the objective.
//!
//! **Scope (P2.G2-minimal — "enough to field a `Farm{sk}` squad"):** *replacement*,
//! not pre-spawn (a dead member's slot unfills and is re-spawned; no `request_renew`
//! — the ADR's "never renew" already holds). Pre-spawn-before-death, per-tick
//! tactical orders (G3), retask-on-complete, and SquadId/`SquadStore` keying (P2.I1
//! — the squad is keyed by its `SquadContext` `Entity` until then) are follow-ons.
//! Retirement deletes the squad entity; orphaned members fall to the existing
//! `SquadCombatJob` fallback (no dangling `SquadContext` — no leak) until the general
//! `Recall` terminal state (P2.M0) lands.

use super::objective_queue::{CombatObjectiveQueue, EconomicIntel, ObjectiveId, ObjectiveKind, OBJECTIVE_PRIORITY_MEDIUM};
use screeps_combat_decision::composition::{SquadComposition, SquadSlot};
use screeps_combat_decision::lifecycle; // P-OBJ #23 / ADR 0027 — the pure reconcile kernel (shared, tested offline)
use super::squad::{AttackTarget, SquadContext, SquadState, SquadTarget, TickMovement, TickOrders};
use crate::combat::kite::{PositionLayers, ThreatField, MAX_KITE_OPS};
use crate::combat::{
    build_room_layers, build_room_threat_field, decide_squad_with_pathing, CombatCreepDto, CombatStructureDto,
    SquadDecision, SquadMemberView, SquadMovement, SquadOrderState, SquadView,
};
// ADR 0026 — the objective/information-dependent strategy-selection layer: pick the per-squad weight
// profile by objective class + room information, instead of the one fixed `SquadTacticParams::default()`.
use crate::combat::strategy::{decide_strategy, default_strategies, CombatObjectiveClass, StrategyContext, StrategyInfo};
use std::collections::HashMap;
use crate::creep::{spawning, CreepOwner};
use crate::entitymappingsystem::EntityMappingData;
use crate::jobs::squad_combat::{creep_to_dto, structure_to_dto};
use crate::room::data::RoomData;
use crate::room::visibilitysystem::{VisibilityQueue, VisibilityRequest, VisibilityRequestFlags, VISIBILITY_PRIORITY_HIGH};
use crate::serialize::SerializeMarker;
use crate::spawnsystem::*;
use screeps::*;
use screeps_rover::{CostMatrixCache, CostMatrixOptions, CostMatrixSystem};
use specs::prelude::*;
use specs::saveload::*;

/// Last-seen present-member count per live objective, so the manager can tell whether a FORMING squad
/// made spawn progress SINCE the previous reconcile (FIX 2 — the rally-stall fix). Ephemeral (NOT
/// serialized): a `BTreeMap` (deterministic iteration; never a result-affecting `HashMap`) reset to empty
/// on a VM reload. On reset a forming squad simply gets a fresh forming budget — still bounded, because
/// the per-objective entry only grows monotonically while the roster grows. Auto-created by specs as a
/// `Default` resource (like `CombatObjectiveQueue`), so no explicit registration is needed.
#[derive(Default)]
pub struct SquadFormingProgress {
    /// objective id → last-observed present-member count.
    last_present: std::collections::BTreeMap<ObjectiveId, usize>,
    /// objective id → the tick this generation STARTED forming (the deep-reach forming-budget clock, Break
    /// #1). Bounds how long the forming-in-flight lease refresh may extend a slow-but-fielding roster — past
    /// `MAX_FORMING_BUDGET` ticks the squad gives up even with a member in flight (no immortal squad).
    forming_started_at: std::collections::BTreeMap<ObjectiveId, u32>,
    /// objective id → the tick the full-roster squad DEPARTED home (the travel-budget clock, Break #2 travel
    /// half). Bounds the travel-phase lease refresh — past `MAX_TRAVEL_BUDGET` ticks the squad gives up.
    departed_at: std::collections::BTreeMap<ObjectiveId, u32>,
    /// objective id → last-observed room-distance from the squad centroid to the target room. The travel
    /// lease only refreshes while this is DECREASING (positional progress) — a stuck traveler gives up.
    last_target_dist: std::collections::BTreeMap<ObjectiveId, u32>,
    /// INTROSPECTION ONLY (zero behavior impact — never read by any gate/kernel). objective id → the phase
    /// label the squad was in at the previous trace, so the `[SquadTrace]` state-vector + transition-event
    /// lines fire on a PHASE CHANGE (and a throttled heartbeat) instead of every tick. A `BTreeMap`
    /// (deterministic; never a result-affecting `HashMap`); cleared on retire alongside the other trackers.
    last_phase: std::collections::BTreeMap<ObjectiveId, SquadPhase>,
    /// INTROSPECTION ONLY. objective id → whether the squad had ENGAGED at the previous trace, so the
    /// `ENGAGED` transition event fires exactly once on the false→true latch.
    last_engaged: std::collections::BTreeMap<ObjectiveId, bool>,
    /// FIX A (assault latch): objective ids whose squad has had `gather_quorum_met` fire at least once. Once
    /// latched, the TRAVEL phase takes the ASSAULT branch (advance the anchor rally→target) WITHOUT
    /// re-evaluating the gather quorum every tick — so members dying/lagging crossing enemy-held neighbours
    /// can't un-commit the assault (the contested in_room<->travel oscillation, BUG A). Ephemeral (a
    /// `BTreeSet`, NOT serialized — no `WORLD_FORMAT_VERSION` bump): on a VM reload the squad re-derives the
    /// quorum from live positions (a massed bloc re-latches immediately; a still-scattered one re-gathers).
    /// Cleared on retire alongside the other per-objective trackers.
    assault_latched: std::collections::BTreeSet<ObjectiveId>,
    /// ADR 0035 D4 (the LOST-IN-ROOM verdict carrier): objective ids whose squad's PREVIOUS-tick combat
    /// verdict over the REAL in-room view was a GENUINE LOSE — `engaged_once && in_room_any &&
    /// !present_force_wins_or_stalls(view, center)` — stamped by Phase B (`compute_squad_orders`, AFTER
    /// `apply_squad_decision` latches `engaged_once`). Phase A reads membership for `retreated_from_contact`
    /// (the abandon signal) WITHOUT rebuilding the SquadView — so abandon is carried from B's real-intel
    /// assessment, not recomputed in A. This is the EXACT inverse of `present_force_wins_or_stalls` (the lose
    /// SUBSET), NOT `ctx.state == Retreating` (a SUPERSET that also includes a critical/low-avg-HP retreat on
    /// a WINNABLE fight — the false-abandon this carrier fixes). Ephemeral (a `BTreeSet`, NOT serialized — no
    /// `WORLD_FORMAT_VERSION` bump): on a VM reload it re-derives next tick from the live in-room assessment.
    /// Cleared on retire alongside `assault_latched`. Membership only (insert/remove/contains — no iteration
    /// on a result-affecting path), so determinism is preserved.
    lost_in_room: std::collections::BTreeSet<ObjectiveId>,
    /// ADR 0034 D5 (RC-4/RC-8 — per-member travel progress): (objective, member-entity) → that member's
    /// last-observed room-distance to the shared rally. The travel lease refreshes while a MAJORITY of
    /// present members are CLOSING (vs the old single MIN-over-members signal that one stuck member could pin
    /// flat or one moving lead could mask). Ephemeral (`BTreeMap`, deterministic; NOT serialized — no WFV
    /// bump). Cleared on retire.
    member_rally_dist: std::collections::BTreeMap<(ObjectiveId, u32), u32>,
    /// ADR 0034 D8 (RC-3/RC-8 — the tighter per-member solo-travel STALL WINDOW): (objective, member-entity)
    /// → consecutive ticks this member has made NO solo-travel progress toward the rally (blocked / NO_PATH /
    /// stuck — its room-distance did not decrease). Past [`SOLO_TRAVEL_STALL_WINDOW`] the manager RE-ASSESSES
    /// the member OUT of the gather quorum (D4 escalation) so the squad proceeds with the reachable subset,
    /// well before the coarse `MAX_TRAVEL_BUDGET`. Ephemeral (NOT serialized — no WFV bump). Cleared on retire.
    member_solo_stall: std::collections::BTreeMap<(ObjectiveId, u32), u32>,
    /// ADR 0034 D5 (RC-4/RC-8 — per-member TARGET progress for the travel lease): (objective, member-entity)
    /// → that member's last-observed room-distance to the TARGET room. The travel lease refreshes while a
    /// MAJORITY of present members are CLOSING on the target — so one stuck member can't pin the lease
    /// "stalled" (the old single MIN signal) and one moving lead can't mask a stuck bulk. Ephemeral (NOT
    /// serialized — no WFV bump). Cleared on retire.
    member_target_dist: std::collections::BTreeMap<(ObjectiveId, u32), u32>,
}

/// ADR 0034 D8 (RC-8): the TIGHTER per-member solo-travel stall window — consecutive ticks a member makes no
/// progress toward the shared rally (blocked / NO_PATH) after which the manager RE-ASSESSES it OUT of the
/// gather quorum (D4) and proceeds with the reachable subset. In the 50–150 band per the ADR so a
/// wrong/blocked rally is caught FAST, well before the coarse `MAX_TRAVEL_BUDGET` (1000) backstop. Ephemeral
/// runtime state (a per-member tracker like `assault_latched`) — NOT serialized, no `WORLD_FORMAT_VERSION` bump.
pub const SOLO_TRAVEL_STALL_WINDOW: u32 = 100;

/// INTROSPECTION ONLY (ADR 0027 squad-lifecycle observability) — a coarse phase label for the
/// `[SquadTrace]` logs so the full FIELD → forming → rally → deploy → travel → in_room → engaged journey
/// is visible on a live soak. Derived purely from already-computed snapshot facts; NEVER feeds a gate,
/// kernel, or control-flow decision. Ordered/`PartialEq` only for the phase-change detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SquadPhase {
    /// Roster incomplete — still spawning/banking members at home.
    Forming,
    /// Full (or quorum) roster, but the rally gate has not released — holding at home to group up.
    Rally,
    /// Rally released, full roster present, not yet in the target room — crossing toward it.
    Travel,
    /// At least one member is standing in the target room but the squad has not engaged.
    InRoom,
    /// The squad has reached `Engaged` (focus acquired + combat) at least once.
    Engaged,
}

impl SquadPhase {
    fn label(self) -> &'static str {
        match self {
            SquadPhase::Forming => "forming",
            SquadPhase::Rally => "rally",
            SquadPhase::Travel => "travel",
            SquadPhase::InRoom => "in_room",
            SquadPhase::Engaged => "engaged",
        }
    }
}

/// INTROSPECTION heartbeat throttle: while a squad sits in a steady phase, re-emit its state vector every
/// this-many ticks so a long-lived stuck squad keeps producing one greppable status line without flooding.
const SQUAD_TRACE_HEARTBEAT: u32 = 25;

/// Global cap on concurrently-fielded manager squads. Objectives above this
/// compete by priority via `best_unclaimed_near`. (Per-objective-kind caps —
/// e.g. SK `max_concurrent_farms` — are enforced by the producers.)
const MAX_CONCURRENT_SQUADS: usize = 4;

/// Cap on squads still FORMING (incomplete roster) at once. A forming squad's slots spawn at HIGH (above
/// the economy bulk — see `spawn_priority_for`), so letting many form together starves logistics AND
/// splits the scarce high-priority spawn-ticks so none completes (observed: two squads co-stalled at 3/5
/// and 1/2 for thousands of ticks). Serializing finishes one or two rosters before the next is claimed.
/// Complete squads (out fighting) do NOT count toward this, so it never reduces total concurrent offense
/// below `MAX_CONCURRENT_SQUADS` — it only paces how fast new rosters are started.
const MAX_FORMING_SQUADS: usize = 2;

/// While a squad is still FORMING (incomplete roster), renew a present member whose remaining TTL drops
/// below this so a slow/contested form does not bleed out its early members to old age before the roster
/// completes (ADR 0028 — the live no-renew member-death; `request_renew` previously had zero callers). The
/// spawn system's renew pass only uses spawns no pending spawn claimed + is gated on room energy, so this
/// never starves spawning or a poor colony.
const RENEW_WHILE_FORMING_TTL: u32 = 300;

/// Max room distance from a candidate home to the objective room for that home to
/// be a spawn source (keeps a squad from being spawned across the map). Matches
/// the legacy `MAX_DEFENSE_SOURCE_DISTANCE` (10) so the defense migration does not
/// narrow the set of rooms a defender can be sourced from.
const MAX_SPAWN_DISTANCE: u32 = 10;

/// P-OBJ #23 commitment lease (ticks). When the manager fields a squad it stamps the objective's
/// `deadline = now + COMMITMENT_BUDGET` and refreshes it every tick the squad still has a focus (is
/// actively closing on / fighting a target). The objective then survives producer silence on stale intel
/// for this whole window — generous cover for form (~120) + travel (~150) + a clear margin (~130) — so a
/// committed squad is never retired underneath before it can arrive and engage. If the lease lapses with
/// no active focus (stuck en route, or fought-and-withdrew without a clean clear) the manager gives up
/// and backs the room off; a clean clear resolves earlier via `engaged_once && no-focus && in-room`.
const COMMITMENT_BUDGET: u32 = 400;

/// Deep-reach fix (Break #1) — absolute bound on how long the forming-in-flight lease refresh may extend a
/// squad's life. A roster that has not completed within this many ticks of its generation starting gives up
/// even with a member nominally in flight (banking), so a genuinely-unfieldable squad is never immortal.
/// Generous: covers a trickle-income RCL6/7 colony banking several capped members serially (the inter-member
/// banking gap can exceed COMMITMENT_BUDGET, which is exactly why the per-present++ refresh was insufficient).
const MAX_FORMING_BUDGET: u32 = 3000;

/// Deep-reach fix (Break #2 travel half) — absolute bound on the travel-phase lease refresh. A full-roster
/// squad that has not arrived within this many ticks of departing home gives up. Covers the longest realistic
/// multi-room hop (MAX_SPAWN_DISTANCE=10 rooms ≈ 500 tiles) with margin.
const MAX_TRAVEL_BUDGET: u32 = 1000;

/// Chebyshev distance between two rooms.
fn room_distance(a: RoomName, b: RoomName) -> u32 {
    let delta = a - b;
    delta.0.unsigned_abs().max(delta.1.unsigned_abs())
}

/// RC-11 — the squad's gather→ASSAULT vs SOLO-TRAVEL branch as a PURE, testable predicate (the exact
/// composition the live `compute_squad_orders` uses for `gathered`). `true` ⇒ ASSAULT (advance the box-
/// formation anchor); `false` ⇒ SOLO-TRAVEL (each member paths individually to the shared rally and the
/// squad MASSES before any formation assault).
///
/// The win-or-stall fast-path (`present_wins_or_stalls`) only short-circuits the count quorum when the
/// squad has REAL target intel (`have_target_intel`) — `winnable_fast_path_allowed`. Without intel (an
/// UNSCOUTED room: empty DTOs, not LiveVisible) a VACUOUS win cannot latch the assault on a SCATTERED
/// squad; the squad falls to the count quorum (`gather_quorum_met`) which a scattered roster does NOT meet
/// → solo-travel. A previously-fired latch (`assault_latched`) keeps the assault committed. This is the one
/// place the freeze-vs-reach distinction is decided, factored out so the conditional fix is unit-tested
/// without the live world plumbing.
fn squad_is_gathered(present_wins_or_stalls: bool, have_target_intel: bool, gather_quorum_met: bool, assault_latched: bool) -> bool {
    let quorum_now =
        screeps_combat_decision::winnable_fast_path_allowed(present_wins_or_stalls, have_target_intel) || gather_quorum_met;
    quorum_now || assault_latched
}

/// Map an objective's selection priority to a spawn-queue priority so a FORMING combat squad is not
/// starved below economy. The spawnsystem head-of-line break (`spawnsystem.rs`: a request with
/// `body_cost > available_energy` but `<= energy_capacity` → `break`) reserves each idle home's energy for
/// the highest-priority pending request and spawns nothing below it that tick. MEDIUM offense slots
/// previously mapped to `SPAWN_PRIORITY_HIGH` (75) — TIED with the economy bulk (haulers / upgraders /
/// claim / secondary-mining all 75) and sorted LAST in-tier (`RunMissionSystem` enqueues economy before
/// `SquadManagerSystem` enqueues squads), so they still sat permanently last behind the colony's constant
/// economy demand and rosters never completed (observed dead-stuck at 3/5, 1/2 for thousands of ticks
/// despite idle in-range spawns). FIX 2: MEDIUM+ objectives (active offense/defense) now map to the
/// dedicated `SPAWN_PRIORITY_COMBAT_FORMING` band (85) — STRICTLY above the HIGH economy bulk so forming
/// slots win the within-tier ordering AND the energy-banking race, but STRICTLY below the CRITICAL miners
/// (100) so energy INCOME is never preempted. Only LOW farms stay at MEDIUM. BOUNDED: the
/// `MAX_FORMING_SQUADS` (=2) cap limits how many squads' slots sit in this band at once, and
/// `economy::can_afford_military` already declined unaffordable squads, so it cannot crater the economy.
/// (Defense objectives upsert at `OBJECTIVE_PRIORITY_HIGH`; invader-core offense at `..._MEDIUM`; farms at
/// `..._LOW`.)
fn spawn_priority_for(objective_priority: f32) -> f32 {
    if objective_priority >= OBJECTIVE_PRIORITY_MEDIUM {
        SPAWN_PRIORITY_COMBAT_FORMING
    } else {
        SPAWN_PRIORITY_MEDIUM
    }
}

/// A squad is *wiped* (overwhelmed — all members lost) when it had spawned members but none remain
/// alive. Gradual losses are refilled by the unfilled-slot spawns (Phase B) and never reach
/// all-empty; only a squad that lost everyone does. Pure so it's host-testable without an ECS world.
fn squad_is_wiped(total_members_added: u32, living_members: usize) -> bool {
    total_members_added > 0 && living_members == 0
}

/// FIX 2 (rally-stall): classify whether a squad is still FORMING its roster and whether it made spawn
/// PROGRESS since the previous reconcile. Pure so it's host-testable without an ECS world.
///
/// `forming` = the squad has members, has NOT engaged yet, and has fewer present members than the
/// requested roster (still assembling). `forming_progress` = the present count grew since the last
/// reconcile — true only on the exact tick a new member appears. The kernel refreshes the lease while
/// `forming && forming_progress`, which is BOUNDED: the present count can only increase up to
/// `requested_slots`, so a squad that stops gaining members (can't bank energy for the next slot) stops
/// being refreshed and gives up. `requested_slots == 0` (unknown) ⇒ never forming (preserve legacy).
fn forming_state(
    has_members: bool,
    engaged_once: bool,
    present_count: usize,
    requested_slots: usize,
    prev_present: usize,
) -> (bool, bool) {
    let forming = has_members && !engaged_once && requested_slots > 0 && present_count < requested_slots;
    let forming_progress = forming && present_count > prev_present;
    (forming, forming_progress)
}

/// FIGHTER-FIRST spawn ordering (deep-reach fix — Break #1): the slot indices of `slots` reordered so the
/// FIGHTER roles (RangedDPS / Dismantler / MeleeDPS) come BEFORE the support roles (Healer / Tank / Hauler).
/// A stable sort within each group preserves the original slot order, so the reorder is deterministic and
/// the per-slot `slot_index` (the composition position the spawn callback + slot-filled tracking key on) is
/// PRESERVED — only the queue-attempt order changes. Pure so it's host-testable without an ECS world.
fn spawn_order_fighter_first(slots: &[SquadSlot]) -> Vec<usize> {
    use screeps_combat_decision::composition::SquadRole;
    let is_fighter = |r: SquadRole| matches!(r, SquadRole::RangedDPS | SquadRole::Dismantler | SquadRole::MeleeDPS);
    let mut order: Vec<usize> = (0..slots.len()).collect();
    // Stable sort by a fighter-first key (false < true ⇒ negate): fighters get key 0, support key 1.
    order.sort_by_key(|&i| u8::from(!is_fighter(slots[i].role)));
    order
}

/// Whether an objective's squad fights as an oriented **formation box** (siege: keep the anchor
/// when engaged, advance to the focus, present armor toward the threat) vs **skirmishes** (kite via
/// `decide_movement`). Today only `Dismantle` (structure siege) is a formation; defense / farm /
/// harass kite. (Offense `Secure`'s style is decided when its producer lands — P2.G4-O6.)
fn is_formation_objective(kind: &ObjectiveKind) -> bool {
    matches!(kind, ObjectiveKind::Dismantle { .. })
}

/// ADR 0026 — classify a squad's objective for the strategy-selection layer. `StructureBreach` = an
/// explicit dismantle objective (`formation`), OR a room whose only remaining hostiles are STRUCTURES
/// (creeps cleared ⇒ switch to breaching the ring); everything else is open-creep combat. Recomputed each
/// tick, so a squad self-corrects as the room state changes (clears the creeps → flips to breach).
fn classify_objective(formation: bool, has_structures: bool, has_live_hostiles: bool) -> CombatObjectiveClass {
    if formation || (has_structures && !has_live_hostiles) {
        CombatObjectiveClass::StructureBreach
    } else {
        CombatObjectiveClass::OpenCombat
    }
}

/// ADR 0027 v1 capability class — the BROAD class a squad/objective belongs to, for the reassignment
/// capability gate (v1: same broad class only; full ADR-0031 capability match later). A defender
/// (`Defend`/`Secure` — the threat-centric defense arm) may reassign to another defense objective; an
/// offense objective (`Harass`/`Dismantle`/`Farm`/`Escort`) only to another offense objective. This stops a
/// freed defender being rebound onto an uncrackable core (the `IN_ROOM_NO_FOCUS` stall the ADR's cohesion
/// risks call out, line 277). Pure + deterministic (a `match`, no `HashMap`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapabilityClass {
    Defense,
    Offense,
    /// ADR 0027 v1.1 P2: a DECLAIM squad (a CLAIM declaimer). A DEDICATED class so a freed declaimer is
    /// NEVER reassigned onto a combat objective (a CLAIM creep can't crack a core / clear creeps) and no
    /// combat squad is ever reassigned onto a Declaim (a RANGED squad can't `attackController` — wrong body).
    Declaim,
}

fn capability_class(kind: &ObjectiveKind) -> CapabilityClass {
    match kind {
        // The threat-centric defense arm (ADR 0027 Option B): `Secure` is how defense is now emitted (at the
        // threat's room), alongside the optional preemptive `Defend` hold.
        ObjectiveKind::Defend { .. } | ObjectiveKind::Secure { .. } => CapabilityClass::Defense,
        ObjectiveKind::Harass { .. } | ObjectiveKind::Dismantle { .. } | ObjectiveKind::Farm { .. } | ObjectiveKind::Escort { .. } => {
            CapabilityClass::Offense
        }
        ObjectiveKind::Declaim { .. } => CapabilityClass::Declaim,
    }
}

// ═══ ADR 0032 v1.1 — EV-of-pairing projection (bot intel → the pure `objective_value`/`pairing_ev` kernels) ══
//
// The auction's per-squad SELECTION ranks candidate objectives by `EV = P(win | squad caps vs defense) ·
// value_e − travel cost` (ADR 0032 §"EV of a (squad, objective) pairing"), reusing the EXISTING squad's
// `capabilities()` (not a candidate search). These helpers PROJECT the bot's per-room intel into the pure
// decision-crate kernels (`objective_value::value_e` + `composition::pairing_ev`), exactly as `war.rs`
// projects intel into `optimize_composition` — so the bot and the kernels agree, no inline EV math here.

use screeps_combat_decision::assignment::{
    build_ev_matrix_with_merge, role_bit, solve_assignment, CapClass, ColumnKind, MatrixParams, ObjectiveCell, SquadRow,
};
use screeps_combat_decision::composition::{pairing_ev, quantize_ev, PairingParams, SquadCapabilities};
use screeps_combat_decision::doctrine::EnemyForce;
use screeps_combat_decision::force_sizing::{DefenseProfile, TowerThreat};
use screeps_combat_decision::objective_value::{value_e, ObjectiveIntel, ObjectiveValueKind};

/// Map the bot's `CapabilityClass` → the decision crate's bot-enum-free [`CapClass`] (ADR 0032 v1.2 —
/// the global Hungarian's capability pre-filter). A 1:1 projection, like `project_value_kind`.
fn cap_class(class: CapabilityClass) -> CapClass {
    match class {
        CapabilityClass::Defense => CapClass::Defense,
        CapabilityClass::Offense => CapClass::Offense,
        CapabilityClass::Declaim => CapClass::Declaim,
    }
}

/// The commit-EV threshold reused from ADR 0031 (`CompositionParams::commit_ev_threshold`) as the
/// per-squad reassign/claim gate floor (ADR 0032 §EV-positive gate): a move must beat its alternative by
/// MORE than this (quantized) to fire, preventing thrash on near-ties. Conservative (small) so a clearly
/// better move always fires but a marginal one does not.
const COMMIT_EV_THRESHOLD: f32 = 1.0;

/// A priority-implied DANGER floor (DPS) for a DEFENSE objective so `value_e` is never starved by missing
/// intel (ADR 0032 §"must not starve real defense"): a producer-set band → a minimum threat danger. Scaled
/// so even a MEDIUM defense objective clears the EV-positive floor (the producer only emits when a defender
/// is warranted), while the scouted DPS (which can exceed this) still ranks objectives against each other.
fn priority_implied_danger(priority: f32) -> f32 {
    use super::objective_queue::{OBJECTIVE_PRIORITY_CRITICAL, OBJECTIVE_PRIORITY_HIGH, OBJECTIVE_PRIORITY_MEDIUM};
    if priority >= OBJECTIVE_PRIORITY_CRITICAL {
        300.0 // base under direct attack — a substantial assault floor
    } else if priority >= OBJECTIVE_PRIORITY_HIGH {
        120.0 // adjacent / operator intent
    } else if priority >= OBJECTIVE_PRIORITY_MEDIUM {
        60.0 // leashed roamer / remote invader
    } else {
        30.0 // a real-but-minor threat (one armed creep)
    }
}

/// Project a bot `ObjectiveKind` → the pure `value_e` kind (parity with the `DoctrineObjective` projection).
///
/// Reach-bug #3 (ADR 0032 §economic-value-unlocked): when the producer attached COMPUTED economic intel
/// (the room's controlled net-ROI), value the objective as a `FarmCore` (income·horizon) REGARDLESS of its
/// `ObjectiveKind` — a winnable lvl0 invader core maps to `Dismantle`/`Denial` (≈0 with dps 0), but the
/// room it UNLOCKS is worth its remote's income, so the economic arm of `value_e` should price it.
fn project_value_kind(kind: &ObjectiveKind, economic: Option<EconomicIntel>) -> ObjectiveValueKind {
    use super::objective_queue::FarmKind;
    if economic.is_some() {
        // The economic-value-unlocked override: price by the controlled-room net-ROI (the FarmCore arm).
        return ObjectiveValueKind::FarmCore;
    }
    match kind {
        ObjectiveKind::Defend { .. } | ObjectiveKind::Secure { .. } | ObjectiveKind::Escort { .. } => ObjectiveValueKind::Defend,
        ObjectiveKind::Farm { kind: FarmKind::Core, .. } => ObjectiveValueKind::FarmCore,
        ObjectiveKind::Farm { kind: FarmKind::SourceKeeper, .. } => ObjectiveValueKind::FarmSourceKeeper,
        ObjectiveKind::Farm { kind: FarmKind::PowerBank, .. } => ObjectiveValueKind::FarmPowerBank,
        // ADR 0027 v1.1 P2: a declaim DENIES the enemy a controller (and acquires a mining room) — value as
        // a denial objective so the EV-positive claim gate treats it like the other resource-denial work.
        ObjectiveKind::Harass { .. } | ObjectiveKind::Dismantle { .. } | ObjectiveKind::Declaim { .. } => ObjectiveValueKind::Denial,
    }
}

/// Build the per-objective `DefenseProfile` the EV P(win) is judged against, from the room's scouted threat
/// intel. The assault tile is the room center (the coarse proxy war.rs uses for non-flag targets); unknown
/// per-tower energy ⇒ assume firing (1000), never under-estimating. `None` intel ⇒ undefended profile.
fn project_defense(threat: Option<&crate::military::threatmap::RoomThreatData>) -> DefenseProfile {
    let Some(td) = threat else {
        return DefenseProfile::default();
    };
    let towers: Vec<TowerThreat> = td
        .hostile_tower_positions
        .iter()
        .enumerate()
        .map(|(i, _)| TowerThreat { range_to_assault: 25, energy: td.tower_energy.get(i).copied().unwrap_or(1000) })
        .collect();
    DefenseProfile {
        towers,
        breach_hits: td.breach_rampart_hits,
        objective_hits: 0,
        // ADR 0031 #41: the hostile-creep dps is carried on the single [`EnemyForce`] channel that
        // `pairing_ev`/`pairing_p_win` read via the separate `enemy` argument (built by `project_enemy` from
        // the same threat intel). `DefenseProfile` is STRUCTURE-only now (no `enemy_dps` field), so there is no
        // dead channel here to keep at 0 — the footgun is gone.
        repair_per_tick: 0.0,
        safe_mode: td.safe_mode_active,
        // ADR 0035 D1: derive the tri-state tower intel from the existing threat fields (threat data present
        // here ⇒ empty list is ScoutedEmpty, non-empty is Seen). Keeps the manager's runtime profile
        // classification consistent with war.rs's commit-side derivation. No new serialized state.
        tower_intel: screeps_combat_decision::force_sizing::tower_intel_from(td.hostile_tower_positions.is_empty(), true),
    }
}

/// Build the hostile CREEP `EnemyForce` the EV P(win) is judged against, from the room's scouted threat —
/// the `enemy` arg `pairing_p_win` actually reads (parity with war.rs's owned-defense path, war.rs ~486-492).
/// `dps`/`heal` are the threat totals; `hits = 0` (this prices the attrition the squad takes, NOT a structure
/// objective to kill — the structure/breach cost is on `DefenseProfile`); `count`/`boosted` come from the
/// per-creep intel. `None` intel ⇒ no enemy (`None`), the genuinely-undefended case.
fn project_enemy(threat: Option<&crate::military::threatmap::RoomThreatData>) -> Option<EnemyForce> {
    let td = threat?;
    Some(EnemyForce {
        dps: td.estimated_attack_dps,
        heal: td.estimated_heal,
        hits: 0,
        count: td.hostile_creeps.len() as u32,
        boosted: td.hostile_creeps.iter().any(|c| c.boosted),
    })
}

/// Build the `ObjectiveIntel` the `value_e` reads. For a DEFENSE objective the value scales with the THREAT
/// DANGER (the dps=0 over-response fix, ADR 0032 line 46): asset_value = the room's energy capacity (the
/// RCL/asset proxy war.rs uses), threat_danger = the scouted estimated DPS. Farm/denial kinds derive their
/// fields from the priority as a coarse income/denial proxy (v1.1 — the precise farm income is the war/SK
/// producer's; the per-squad gate only needs a comparable ordering).
fn project_intel(
    kind: &ObjectiveKind,
    priority: f32,
    asset_value: f32,
    threat: Option<&crate::military::threatmap::RoomThreatData>,
    economic: Option<EconomicIntel>,
) -> ObjectiveIntel {
    // Reach-bug #3 (ADR 0032 §economic-value-unlocked): if the producer attached the room's COMPUTED
    // controlled net-ROI, feed it straight into the FarmCore economic arm (`income_per_tick · horizon`) —
    // the real economy unlocked, not the priority proxy. `project_value_kind` returns FarmCore to match.
    if let Some(econ) = economic {
        return ObjectiveIntel {
            income_per_tick: econ.net_income_per_tick.max(0.0),
            horizon: econ.horizon.max(0.0),
            ..Default::default()
        };
    }
    let danger = threat.map(|t| t.estimated_attack_dps).unwrap_or(0.0);
    match project_value_kind(kind, None) {
        // DEFENSE: scale value by the THREAT DANGER (the dps=0 over-response fix — a HIGHER-dps threat is
        // worth more to defend), but FLOOR the danger by a priority-implied minimum so a defense objective is
        // NEVER starved by missing/stale intel: the producer (war.rs) only emits a Defend/Secure when a
        // threat ALREADY warrants a defender (`hostile_warrants_defender` — incl. dps=0 controller-attackers),
        // so its mere existence is a real threat. The floor keeps a genuinely-dangerous threat (high
        // priority) fielding a defender even before its DPS is scouted; the scouted DPS still differentiates
        // RANKING among defense objectives. (The pure "harmless scout → 0 value" case is gated upstream at the
        // observe layer; here a fielded defense objective always clears the EV-positive floor.)
        ObjectiveValueKind::Defend => {
            ObjectiveIntel { asset_value, threat_danger: danger.max(priority_implied_danger(priority)), ..Default::default() }
        }
        // Farm/denial: the producer-set priority is a comparable upside proxy (v1.1). Scaled so it lands in a
        // similar magnitude to a defended value_e (priority ∈ ~[0,100] → a denial-magnitude upside).
        ObjectiveValueKind::FarmCore | ObjectiveValueKind::FarmSourceKeeper => {
            ObjectiveIntel { income_per_tick: priority.max(0.0), horizon: 100.0, ..Default::default() }
        }
        ObjectiveValueKind::FarmPowerBank => ObjectiveIntel { roi: priority.max(0.0) * 100.0, ..Default::default() },
        ObjectiveValueKind::Denial => ObjectiveIntel { denial_value: priority.max(0.0) * 100.0, ..Default::default() },
    }
}

/// THE per-squad EV of pairing `caps` with an objective (ADR 0032 v1.1), quantized for a stable discrete
/// branch (ADR 0020 §6): `EV = P(win | caps vs defense) · value_e − w_travel · travel`. `caps` is the
/// EXISTING squad's surviving capability; `value_e`/`defense`/`intel` are projected from the objective's
/// kind + the room's scouted intel; `travel` is the Chebyshev distance home→room. Pure inputs → the pure
/// kernels → a deterministic integer.
#[allow(clippy::too_many_arguments)]
fn objective_ev_q(
    caps: SquadCapabilities,
    kind: &ObjectiveKind,
    priority: f32,
    asset_value: f32,
    threat: Option<&crate::military::threatmap::RoomThreatData>,
    economic: Option<EconomicIntel>,
    onsite_window: u32,
    travel_rooms: u32,
) -> i64 {
    let intel = project_intel(kind, priority, asset_value, threat, economic);
    let val = value_e(project_value_kind(kind, economic), &intel);
    let defense = project_defense(threat);
    // Price the hostile CREEP force the P(win) is judged against (the EV-wiring fix): `pairing_p_win` reads
    // the enemy via this single `EnemyForce` arg (ADR 0031 #41 — the one enemy-creep channel). Passing `None`
    // let a room defended ONLY by hostile creeps (no energized towers, objective_hits=0) read as `undefended`
    // → P(win)=1.0 against a room full of attackers, inflating EV for creep-defended Harass/Dismantle/Farm/Defend.
    // Derive the force from the room's scouted threat exactly as war.rs's owned-defense path does (war.rs
    // ~486-492): dps/heal from the threat totals, hits=0 (creeps, not a structure objective), count/boosted
    // from the per-creep intel.
    let enemy = project_enemy(threat);
    let ev = pairing_ev(caps, &defense, enemy, val, onsite_window, travel_rooms, &PairingParams::default());
    quantize_ev(ev)
}

/// ADR 0032 v1.2 — the GLOBAL EV-maximizing REASSIGN matching (the Hungarian kernel, run ONCE per scan).
/// Builds the `N×K` EV matrix over the managed squads (ROWS, in the caller's STABLE id order) × all live
/// objectives (COLUMNS) + the per-row StayPut/Recycle columns, solves it deterministically
/// ([`solve_assignment`]), and returns a `squad entity → globally-optimal NEW objective` map. A squad whose
/// optimum is StayPut/Recycle (keep its current fight / no net-positive move) is ABSENT from the map.
///
/// This REPLACES the v1.1 per-squad greedy `best_by_ev` reassign loop: the per-squad reconcile below
/// consults this single global solution instead of each squad greedily grabbing its own best. The cell EV,
/// `value_e`, defense/enemy projection, and the EV-positive gate (the StayPut/Recycle columns) reuse the
/// SAME helpers v1.1 used (`project_*`/`pairing_ev`/`value_e`) — only the SELECTION changed from greedy to
/// global. Pure read of `data` (no mutation); deterministic (Vec-ordered, integer EV, no `HashMap` in the
/// kernel — the returned map is built after the deterministic solve).
/// ADR 0032 v2 / ADR 0027 — the result of a chosen `Merge→Bk` column: the DONOR squad `donor` sheds its
/// role-matched present member(s) into the RECEIVER squad `receiver`'s open pending slot(s). The apply
/// layer performs the transfer (rebind creep squad-ref + slot to B's pending slot, drop the now-filled
/// spawn slot, donor empties → clean retire). `roles` = the donor's sheddable role bitmask (so the apply
/// matches each transferred creep to a compatible OPEN slot of B deterministically).
#[derive(Clone, Copy, Debug)]
struct MergeDecision {
    donor: Entity,
    receiver: Entity,
    roles: u8,
}

/// Compute the SHEDDABLE capability + role bitmask of the donor's FILLED slots (ADR 0027 — the member(s) it
/// transfers): the sub-composition of `comp.slots` whose `slot_index` is in `filled`. Deterministic (Vec
/// order; no HashMap). Returns `(caps, role_bitmask)`.
fn sheddable_of(comp: &SquadComposition, filled: &[usize], squad_energy: u32) -> (SquadCapabilities, u8) {
    let mut roles = 0u8;
    let mut sub = SquadComposition {
        label: String::new(),
        slots: Vec::new(),
        formation_shape: comp.formation_shape,
        formation_mode: comp.formation_mode,
        retreat_threshold: comp.retreat_threshold,
    };
    for (i, slot) in comp.slots.iter().enumerate() {
        if filled.contains(&i) {
            roles |= role_bit(slot.role);
            sub.slots.push(SquadSlot { role: slot.role, body_type: slot.body_type });
        }
    }
    (sub.capabilities(squad_energy), roles)
}

/// Compute the OPEN (unfilled) pending-slot role bitmask of a forming receiver (ADR 0027 line 258 — the
/// PENDING SPAWN SLOT a donor's creep may fill): the OR of `role_bit` over `comp.slots` whose `slot_index`
/// is NOT in `filled`. Deterministic. Zero ⇒ no open slot ⇒ not a merge receiver.
fn open_slot_roles_of(comp: &SquadComposition, filled: &[usize]) -> u8 {
    let mut roles = 0u8;
    for (i, slot) in comp.slots.iter().enumerate() {
        if !filled.contains(&i) {
            roles |= role_bit(slot.role);
        }
    }
    roles
}

fn solve_global_reassignment(
    data: &SquadManagerSystemData,
    managed: &[(Entity, ObjectiveId)],
    homes: &[HomeRoom],
    now: u32,
) -> (std::collections::HashMap<Entity, ObjectiveId>, Vec<MergeDecision>) {
    let mut out = std::collections::HashMap::new();
    if managed.is_empty() || homes.is_empty() {
        return (out, Vec::new());
    }
    let anchor = homes.first().map(|h| h.name);
    let squad_energy = homes.first().map(|h| h.energy_capacity).unwrap_or(0);
    let threat_for = |room: RoomName| -> Option<&crate::military::threatmap::RoomThreatData> {
        data.mapping.get_room(&room).and_then(|e| data.threat_data.get(e))
    };
    let asset_of = |room: RoomName| -> f32 {
        data.mapping
            .get_room(&room)
            .and_then(|e| data.room_data.get(e))
            .and_then(|rd| game::rooms().get(rd.name))
            .map(|r| r.energy_capacity_available() as f32)
            .unwrap_or(1.0)
    };

    // ── ROWS: the managed squads, in the caller's stable order. Each row carries its surviving caps (off the
    //    claimed objective's composition at the anchor energy — what we fielded), its class, and its current
    //    objective id (so the StayPut column re-scores the right fight). recycle_ev = 0 (the reassign path
    //    reuses bodies; recycling here is the net-negative floor, not a refund model — v1.1 parity). ──
    let mut rows: Vec<SquadRow> = Vec::with_capacity(managed.len());
    for (entity, obj_id) in managed {
        let obj = data.objective_queue.get(*obj_id);
        let class = obj.map(|o| cap_class(capability_class(&o.kind))).unwrap_or(CapClass::Offense);
        let comp = obj.and_then(|o| o.force.squads.first());
        let caps: SquadCapabilities = comp.map(|c| c.capabilities(squad_energy)).unwrap_or_default();

        // ── ADR 0032 v2 / ADR 0027 MERGE fields. Read this squad's live members → filled slot indices +
        //    present count + whether it has committed to a fight (`engaged_once`). The donor SHEDS its filled
        //    slots; the receiver OFFERS its unfilled (open pending) slots. ──
        let ctx = data.squad_contexts.get(*entity);
        let filled: Vec<usize> = ctx.map(|c| c.members.iter().map(|m| m.slot_index).collect()).unwrap_or_default();
        let present = ctx.map(|c| c.members.iter().filter(|m| m.position.is_some()).count()).unwrap_or(0);
        let engaged_once = ctx.map(|c| c.engaged_once).unwrap_or(false);
        let has_members = !filled.is_empty();
        let requested = comp.map(|c| c.slots.len()).unwrap_or(0);
        let objective_gone = obj.is_none();

        // DONOR: merge-eligible iff terminal-with-survivors (objective gone, members alive) OR a FORMING
        // squad consolidating (has members, not yet committed to a fight). A mid-fight (engaged) squad is
        // NEVER eligible — it sheds, never weakens mid-fight (ADR 0027 line 273).
        let forming_consolidate = has_members && !engaged_once && requested > 0 && filled.len() < requested;
        let merge_eligible = (objective_gone && has_members) || forming_consolidate;
        let (sheddable, sheddable_roles) = match comp {
            Some(c) if merge_eligible => sheddable_of(c, &filled, squad_energy),
            _ => (SquadCapabilities::default(), 0),
        };
        // RECEIVER: a FORMING squad (has at least one present member, not full) offers its OPEN pending slots.
        // An empty squad is not a receiver (it would just spawn its whole roster); a full one has no open slot.
        let is_forming_receiver = present > 0 && requested > 0 && filled.len() < requested && !engaged_once;
        let open_slot_roles = match comp {
            Some(c) if is_forming_receiver => open_slot_roles_of(c, &filled),
            _ => 0,
        };

        rows.push(SquadRow {
            caps,
            class,
            // A gone objective ⇒ no StayPut fight (the reconcile retire path owns it); None ⇒ StayPut infeasible.
            current_objective: obj.map(|o| o.id.0),
            recycle_ev: 0,
            merge_eligible,
            sheddable,
            sheddable_roles,
            open_slot_roles,
        });
    }

    // ── COLUMNS: all live objectives, in the queue's stable Vec order. Per-row feasibility = NOT claimed by
    //    ANOTHER squad, NOT in give-up backoff, AND NOT the row's OWN current objective (no-ping-pong — the
    //    current fight is reachable only via StayPut). The capability-class match is the kernel's own pre-
    //    filter (`SquadRow.class` vs `ObjectiveCell.class`). ──
    let objectives: Vec<&super::objective_queue::CombatObjective> = data.objective_queue.iter_objectives().collect();
    let mut cells: Vec<ObjectiveCell> = Vec::with_capacity(objectives.len());
    for o in &objectives {
        let room = o.kind.room();
        let unwinnable = data.objective_queue.is_unwinnable_now(room, now);
        let claimed_by = data.objective_queue.claimed_by(o.id);
        let travel = anchor.map(|h| room_distance(h, room)).unwrap_or(0);
        // Per-row travel + feasibility (parallel to `rows`).
        let mut travel_rooms_per_row = Vec::with_capacity(rows.len());
        let mut feasible_per_row = Vec::with_capacity(rows.len());
        for (entity, cur_id) in managed {
            travel_rooms_per_row.push(travel);
            // Feasible to REASSIGN onto iff: not the row's current objective (StayPut owns that), the room is
            // winnable, and (it is unclaimed OR claimed by THIS very squad — its own claim never blocks it).
            let is_own_current = *cur_id == o.id;
            let claimed_by_other = matches!(claimed_by, Some(c) if c != *entity);
            feasible_per_row.push(!is_own_current && !unwinnable && !claimed_by_other);
        }
        let econ = data.objective_queue.economic_intel(o.id);
        cells.push(ObjectiveCell {
            id: o.id.0,
            class: cap_class(capability_class(&o.kind)),
            value_kind: project_value_kind(&o.kind, econ),
            intel: project_intel(&o.kind, o.priority, asset_of(room), threat_for(room), econ),
            defense: project_defense(threat_for(room)),
            enemy: project_enemy(threat_for(room)),
            travel_rooms_per_row,
            feasible_per_row,
        });
    }

    // ── ADR 0032 v2 — the donor→receiver-rally transfer-travel matrix (row-major `rows × rows`). The
    //    receiver is the coordination unit (rallies at/near home), so the transfer cost ≈ the donor's
    //    distance from the receiver's objective room (both forming squads rally home → typically ~0).
    //    Deterministic (Vec order; no HashMap). ──
    let obj_room_of = |obj_id: ObjectiveId| -> Option<RoomName> { data.objective_queue.get(obj_id).map(|o| o.kind.room()) };
    let mut merge_travel_rooms: Vec<u32> = vec![0; managed.len() * managed.len()];
    for (di, (_, d_obj)) in managed.iter().enumerate() {
        for (ri, (_, r_obj)) in managed.iter().enumerate() {
            let t = match (obj_room_of(*d_obj), obj_room_of(*r_obj)) {
                (Some(dr), Some(rr)) => room_distance(dr, rr),
                _ => 0,
            };
            merge_travel_rooms[di * managed.len() + ri] = t;
        }
    }

    // The on-site window proxy (a reassign reuses already-spawned bodies — a generous window, v1.1 parity).
    let params = MatrixParams { onsite_window: MAX_TRAVEL_BUDGET, pairing: PairingParams::default(), w_transfer: 1.0 };
    let matrix = build_ev_matrix_with_merge(&rows, &cells, &merge_travel_rooms, &params);
    let solution = solve_assignment(&matrix);

    // Map each squad's assigned column back to a NEW objective id, applying the EV-POSITIVE GATE against
    // StayPut: a reassign fires only if the chosen objective beats the row's StayPut EV by MORE than the
    // commit threshold (so a marginal swap does not thrash — v1.1 parity). StayPut/Recycle columns ⇒ no
    // reassign (absent from the map). A column whose id == the row's current objective is impossible (the
    // no-ping-pong feasibility filter excludes it), but we guard anyway.
    let commit_threshold_q = quantize_ev(COMMIT_EV_THRESHOLD);
    let stay_base = cells.len(); // the first StayPut column index
    let mut merges: Vec<MergeDecision> = Vec::new();
    for (r, (entity, cur_id)) in managed.iter().enumerate() {
        let Some(col) = solution.row_to_col[r] else { continue };
        match matrix.columns[col] {
            ColumnKind::Objective { id } => {
                if id == cur_id.0 {
                    continue; // defensive — already excluded by feasibility
                }
                let new_ev = matrix.at(r, col);
                let stay_ev = matrix.at(r, stay_base + r); // this row's private StayPut column EV
                // The gate: only reassign if the global pick beats continuing the current fight by the threshold.
                if new_ev - stay_ev > commit_threshold_q {
                    out.insert(*entity, ObjectiveId(id));
                }
            }
            // ── ADR 0032 v2 / ADR 0027 — row `r` (the DONOR) MERGES into receiver row's pending slot. The
            //    merge cell EV is the receiver's MARGINAL P(win) lift; a merge fires only if it is net-positive
            //    by the same commit threshold (a marginal lift does not thrash). ──
            ColumnKind::Merge { receiver_row } => {
                let merge_ev = matrix.at(r, col);
                let stay_ev = matrix.at(r, stay_base + r);
                // The donor merges only if the lift beats keeping the donor's own fight by the threshold (and
                // is feasible — solve never returns an INFEASIBLE_EV cell as a real match, but guard anyway).
                if merge_ev != screeps_combat_decision::assignment::INFEASIBLE_EV
                    && merge_ev.saturating_sub(stay_ev.max(0)) > commit_threshold_q
                {
                    let receiver = managed[receiver_row].0;
                    merges.push(MergeDecision { donor: *entity, receiver, roles: rows[r].sheddable_roles });
                }
            }
            ColumnKind::StayPut { .. } | ColumnKind::Recycle { .. } => {}
        }
    }
    (out, merges)
}

/// ADR 0032 v2 / ADR 0027 — apply the chosen `Merge→Bk` TRANSFERS (lines 256-312 of ADR 0027). For each
/// decision: transfer the donor's role-matched present member(s) into the RECEIVER's OPEN pending slot(s)
/// (rebind the creep's `SquadCombatJob` squad-ref + target room to the receiver, move the `SquadMember` from
/// the donor `SquadContext` to the receiver's, re-keyed to the receiver's open `slot_index`), then if the
/// donor is now EMPTY delete its squad entity DIRECTLY via `world.delete_entity` (the SAME route
/// `retire_squad` uses — neither goes through `EntityCleanupQueue`). The direct delete is SAFE precisely
/// because the donor has shed ALL its members (the creeps were TRANSFERRED, not orphaned/deleted): an empty
/// donor holds no live member refs, and creeps hold a non-`ConvertSaveload` `Option<SquadRef>` (validate-on-
/// access), so no dangling Entity ref survives to serialize. The now-filled receiver slot is dropped from the
/// spawn queue automatically: Phase B checks `is_slot_filled(slot_index)`, which becomes true once the
/// transferred member occupies it.
///
/// SAFETY (ADR 0027 line "SAFE entity ops" / the ECS dangling-ref serialize panic history): NOTHING is
/// routed through `entities.delete` for a creep — the creeps stay alive and bound to EXACTLY ONE squad
/// (removed from the donor's `members` in the SAME `exec_mut` they are added to the receiver's), so
/// `get_creeps()` / `repair_entity_integrity` / `ConvertSaveload` never see a dangling or doubly-owned ref.
/// Only the now-EMPTY donor squad ENTITY is deleted (directly — see above), never a creep.
/// All membership + job rebinds happen inside ONE `exec_mut` per decision (full world access), reading the
/// LIVE post-spawn world; the receiver composition's slot→role map is captured BEFORE the closure.
fn apply_merges(data: &mut SquadManagerSystemData, merges: &[MergeDecision], _now: u32, debug: bool) {
    for m in merges {
        // Capture the receiver's objective composition (slot→role) + target room BEFORE the closure (the
        // queue is not available inside exec_mut). Skip a decision whose receiver objective vanished.
        let Some(recv_obj) = data
            .squad_contexts
            .get(m.receiver)
            .and_then(|ctx| ctx.objective_id)
            .and_then(|id| data.objective_queue.get(id))
        else {
            continue;
        };
        let Some(recv_comp) = recv_obj.force.squads.first() else { continue };
        // (slot_index, role) for every receiver slot, in stable order.
        let recv_slots: Vec<(usize, screeps_combat_decision::composition::SquadRole)> =
            recv_comp.slots.iter().enumerate().map(|(i, s)| (i, s.role)).collect();
        let recv_target_room = objective_target(&recv_obj.kind).1;
        let donor = m.donor;
        let receiver = m.receiver;
        let shed_roles = m.roles;

        data.updater.exec_mut(move |world| {
            // Both squads must still be alive (a concurrent retire could have removed one).
            if !world.entities().is_alive(donor) || !world.entities().is_alive(receiver) {
                return;
            }
            // ── 1) Compute the transfers from the LIVE world: receiver's OPEN slots (not occupied by a live
            //    member) whose role matches a shed role, paired greedily-in-stable-order with the donor's
            //    present members of that role. Deterministic (Vec order; no HashMap). ──
            let mut transfers: Vec<(Entity, usize, screeps_combat_decision::composition::SquadRole)> = Vec::new(); // (creep, recv_slot_index, role)
            {
                let contexts = world.read_storage::<SquadContext>();
                let Some(recv_ctx) = contexts.get(receiver) else { return };
                let Some(donor_ctx) = contexts.get(donor) else { return };
                // Open receiver slots (role-matched to a shed role), still-needed in stable order.
                let mut open_slots: Vec<(usize, screeps_combat_decision::composition::SquadRole)> = recv_slots
                    .iter()
                    .copied()
                    .filter(|(idx, role)| {
                        (role_bit(*role) & shed_roles) != 0 && !recv_ctx.members.iter().any(|mem| mem.slot_index == *idx)
                    })
                    .collect();
                // Donor's present members eligible to shed (a resolved position = a real body), stable order.
                let donor_members: Vec<(Entity, screeps_combat_decision::composition::SquadRole)> =
                    donor_ctx.members.iter().filter(|mem| mem.position.is_some()).map(|mem| (mem.entity, mem.role)).collect();
                // Greedy role-match: each open slot pulls the FIRST unused donor member of the same role.
                let mut used: Vec<bool> = vec![false; donor_members.len()];
                for (slot_idx, slot_role) in open_slots.drain(..) {
                    if let Some(pos) = (0..donor_members.len()).find(|&i| !used[i] && donor_members[i].1 == slot_role) {
                        used[pos] = true;
                        transfers.push((donor_members[pos].0, slot_idx, slot_role));
                    }
                }
            }
            if transfers.is_empty() {
                return;
            }

            // ── 2) Apply the membership move + the job rebind for each transfer (the creep ends up owned by
            //    EXACTLY ONE squad). Remove from the donor FIRST, then add to the receiver. ──
            {
                let mut contexts = world.write_storage::<SquadContext>();
                for (creep, slot_idx, role) in &transfers {
                    if let Some(donor_ctx) = contexts.get_mut(donor) {
                        donor_ctx.members.retain(|mem| mem.entity != *creep);
                    }
                    if let Some(recv_ctx) = contexts.get_mut(receiver) {
                        recv_ctx.add_member(*creep, *role, *slot_idx);
                    }
                }
            }
            {
                let mut jobs = world.write_storage::<crate::jobs::data::JobData>();
                for (creep, _slot_idx, _role) in &transfers {
                    if let Some(crate::jobs::data::JobData::SquadCombat(job)) = jobs.get_mut(*creep) {
                        job.rebind_to_squad(recv_target_room, receiver);
                    }
                }
            }
            if debug {
                log::info!(
                    "[Lifecycle] MERGE donor={:?} -> receiver={:?} transferred={} member(s) into open pending slot(s) (ADR 0027 pending-slot transfer)",
                    donor,
                    receiver,
                    transfers.len()
                );
            }

            // ── 3) If the donor is now EMPTY, retire it cleanly (the creeps were transferred, not deleted).
            //    A PARTIAL donor (members left) keeps its objective — the per-squad reconcile classifies it
            //    next. We only delete the EMPTY donor squad entity here (no creep deletion). ──
            let donor_empty = world.read_storage::<SquadContext>().get(donor).map(|c| c.members.is_empty()).unwrap_or(true);
            if donor_empty && world.entities().is_alive(donor) {
                let _ = world.delete_entity(donor);
            }
        });
        // NOTE: the donor's ephemeral objective claim is left to the per-squad reconcile (it re-claims live
        // squads each tick) / `release_entity` on the deferred delete — a merge is a FORCE move, not an
        // objective resolution, and a PARTIAL donor keeps its claim, so we must NOT release here.
    }
}

/// Map an objective to the squad's target + the room its members travel to.
fn objective_target(kind: &ObjectiveKind) -> (SquadTarget, RoomName) {
    match kind {
        ObjectiveKind::Defend { room } => (SquadTarget::DefendRoom { room: *room }, *room),
        ObjectiveKind::Harass { room } => (SquadTarget::HarassRoom { room: *room }, *room),
        ObjectiveKind::Dismantle { room, pos } => (SquadTarget::AttackStructure { position: *pos }, *room),
        // ADR 0027 v1.1 P2: a declaim squad travels to the room and `attackController`s the controller tile.
        ObjectiveKind::Declaim { room, controller } => (SquadTarget::AttackController { position: *controller }, *room),
        // Secure / Farm / Escort all reduce to "go to the room and clear it";
        // the SquadCombatJob self-drives there and engages whatever is hostile.
        ObjectiveKind::Secure { room } | ObjectiveKind::Farm { room, .. } | ObjectiveKind::Escort { room } => {
            (SquadTarget::AttackRoom { room: *room }, *room)
        }
    }
}

/// ADR 0032 v2 — the spawn-completion REGISTRATION decision: should the freshly-spawned creep be added to
/// its receiver squad's roster at `slot_index`? Only when the squad is still ALIVE **and** that slot is not
/// ALREADY filled. A `false` result means the creep must NOT be added (it would over-roster the squad):
///
///   * squad dead — the squad died during the spawn delay (the recycled-slot / retired-squad case);
///   * slot already filled — the SAME-TICK DOUBLE-FILL race: a merge-transfer (`apply_merges`) rebinds a
///     donor creep into this very open pending slot via a DEFERRED `exec_mut`, while Phase B (reading the
///     pre-`maintain` live storage that tick) still saw the slot empty and queued THIS spawn. The transfer
///     applies at `maintain` (filling the slot); when this spawn then completes, registering it would push a
///     SECOND member at the same `slot_index` — a surplus creep + an over-rostered (>requested) squad. The
///     recycled-slot reuse race the callback already contemplates produces the same "slot already filled"
///     state, so one recheck covers both.
///
/// When this returns `false` the caller still BUILDS the creep entity with a squad-bound `SquadCombatJob`
/// (so it is ECS-tracked) but skips `add_member`; the job's zero-orphan recall (ADR 0027 §(d)) then walks it
/// home to recycle rather than leaving it stranded. Pure so it is host-testable without an ECS world.
fn should_register_spawned_member(squad_alive: bool, slot_already_filled: bool) -> bool {
    squad_alive && !slot_already_filled
}

/// The spawn-completion callback: mints the creep entity with a squad-bound
/// `SquadCombatJob` and registers it on the `SquadContext`. Mirrors
/// `AttackMission::create_spawn_callback`.
fn create_spawn_callback(
    role: screeps_combat_decision::composition::SquadRole,
    slot_index: usize,
    target_room: RoomName,
    squad_entity: Entity,
) -> SpawnQueueCallback {
    Box::new(move |system_data, name| {
        let name = name.to_string();
        system_data.updater.exec_mut(move |world| {
            // Generation-safe: the squad may have died during the spawn delay and its ECS slot been
            // recycled. `is_alive` on the FULL entity (generation included) rejects a recycled slot,
            // so we never register the fresh creep onto a *different* squad that now occupies the
            // index (the recycled-slot aliasing bug). `squad_entity` is captured whole — not as a
            // bare `.id()` reconstructed via `entity(id)`, which would alias.
            let squad_alive = world.entities().is_alive(squad_entity);

            // ADR 0032 v2 (same-tick DOUBLE-FILL guard): recheck whether this slot is ALREADY filled before
            // registering. A merge-transfer (`apply_merges`) can have rebound a donor creep into this very
            // open pending slot via a DEFERRED `exec_mut` that applied at `maintain` (AFTER Phase B, reading
            // the still-empty live storage, queued THIS spawn). Reading the LIVE post-`maintain` storage here
            // sees that fill, so we never push a SECOND member at `slot_index`. (Also covers the recycled-slot
            // reuse race the callback already contemplated — same "slot already filled" state.)
            let slot_already_filled = squad_alive
                && world
                    .read_storage::<SquadContext>()
                    .get(squad_entity)
                    .map(|ctx| ctx.is_slot_filled(slot_index))
                    .unwrap_or(false);

            // Always BUILD the creep entity with a squad-bound `SquadCombatJob` — so it is ECS-tracked and
            // carries the zero-orphan recall machinery (ADR 0027 §(d)) — and THEN decide registration. A creep
            // we do NOT register (squad dead, or its slot already filled by a merge transfer) is a surplus that
            // must still be cleaned up: its job recalls it home to recycle rather than orphaning it in-world.
            let creep_job = crate::jobs::data::JobData::SquadCombat(crate::jobs::squad_combat::SquadCombatJob::new_with_squad(
                target_room,
                squad_entity,
            ));
            let creep_entity = spawning::build(world.create_entity(), &name).with(creep_job).build();

            if !should_register_spawned_member(squad_alive, slot_already_filled) {
                log::warn!(
                    "[SquadManager] Spawn callback: squad {:?} {} for creep {} (slot {}); NOT registered — its \
                     squad-bound job recalls it home to recycle (zero-orphan)",
                    squad_entity,
                    if !squad_alive { "no longer alive" } else { "slot already filled (merge-transfer surplus)" },
                    name,
                    slot_index
                );
                return;
            }

            if let Some(squad_ctx) = world.write_storage::<SquadContext>().get_mut(squad_entity) {
                squad_ctx.add_member(creep_entity, role, slot_index);
            } else {
                log::warn!(
                    "[SquadManager] Spawn callback: SquadContext missing for {:?}, creep {} (slot {}) not registered",
                    squad_entity,
                    name,
                    slot_index
                );
            }
        });
    })
}

pub struct SquadManagerSystem;

#[derive(SystemData)]
pub struct SquadManagerSystemData<'a> {
    entities: Entities<'a>,
    updater: Read<'a, LazyUpdate>,
    objective_queue: Write<'a, CombatObjectiveQueue>,
    forming_progress: Write<'a, SquadFormingProgress>,
    squad_contexts: WriteStorage<'a, SquadContext>,
    spawn_queue: Write<'a, SpawnQueue>,
    room_data: ReadStorage<'a, RoomData>,
    // ADR 0032 v1.1: the per-room scouted intel the EV-of-pairing helper reads (threat danger → value_e for a
    // defense objective; towers/dps/safe-mode → the `DefenseProfile` P(win) judges against). Read-only.
    threat_data: ReadStorage<'a, crate::military::threatmap::RoomThreatData>,
    mapping: Read<'a, EntityMappingData>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    visibility: Write<'a, VisibilityQueue>,
    features: Read<'a, crate::features::Features>,
}

/// A home room that can act as a spawn source for a squad.
struct HomeRoom {
    entity: Entity,
    name: RoomName,
    energy_capacity: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for SquadManagerSystem {
    type SystemData = SquadManagerSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let now = game::time();
        // P-OBJ #23 lifecycle introspection: reuse the war debug flag for low-noise, per-event squad/
        // objective lifecycle logs (field / reach / engage / retire-reason) so a live capture pinpoints
        // WHICH stage a squad fails at, instead of guessing from Docker.
        let debug = data.features.military.debug_log;

        // ── Gather candidate home rooms (owned, has an idle-capable spawn). ──
        let homes: Vec<HomeRoom> = (&data.entities, &data.room_data)
            .join()
            .filter_map(|(entity, rd)| {
                let dvd = rd.get_dynamic_visibility_data()?;
                if !dvd.owner().mine() {
                    return None;
                }
                let structures = rd.get_structures()?;
                if structures.spawns().iter().all(|s| !s.my()) {
                    return None;
                }
                let energy_capacity = game::rooms().get(rd.name).map(|r| r.energy_capacity_available()).unwrap_or(0);
                if energy_capacity == 0 {
                    return None;
                }
                Some(HomeRoom {
                    entity,
                    name: rd.name,
                    energy_capacity,
                })
            })
            .collect();

        // ── Phase A: reconcile existing manager-owned squads. ──
        // STABLE-ORDERED (by Entity id, never the join's arbitrary ECS order) so the global Hungarian's ROWS
        // are deterministic (ADR 0032 §Determinism — "stable id, never `Entity` index" for the matrix order).
        let mut managed: Vec<(Entity, ObjectiveId)> = (&data.entities, &data.squad_contexts)
            .join()
            .filter_map(|(e, ctx)| ctx.objective_id.map(|id| (e, id)))
            .collect();
        managed.sort_by_key(|(e, _)| e.id());

        // ── ADR 0032 v1.2: the GLOBAL EV-maximizing matching (Hungarian) over ALL managed squads × ALL
        //    claimable objectives — computed ONCE here, between Phase-A classify (the per-squad reconcile
        //    below) and apply. This REPLACES the v1.1 per-squad greedy `best_by_ev` reassign SELECTION: the
        //    per-squad loop now CONSULTS this global solution (`global_reassign[entity]` = the squad's
        //    globally-optimal NEW objective, or None ⇒ stay/recycle) instead of each squad greedily grabbing
        //    its own best + `covered`-marking it. Column-exclusivity makes a double-claim impossible (so the
        //    `covered` guard is retired for reassign). The lease/forming/travel lifecycle (the per-squad
        //    reconcile) is ORTHOGONAL and unchanged — reconcile still decides retire-vs-keep; only WHICH new
        //    objective a Reassign binds to is now the global optimum, not a greedy pick. ──
        let (global_reassign, global_merges): (std::collections::HashMap<Entity, ObjectiveId>, Vec<MergeDecision>) =
            solve_global_reassignment(&data, &managed, &homes, now);

        // ── ADR 0032 v2 / ADR 0027 — apply the chosen MERGE transfers before the per-squad reconcile loop
        //    below. NOTE the transfer is DEFERRED (it runs inside `exec_mut`, applied at `world.maintain()`),
        //    so the reconcile loop THIS tick still observes the donor with its members — it is NOT the
        //    reconcile that retires an emptied donor. Instead `apply_merges` itself, once the donor has shed
        //    all members, deletes the now-EMPTY donor squad ENTITY DIRECTLY via `world.delete_entity` (the
        //    SAME route `retire_squad` uses — neither goes through `EntityCleanupQueue`; the one-tick deferral
        //    is harmless). The receiver fields its now-filled slot by transfer (the spawn queue drops the slot
        //    because `is_slot_filled` now reports it filled). The transfer rebinds each shed creep's squad-ref
        //    + slot to the receiver's open pending slot (the creep ends up owned by EXACTLY ONE squad), routes
        //    NO CREEP through `entities.delete` (see the ECS dangling-ref panic history), and keeps every
        //    squad's `get_creeps()`/members correct so serialize + repair_entity_integrity do not hit a
        //    dangling Entity ref (the direct donor-entity delete is safe because the donor is empty). ──
        apply_merges(&mut data, &global_merges, now, debug);

        let mut live_managed: Vec<(Entity, ObjectiveId)> = Vec::new();
        let mut covered: std::collections::HashSet<ObjectiveId> = std::collections::HashSet::new();

        for (squad_entity, obj_id) in managed {
            // Snapshot the objective once (all Copy) so the queue mutations below don't fight a borrow.
            let obj_info = data
                .objective_queue
                .get(obj_id)
                .map(|o| (o.kind.room(), matches!(o.kind, ObjectiveKind::Defend { .. }), o.deadline, capability_class(&o.kind)));
            let objective_gone = obj_info.is_none();
            let squad_room = obj_info.map(|(r, _, _, _)| r);
            let is_defend = obj_info.map(|(_, d, _, _)| d).unwrap_or(false);
            let cur_class = obj_info.map(|(_, _, _, c)| c);
            // ADR 0027 v1.1 P2: a DECLAIM objective (a CLAIM declaimer). Drives the `declaiming` lease-hold
            // below so the squad persists across the 1000-tick cadence (a declaimer has no focus to refresh on).
            let is_declaim = cur_class == Some(CapabilityClass::Declaim);
            // P-OBJ #23: has the commitment lease lapsed (the squad failed to make progress in time)?
            let deadline_lapsed = obj_info.and_then(|(_, _, dl, _)| dl).is_some_and(|d| now >= d);

            // Snapshot the squad facts (Copy) in one borrow.
            // ADR 0034 D5: also collect PER-MEMBER (entity, room-distance-to-target) so the travel lease can
            // refresh on a MAJORITY closing (not the single min). `member_dists` is empty while forming.
            let (wiped, has_focus, engaged_once, in_target_room, has_members, present_count, target_dist, member_dists) = data
                .squad_contexts
                .get(squad_entity)
                .map(|ctx| {
                    // Wave-wipe (P2.G4-O4): the squad had members and all are now dead — overwhelmed.
                    let wiped = squad_is_wiped(ctx.total_members_added, ctx.members.len());
                    let in_room = squad_room
                        .map(|room| ctx.members.iter().any(|m| m.position.is_some_and(|p| p.room_name() == room)))
                        .unwrap_or(false);
                    // FIX 2: count members PRESENT in the world (a resolved position) — a still-spawning
                    // slot has no body yet and must not count as progress. Matches the rally gate's notion.
                    let present = ctx.members.iter().filter(|m| m.position.is_some()).count();
                    // Deep-reach fix (Break #2 travel half): the CLOSEST present member's room-distance to the
                    // target room — the travel-progress signal. Decreasing ⇒ the squad is closing on the
                    // target (refresh the travel lease); flat/increasing ⇒ stuck (let it give up). `None`
                    // when no member has a position yet (still forming) — handled as "no travel progress".
                    let dist = squad_room.and_then(|room| {
                        ctx.members
                            .iter()
                            .filter_map(|m| m.position.map(|p| room_distance(p.room_name(), room)))
                            .min()
                    });
                    // ADR 0034 D5: per-member (entity-id, room-distance-to-target) for the majority signal.
                    let dists: Vec<(u32, u32)> = squad_room
                        .map(|room| {
                            ctx.members
                                .iter()
                                .filter_map(|m| m.position.map(|p| (m.entity.id(), room_distance(p.room_name(), room))))
                                .collect()
                        })
                        .unwrap_or_default();
                    (wiped, ctx.focus_target.is_some(), ctx.engaged_once, in_room, !ctx.members.is_empty(), present, dist, dists)
                })
                .unwrap_or((false, false, false, false, false, 0, None, Vec::new()));
            // ADR 0035 D4: the squad's PREVIOUS-tick LOSE VERDICT over the REAL in-room view, CARRIED from
            // Phase B (`compute_squad_orders` stamps `lost_in_room` AFTER `apply_squad_decision`) — the
            // GENUINE lose `engaged_once && in_room_any && !present_force_wins_or_stalls`, NOT the broader
            // `ctx.state == Retreating` superset (which also fires for a critical-HP / low-avg / stalemate
            // retreat on a WINNABLE fight — exactly the false-abandon this carrier replaces). Reading the
            // carrier here is the EXACT INVERSE of `present_force_wins_or_stalls`, so the abandon verdict
            // (built below) cannot disagree with whether the squad is actually LOSING — and we do NOT rebuild
            // the SquadView in Phase A (the lose verdict is carried, not recomputed). Ephemeral membership
            // read (NOT serialized — no `WORLD_FORMAT_VERSION` bump; `contains`, no result-affecting iteration).
            let lost_in_room = data.forming_progress.lost_in_room.contains(&obj_id);

            // FIX 2 (rally-stall): a forming squad legitimately sitting at home assembling its roster has
            // no focus, so the base +400 lease lapses and the kernel would retire it mid-form → re-field →
            // Generation churn that orphans the already-spawned members. Tell the kernel whether the squad
            // is still FORMING and whether it made spawn PROGRESS since the last reconcile, so it refreshes
            // the lease while progressing (bounded — progress can only be true `requested` times). Requested
            // roster size off the objective (the producer owns it).
            let requested_slots_for_form = data
                .objective_queue
                .get(obj_id)
                .and_then(|o| o.force.squads.first())
                .map(|c| c.slots.len())
                .unwrap_or(0);
            let prev_present = data.forming_progress.last_present.get(&obj_id).copied().unwrap_or(0);
            let (forming, forming_progress) =
                forming_state(has_members, engaged_once, present_count, requested_slots_for_form, prev_present);
            // Record this tick's present count for the next reconcile's progress delta.
            data.forming_progress.last_present.insert(obj_id, present_count);

            // ── Deep-reach fix (Break #1, the forming-lease): a forming squad has a slot still QUEUED or
            // IN FLIGHT (an unfilled slot Phase B re-queues every tick = a member banking/spawning) whenever
            // it is forming — so refresh the lease through the inter-member banking gap, NOT only on the exact
            // present++ tick (which lapsed between members under contention → re-field churn). BOUNDED by a
            // per-generation forming clock: past MAX_FORMING_BUDGET the refresh stops and the squad gives up.
            let forming_in_flight = forming;
            let forming_started_at = *data.forming_progress.forming_started_at.entry(obj_id).or_insert(now);
            let forming_budget_remaining = now.saturating_sub(forming_started_at) < MAX_FORMING_BUDGET;

            // ── Deep-reach fix (Break #2 travel half, the travel-lease): a FULL-ROSTER squad that has departed
            // home but not yet engaged / arrived is TRAVELING — it has no focus and is not forming, so the
            // base lease lapses mid-hop (the W7N7 1-slot lapse). Refresh while it is closing distance on the
            // target room (positional progress), BOUNDED by an absolute travel clock from the departure tick.
            let full_roster = requested_slots_for_form > 0 && present_count >= requested_slots_for_form;
            let traveling = full_roster && !engaged_once && !in_target_room && has_members;
            let departed_at = if traveling {
                *data.forming_progress.departed_at.entry(obj_id).or_insert(now)
            } else {
                data.forming_progress.departed_at.remove(&obj_id);
                now
            };
            let travel_budget_remaining = now.saturating_sub(departed_at) < MAX_TRAVEL_BUDGET;
            // ── ADR 0034 D5 (RC-4/RC-8 — per-member + MAJORITY travel progress). Refresh the travel lease
            // while a MAJORITY of PRESENT members are CLOSING distance on the target (or arrived in it), NOT
            // while the single closest is. The old MIN-over-members signal let ONE stuck member pin the lease
            // "stalled" while the bulk advanced (RC-4), OR a single moving lead mask a stuck bulk — both
            // mis-read a converging/stuck squad. Per-member, keyed by entity (stable), tracked ephemerally.
            let mut closing = 0usize;
            let mut counted = 0usize;
            for &(ent_id, cur) in &member_dists {
                counted += 1;
                let key = (obj_id, ent_id);
                let prev = data.forming_progress.member_target_dist.get(&key).copied();
                // Closing = the per-member distance strictly decreased, the member is IN the target room
                // (dist 0), or it is the first reading (assume progress for one reconcile).
                if cur == 0 || matches!(prev, Some(p) if cur < p) || prev.is_none() {
                    closing += 1;
                }
                data.forming_progress.member_target_dist.insert(key, cur);
            }
            let majority_closing = counted > 0 && closing * 2 > counted;
            let travel_progress = traveling && majority_closing;
            // Keep the MIN-distance tracker fresh for the introspection trace (logging only — not the gate).
            if let Some(d) = target_dist {
                data.forming_progress.last_target_dist.insert(obj_id, d);
            }

            // ── ADR 0032 v1.2 (whole-squad REASSIGN, GLOBAL Hungarian): the squad's reassign target is the
            //    column the GLOBAL solve assigned this squad's row (`solve_global_reassignment` above), NOT a
            //    per-squad greedy `best_by_ev` pick. The global solve already applied: the capability-class
            //    pre-filter (an Offense squad never reassigns onto Defense), the EV-positive gate (the
            //    per-row StayPut/Recycle columns — a sub-threshold/net-negative move is never the optimum),
            //    column-exclusivity (no two squads target the same objective — the v1 `covered` double-claim
            //    guard is RETIRED for reassign), and the no-ping-pong exclusion (the row's own current
            //    objective is reachable only via StayPut, never as a reassign column). So here we just LOOK UP
            //    the global decision: `Some(new_id)` ⇒ the optimum moves this squad to `new_id`; absent ⇒
            //    StayPut/Recycle (keep the current fight / the reconcile retire path handles a gone target).
            //    The reconcile kernel still decides retire-vs-keep; `reassign_available` only tells it a
            //    globally-better objective EXISTS for a non-loss terminal. ──
            let best_reassignment = global_reassign.get(&squad_entity).copied();
            let reassign_available = best_reassignment.is_some();

            // P-OBJ #23 / ADR 0027: the pure reconcile kernel decides retire-vs-keep (unit-tested offline
            // in `screeps_combat_decision::lifecycle`). The manager only builds the snapshot and applies the
            // action — single source of truth, shared with the offline lifecycle harness (no drift).
            let snapshot = lifecycle::ReconcileSnapshot {
                objective_gone,
                duplicate: covered.contains(&obj_id),
                is_defend,
                deadline_lapsed,
                wiped,
                has_focus,
                engaged_once,
                in_target_room,
                has_members,
                forming,
                forming_progress,
                forming_in_flight,
                forming_budget_remaining,
                traveling,
                travel_progress,
                travel_budget_remaining,
                // FIX B2: a Defend squad garrisoning its CLEAR owned room (arrived, no in-room focus) holds
                // its lease while the Defend objective persists, instead of GaveUp+refield (Gen churn). The
                // owned-room threat roams a NEIGHBOUR room, so the owned room itself shows no in-room focus.
                holding_station: is_defend && in_target_room && !has_focus,
                // ADR 0027 v1.1 P2: an in-room declaimer is HOLDING (striking on the 1000-tick cadence), so
                // refresh its lease + block the false Resolve while it neutralizes the controller. Bounded by
                // the objective lifecycle: the producer withdraws on controller-neutral / re-arm → objective_gone.
                declaiming: is_declaim && in_target_room && has_members,
                reassign_available,
                // ADR 0035 D4 (ABANDON-ON-UNWINNABLE-CONTACT): reached + engaged + the real in-room P(win) =
                // LOSE. The kernel splits this from a clean clear so a lost fight is BACKED OFF (GaveUp +
                // mark_unwinnable), not withdrawn-as-clean (which invited an instant re-field → the
                // reach↔retreat spiral). `lost_in_room` is the GENUINE lose verdict carried from Phase B (the
                // EXACT inverse of `present_force_wins_or_stalls` over the real in-room view) — NOT
                // `ctx.state == Retreating`, which is a SUPERSET that also fires for a critical-HP / low-avg /
                // stalemate retreat on a WINNABLE fight (that false-abandon retired bloodied-but-winning
                // squads mid-fight + backed off winnable rooms). It already encodes `engaged_once &&
                // in_room_any`; the `engaged_once && in_target_room` here is a defensive re-gate so it never
                // fires en route or before contact even if the carrier is momentarily stale.
                retreated_from_contact: engaged_once && in_target_room && lost_in_room,
            };
            let action = lifecycle::reconcile(snapshot);
            if let lifecycle::ReconcileAction::Retire { reason, withdraw, mark_unwinnable } = action {
                if debug {
                    log::info!(
                        "[Lifecycle] RETIRE squad={:?} obj={:?} reason={:?} engaged_once={} in_room={} focus={} deadline_lapsed={} members={}",
                        squad_entity, obj_id, reason, engaged_once, in_target_room, has_focus, deadline_lapsed, has_members
                    );
                    // GIVE-UP BREAKDOWN (introspection only): spell out WHICH bound tripped + the raw clock
                    // values so a `reason=GaveUp` is self-explaining (deadline lapse vs forming-budget vs
                    // travel-budget vs no-progress) without a deploy-observe cycle. Mirrors the kernel's
                    // refresh conditions (we don't re-derive the verdict — that's the kernel's job — we only
                    // attribute it). `deadline` is the absolute lease tick; None ⇒ never stamped.
                    let deadline = obj_info.and_then(|(_, _, dl, _)| dl);
                    let forming_exhausted = forming && !forming_budget_remaining;
                    let travel_exhausted = traveling && !travel_budget_remaining;
                    let forming_no_progress = forming && forming_budget_remaining && !forming_progress;
                    let travel_no_progress = traveling && travel_budget_remaining && !travel_progress;
                    log::info!(
                        "[SquadTrace] GIVEUP squad={:?} obj={:?} deadline_lapsed={} forming_budget_exhausted={} travel_budget_exhausted={} forming_no_progress={} travel_no_progress={} | deadline={:?} now={} gen_start={:?} departed_at={:?} last_target_dist={:?} forming={} traveling={}",
                        squad_entity, obj_id, deadline_lapsed, forming_exhausted, travel_exhausted, forming_no_progress, travel_no_progress,
                        deadline, now,
                        data.forming_progress.forming_started_at.get(&obj_id).copied(),
                        data.forming_progress.departed_at.get(&obj_id).copied(),
                        data.forming_progress.last_target_dist.get(&obj_id).copied(),
                        forming, traveling
                    );
                }
                if withdraw {
                    data.objective_queue.withdraw(obj_id); // clean win — clear the objective so no one re-fields it
                    // ADR 0035 D6: a GENUINE Resolved clear (the only path that sets `withdraw`) is a REAL win
                    // — RESET the give-up backoff for the room so a later legitimate target there is not
                    // suppressed by a stale abandon record. (`mark_unwinnable` is the anti-flicker latch; a
                    // real win clears it.) No-op when the room was never marked unwinnable.
                    if let Some(room) = squad_room {
                        data.objective_queue.clear_unwinnable(room);
                    }
                } else if mark_unwinnable {
                    // ADR 0035 D4/D6: an abandon (GaveUp/Wiped/unwinnable-contact) BACKS the room off — the
                    // exponential backoff IS the anti-flicker latch. Called ONCE per de-commit (this Retire
                    // branch runs once then `continue`s). Defense is exempt (kernel never sets this for
                    // is_defend) — we never abandon an owned room.
                    if let Some(room) = squad_room {
                        data.objective_queue.mark_unwinnable(room, now);
                    }
                }
                retire_squad(&data.updater, &data.entities, squad_entity);
                data.objective_queue.release_entity(squad_entity);
                // Drop ALL per-objective lifecycle trackers so a RE-FIELD (new generation claiming the same
                // objective) restarts the forming + travel budget clocks from scratch (the deep-reach bounds
                // are per-generation, like the offline harness's `gen_start`).
                data.forming_progress.last_present.remove(&obj_id);
                data.forming_progress.forming_started_at.remove(&obj_id);
                data.forming_progress.departed_at.remove(&obj_id);
                data.forming_progress.last_target_dist.remove(&obj_id);
                // Introspection trackers too, so a re-field starts the phase-change/heartbeat trace fresh.
                data.forming_progress.last_phase.remove(&obj_id);
                data.forming_progress.last_engaged.remove(&obj_id);
                // FIX A: clear the assault latch so a RE-FIELD (new generation) re-derives the quorum.
                data.forming_progress.assault_latched.remove(&obj_id);
                // ADR 0035 D4: clear the lost-in-room verdict carrier so a RE-FIELD re-derives it from the
                // live in-room assessment (no stale lose verdict bleeding into a fresh generation).
                data.forming_progress.lost_in_room.remove(&obj_id);
                // ADR 0034 D4/D5/D8: clear the per-member rally/target distance + solo-stall trackers so a
                // RE-FIELD re-derives them (a new generation's members must not inherit a stale block streak).
                clear_member_trackers(&mut data.forming_progress, obj_id);
                continue;
            }
            // ── ADR 0027 v1 (whole-squad REASSIGN): a non-loss terminal (Resolved/ObjectiveGone) with a
            //    compatible sibling available → REBIND THIS SQUAD IN PLACE to the new objective. Bodies are
            //    reused — NO `retire_squad`/`field_new_squad`, NO Generation churn. Atomic: release/withdraw
            //    the old claim → claim the new (+ cover it) → rewrite objective_id/target → reset
            //    engaged_once/focus/state/squad_path → re-key the per-objective clocks under the new id →
            //    reopen the COMMITMENT lease. The Phase-B renew/rally then follow the new rally next tick. ──
            if let lifecycle::ReconcileAction::Reassign { withdraw_old } = action {
                let Some(new_id) = best_reassignment else {
                    // Defensive: the kernel only returns Reassign when `reassign_available` (i.e.
                    // `best_reassignment.is_some()`); if it somehow vanished this tick, fall through to keep.
                    data.objective_queue.claim(obj_id, squad_entity);
                    covered.insert(obj_id);
                    live_managed.push((squad_entity, obj_id));
                    continue;
                };
                // Release/withdraw the OLD objective (withdraw on a clean clear so no one re-fields it).
                data.objective_queue.release_entity(squad_entity);
                if withdraw_old {
                    data.objective_queue.withdraw(obj_id);
                }
                // Claim the NEW objective + add to the Phase-A covered set so a second reassigner this tick
                // cannot double-claim it. Reopen the commitment lease for the new objective.
                data.objective_queue.claim(new_id, squad_entity);
                covered.insert(new_id);
                data.objective_queue.set_deadline(new_id, Some(now + COMMITMENT_BUDGET));
                let new_target = data.objective_queue.get(new_id).map(|o| objective_target(&o.kind));
                // Rewrite the SquadContext IN PLACE: re-point it at the new objective + reset the per-squad
                // engage/travel/path state so it re-gathers + re-approaches the new rally cleanly.
                if let Some(ctx) = data.squad_contexts.get_mut(squad_entity) {
                    ctx.objective_id = Some(new_id);
                    if let Some((target, _room)) = new_target {
                        ctx.target = Some(target);
                    }
                    ctx.engaged_once = false;
                    ctx.focus_target = None;
                    ctx.state = SquadState::Forming;
                    ctx.squad_path = None;
                    ctx.rally_point = None;
                }
                // Re-key the per-objective lifecycle trackers under the NEW id (reuse the re-field cleanup,
                // then stamp fresh clocks) — the deep-reach forming/travel budgets are per-objective, so the
                // reassigned squad gets a fresh forming/travel window at the new target.
                data.forming_progress.last_present.remove(&obj_id);
                data.forming_progress.forming_started_at.remove(&obj_id);
                data.forming_progress.departed_at.remove(&obj_id);
                data.forming_progress.last_target_dist.remove(&obj_id);
                data.forming_progress.last_phase.remove(&obj_id);
                data.forming_progress.last_engaged.remove(&obj_id);
                data.forming_progress.assault_latched.remove(&obj_id);
                // ADR 0035 D4: clear the lost-in-room verdict carrier under the OLD id (reassign is a NON-LOSS
                // terminal so it is false here, but re-key hygiene matches the other per-objective trackers).
                data.forming_progress.lost_in_room.remove(&obj_id);
                // ADR 0034 D4/D5/D8: a reassigned squad gets fresh per-member rally/target/stall trackers at
                // the new target (the old block streak is meaningless against the new rally corridor).
                clear_member_trackers(&mut data.forming_progress, obj_id);
                data.forming_progress.forming_started_at.insert(new_id, now);
                data.forming_progress.last_present.insert(new_id, 0);
                if debug {
                    log::info!(
                        "[Lifecycle] REASSIGN squad={:?} from_obj={:?} to_obj={:?} withdraw_old={} (in-place rebind — bodies reused, no Gen churn)",
                        squad_entity, obj_id, new_id, withdraw_old
                    );
                }
                live_managed.push((squad_entity, new_id));
                continue;
            }
            // Live (Keep / KeepRefreshLease): re-establish the (ephemeral) claim — idempotent, self-heals
            // post-reset. Refresh the commitment lease on KeepRefreshLease — the kernel returns it both while
            // actively engaging (a long fight / vision gap) AND while a FORMING squad is still making spawn
            // progress (FIX 2 — so a squad assembling its roster is not retired mid-form → re-field churn).
            data.objective_queue.claim(obj_id, squad_entity);
            if action == lifecycle::ReconcileAction::KeepRefreshLease {
                data.objective_queue.set_deadline(obj_id, Some(now + COMMITMENT_BUDGET));
            }
            // Intel coverage: keep eyes on a committed objective's room so its intel never goes stale
            // underneath the producer. OBSERVE-only + HIGH so an in-range RCL8 observer refreshes it free;
            // if no observer covers it, commitment + the deadline lease bridge the gap instead.
            if let Some(room) = squad_room {
                data.visibility
                    .request(VisibilityRequest::new(room, VISIBILITY_PRIORITY_HIGH, VisibilityRequestFlags::OBSERVE));
            }
            covered.insert(obj_id);
            live_managed.push((squad_entity, obj_id));
        }

        // ── Phase B: field rosters (spawn unfilled slots) for live squads. ──
        for (squad_entity, obj_id) in &live_managed {
            // Read the composition off the objective each tick (the producer owns it).
            let (slots, target_room, spawn_priority) = match data.objective_queue.get(*obj_id) {
                Some(obj) => match obj.force.squads.first() {
                    Some(comp) => (comp.slots.clone(), objective_target(&obj.kind).1, spawn_priority_for(obj.priority)),
                    None => continue,
                },
                None => continue,
            };

            // FIGHTER-FIRST spawn order (deep-reach fix — Break #1): attempt the FIGHTER slots
            // (RangedDPS / Dismantler / MeleeDPS) BEFORE the Healer / Tank / Hauler slots, so a roster that
            // forms slowly under spawn contention spawns a combat-capable member FIRST. A partial roster
            // (the common contention case) is then a fighter, not a pile of orphaned healers waiting for a
            // fighter that lost the spawn race (the live W7N4 "5 Healers + 1 RangedDPS at present=1/2"
            // healer pile-up). The slot's stable `slot_index` (its composition position) is PRESERVED —
            // only the queue-attempt ORDER changes, so the engaged formation / member tracking is unchanged.
            for slot_index in spawn_order_fighter_first(&slots) {
                let slot = &slots[slot_index];
                let already_filled = data
                    .squad_contexts
                    .get(*squad_entity)
                    .map(|ctx| ctx.is_slot_filled(slot_index))
                    .unwrap_or(false);
                if already_filled {
                    continue;
                }
                queue_slot_spawn(&mut data.spawn_queue, &homes, slot, slot_index, target_room, *squad_entity, spawn_priority, debug);
            }
        }

        // ── Phase B-renew: keep a forming/holding squad's members alive while it rallies (ADR 0028 + ADR 0034
        // D6b RC-5). Without renew, a slow/contested form loses its early members to old age → they drop to
        // unfilled → re-spawn → churn → never all-present; and (RC-5) a FULL-but-still-rallying squad whose
        // members hold at a home spawn (the D6a lifetime gate held a too-short member to top it up before the
        // long crawl) bleeds out the same way. Request a renew for any present member with low TTL THAT IS
        // STILL AT A HOME SPAWN; the spawn system renews creeps adjacent to a free spawn and is gated on room
        // energy, so it never starves spawning, monopolizes a lane, or renews infinitely (a departed member is
        // no longer in a home room → never matched; once topped up + released to travel it leaves the renew).
        //
        // ADR 0034 RC-5 CHANGE: the renew is NO LONGER gated FORMING-ONLY (`filled >= requested { continue }`).
        // A member is renewed iff it is present, AT A HOME ROOM, and below the TTL threshold — so it covers
        // BOTH the slow-form early-member case (Phase 0028) AND the D6a held-at-home lifetime-gate case (RC-5),
        // while a departed/traveling member is intrinsically excluded (no home-room match). The per-member
        // home-room filter is the bound; the spawn system's free-spawn + energy gate is the economy guard.
        for (squad_entity, obj_id) in &live_managed {
            let requested = data
                .objective_queue
                .get(*obj_id)
                .and_then(|o| o.force.squads.first())
                .map(|c| c.slots.len())
                .unwrap_or(0);
            let Some(ctx) = data.squad_contexts.get(*squad_entity) else {
                continue;
            };
            if requested == 0 {
                continue; // unknown roster — no renew (legacy parity)
            }
            // Collect first (immutable ctx + creep_owner borrow), then issue (mutable spawn_queue). A member is
            // renewed iff it is AT A HOME ROOM (still holding/rallying near a home spawn) and below the TTL
            // threshold — a departed/traveling member is far from any home room and is intrinsically skipped.
            let renews: Vec<(Entity, Entity, u32)> = ctx
                .members
                .iter()
                .filter_map(|m| {
                    let pos = m.position?;
                    let home = homes.iter().find(|h| h.name == pos.room_name())?;
                    let ttl = data.creep_owner.get(m.entity).and_then(|co| co.owner.resolve()).and_then(|c| c.ticks_to_live())?;
                    (ttl < RENEW_WHILE_FORMING_TTL).then_some((home.entity, m.entity, ttl))
                })
                .collect();
            for (room, member, ttl) in renews {
                data.spawn_queue.request_renew(room, member, ttl);
                if debug {
                    log::info!("[Lifecycle] RENEW squad={:?} obj={:?} ttl={} (forming/holding — keep the roster alive)", squad_entity, obj_id, ttl);
                }
            }
        }

        // ── Phase B2: compute per-squad tactical orders. ──
        // The *tactics* are the pure `decide_squad` (focus + engage/retreat hysteresis,
        // ADR 0008 §4 / P2.G3) — the SAME code the sim runs. The manager is only the
        // live adapter: it builds the JS-free `SquadView` from `SquadContext` + the room,
        // calls `decide_squad`, and writes the result back as orders/state. No tactics
        // math lives here.
        // ADR 0019 Stage 3b build-once-per-room sharing: the threat field + reachability flood depend
        // only on a room's enemies, not the deciding squad, so they are built ONCE per room (this tick)
        // and reused by every squad fighting there. Per-squad work (the cohesion search) is unaffected.
        let mut room_layers: HashMap<RoomName, (LocalCostMatrix, PositionLayers)> = HashMap::new();
        for (squad_entity, obj_id) in &live_managed {
            let (target_room, formation, requested_slots, deadline) = match data.objective_queue.get(*obj_id) {
                Some(obj) => (
                    objective_target(&obj.kind).1,
                    is_formation_objective(&obj.kind),
                    obj.force.squads.first().map(|c| c.slots.len()).unwrap_or(0),
                    obj.deadline,
                ),
                None => continue,
            };
            // ADR 0031 #39 P3 — the oracle's chosen assault mode for this objective (the war producer attached
            // it to the ephemeral runtime entry). `Some(Drain)` → the drive fires the `DrainBreach` strategy +
            // sets the squad's drain stance; `None`/`Some(Breach)` → the byte-unchanged direct breach/engage.
            let assault_mode = data.objective_queue.assault_mode(*obj_id);
            compute_squad_orders(
                &data.room_data,
                &data.mapping,
                &mut data.squad_contexts,
                &data.creep_owner,
                *squad_entity,
                *obj_id,
                target_room,
                formation,
                assault_mode,
                &mut room_layers,
                debug,
                requested_slots,
                now,
                deadline,
                &mut data.forming_progress,
            );
        }

        // ── Phase C: claim new objectives up to the global cap. ──
        // `skipped` holds objectives we cannot field THIS tick (no requested force,
        // or no spawn-home in range). We pass over them WITHOUT claiming — claiming
        // an unfieldable objective would leak a concurrency slot to a `SquadContext`
        // that never spawns (the pre-removal slot-leak vector for a far operator
        // `defend`-flag room) — and exclude them so the selection loop doesn't spin.
        let mut active = live_managed.len();
        // Count squads still FORMING (incomplete roster). We pace new claims so at most
        // `MAX_FORMING_SQUADS` are forming at once — their slots spawn at HIGH and would otherwise split
        // the scarce high-priority spawn-ticks and starve logistics (see MAX_FORMING_SQUADS).
        let mut forming = live_managed
            .iter()
            .filter(|(se, oid)| {
                let Some(o) = data.objective_queue.get(*oid) else {
                    return false;
                };
                // FIX C (ADR 0029): defense is EXEMPT from the forming pace — defenders deploy immediately
                // (FIX A) and must never queue behind offense. Counting only OFFENSE forming makes the cap
                // serialize offense rosters at <= MAX_FORMING_SQUADS without ever starving owned-room defense.
                if matches!(o.kind, ObjectiveKind::Defend { .. }) {
                    return false;
                }
                let requested = o.force.squads.first().map(|c| c.slots.len()).unwrap_or(0);
                let filled = data.squad_contexts.get(*se).map(|c| c.filled_slot_count()).unwrap_or(0);
                requested > 0 && filled < requested
            })
            .count();
        let claim_anchor = homes.first().map(|h| h.name);
        let claim_energy = homes.first().map(|h| h.energy_capacity).unwrap_or(0);
        let claim_threat_for = |room: RoomName| -> Option<&crate::military::threatmap::RoomThreatData> {
            data.mapping.get_room(&room).and_then(|e| data.threat_data.get(e))
        };
        // ── ADR 0032 v1.2: Phase C as GLOBAL "about-to-field" rows (ADR 0032 §Integration: "Phase C
        //    becomes additional about-to-field rows, capped by the concurrency limits"). The new-squad
        //    fielders are INTERCHANGEABLE generic slots (each fields the objective's OWN requested force), so
        //    the global EV-maximizing assignment over (slots × claimable objectives) reduces to "field the
        //    top-K claimable objectives by their requested-force EV" — provably the global optimum for
        //    identical rows (a Hungarian over a constant-per-column matrix picks the K largest columns). We
        //    therefore pre-rank ALL claimable objectives by the SAME quantized EV the v1.1 claim used (the
        //    requested force's caps vs the objective's defense · value_e − travel), apply the EV-positive gate
        //    (EV > the commit threshold, the idle/Recycle alternative being 0), and field down the ranked list
        //    until the concurrency / forming caps are hit. This REPLACES the per-iteration greedy `best_by_ev`
        //    claim loop (deterministic: a stable sort over the Vec-ordered queue, integer EV, ties → smaller
        //    id). ──
        let ev_of_claim = |o: &super::objective_queue::CombatObjective| -> i64 {
            let room = o.kind.room();
            let caps = o.force.squads.first().map(|c| c.capabilities(claim_energy)).unwrap_or_default();
            let asset = data
                .mapping
                .get_room(&room)
                .and_then(|e| data.room_data.get(e))
                .and_then(|rd| game::rooms().get(rd.name))
                .map(|r| r.energy_capacity_available() as f32)
                .unwrap_or(1.0);
            let travel = claim_anchor.map(|h| room_distance(h, room)).unwrap_or(0);
            let econ = data.objective_queue.economic_intel(o.id);
            objective_ev_q(caps, &o.kind, o.priority, asset, claim_threat_for(room), econ, MAX_TRAVEL_BUDGET, travel)
        };
        let commit_threshold_q = quantize_ev(COMMIT_EV_THRESHOLD);
        // Rank the claimable (unclaimed, non-backoff, EV-positive) objectives by EV desc; tie → smaller id
        // (the same stable tie-break the kernel uses — ADR 0032 §Determinism). Vec-ordered, no HashMap.
        let mut ranked_claims: Vec<(ObjectiveId, i64)> = data
            .objective_queue
            .iter_objectives()
            .filter(|o| !data.objective_queue.is_claimed(o.id))
            .filter(|o| !data.objective_queue.is_unwinnable_now(o.kind.room(), now))
            .map(|o| (o.id, ev_of_claim(o)))
            .filter(|(_, ev_q)| *ev_q > commit_threshold_q)
            .collect();
        ranked_claims.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0 .0.cmp(&b.0 .0)));

        let mut claim_iter = ranked_claims.into_iter();
        while active < MAX_CONCURRENT_SQUADS && forming < MAX_FORMING_SQUADS {
            let Some((obj_id, _ev_q)) = claim_iter.next() else {
                break; // ran out of EV-positive claimable objectives
            };

            let (composition, target) = match data.objective_queue.get(obj_id) {
                Some(obj) => match obj.force.squads.first() {
                    Some(comp) => (comp.clone(), objective_target(&obj.kind)),
                    None => {
                        // Malformed objective (no force requested) — can't field it; skip to the next claim.
                        continue;
                    }
                },
                None => break,
            };

            // No in-range home can spawn this squad → don't claim it (a claimed-but-
            // never-spawned `SquadContext` would linger forever holding a cap slot).
            // Skip and try the next-best objective.
            if !homes.iter().any(|h| room_distance(h.name, target.1) <= MAX_SPAWN_DISTANCE) {
                if debug {
                    log::info!("[Lifecycle] SKIP obj={:?} room={} reason=no_home_in_range", obj_id, target.1);
                }
                continue;
            }

            if debug {
                log::info!("[Lifecycle] FIELD obj={:?} room={} members={}", obj_id, target.1, composition.member_count());
            }
            field_new_squad(&data.updater, &data.entities, &mut data.objective_queue, obj_id, &composition, target, now);
            active += 1;
            forming += 1; // the newly-claimed squad starts forming (slot 0 spawns next tick)
        }
    }
}

/// Delete a squad entity (retire). Orphaned members detach via the job fallback.
fn retire_squad(updater: &Read<LazyUpdate>, entities: &Entities, squad_entity: Entity) {
    if entities.is_alive(squad_entity) {
        updater.exec_mut(move |world| {
            if world.entities().is_alive(squad_entity) {
                let _ = world.delete_entity(squad_entity);
            }
        });
    }
}

/// ADR 0034 D4/D5/D8: drop the PER-MEMBER ephemeral travel trackers (rally-distance, target-distance, and
/// solo-stall) for one objective on retire/reassign. These are keyed by `(ObjectiveId, member-entity-id)`,
/// so a per-objective sweep retains only the entries for OTHER objectives. Ephemeral runtime state — no
/// serialized field, no `WORLD_FORMAT_VERSION` bump.
fn clear_member_trackers(fp: &mut SquadFormingProgress, obj_id: ObjectiveId) {
    fp.member_rally_dist.retain(|&(oid, _), _| oid != obj_id);
    fp.member_target_dist.retain(|&(oid, _), _| oid != obj_id);
    fp.member_solo_stall.retain(|&(oid, _), _| oid != obj_id);
}

/// Queue one slot's spawn to every in-range home room, sharing a token so exactly
/// one room fulfills it per tick.
#[allow(clippy::too_many_arguments)]
fn queue_slot_spawn(
    spawn_queue: &mut SpawnQueue,
    homes: &[HomeRoom],
    slot: &SquadSlot,
    slot_index: usize,
    target_room: RoomName,
    squad_entity: Entity,
    priority: f32,
    debug: bool,
) {
    // Size the member's body ONCE to the STRONGEST in-range home (capped by the body's
    // `maximum_repeat`) — the composition's intended size — NOT per-home. Per-home sizing let a cheaper
    // idle home win the shared-token spawn and field an UNDERSIZED creep (e.g. a 3-repeat SK duo too
    // weak to survive the keepers, even though the operation's affordability gate passed on the strong
    // home's capacity). Because the spawn system skips any home whose capacity is below the body cost
    // (`spawnsystem`: `body_cost > energy_capacity` → `continue`) and the shared token then routes the
    // spawn to an affording home, queuing the one intended-size body on every in-range home is correct —
    // no separate room-affordability filter is needed.
    let best_capacity = homes
        .iter()
        .filter(|h| room_distance(h.name, target_room) <= MAX_SPAWN_DISTANCE)
        .map(|h| h.energy_capacity)
        .max();
    let Some(best_capacity) = best_capacity else {
        return;
    };
    // Build via `build_body` so a force-SIZED slot (BodyType::Sized, R3) goes through the dynamic builder
    // and a template slot through create_body. CAP the build energy at PREFERRED_MEMBER_ENERGY: a force-
    // sized spec is already ≤ that ceiling (sized_for capped it), but a TEMPLATE fallback (a defense shape
    // when sized_for defers) would otherwise scale to the strongest in-range home and spawn a ~5000e blob
    // that never banks at HIGH priority while CRITICAL economy drains the home (the live W5N2/W4N7 defense
    // squads that re-queued forever). Capping keeps every spawned member bankable.
    let build_energy = best_capacity.min(screeps_combat_decision::composition::PREFERRED_MEMBER_ENERGY);
    let body = match slot.body_type.build_body(build_energy, screeps_combat_decision::bodies::MoveProfile::Plains) {
        Some(body) => body,
        // Even the strongest in-range home can't build it (template min OR the sized spec) — don't field
        // an undersized one. (A sized slot that doesn't fit was already vetoed upstream by sized_for.)
        None => {
            // This is a silent roster-stall point: the slot is NEVER queued, so the squad rallies forever
            // at present<full. Surface it so an over-sized per-member spec (or no strong-enough in-range
            // home) is diagnosable instead of invisible.
            if debug {
                log::warn!(
                    "[SpawnQueue] slot={} role={:?} target={} CANNOT BUILD: build_body None at best_cap={} (per-member spec exceeds the strongest IN-RANGE home, or >50 parts) — slot never queued, roster stalls here",
                    slot_index,
                    slot.role,
                    target_room,
                    best_capacity,
                );
            }
            return;
        }
    };

    // Observability: dump the ACTUAL body queued for this slot so we can confirm sizing live (e.g. is the
    // whole force piled onto one member, vs split across members). Behind features.military.debug_log.
    if debug {
        let n = |p: Part| body.iter().filter(|b| **b == p).count();
        let cost: u32 = body.iter().map(|p| p.cost()).sum();
        let in_range = homes.iter().filter(|h| room_distance(h.name, target_room) <= MAX_SPAWN_DISTANCE).count();
        log::info!(
            "[SpawnQueue] slot={} role={:?} target={} parts={} (rng={} heal={} atk={} work={} tough={} carry={} move={}) cost={} prio={} homes_in_range={} (best_cap={})",
            slot_index,
            slot.role,
            target_room,
            body.len(),
            n(Part::RangedAttack),
            n(Part::Heal),
            n(Part::Attack),
            n(Part::Work),
            n(Part::Tough),
            n(Part::Carry),
            n(Part::Move),
            cost,
            priority,
            in_range,
            best_capacity,
        );
    }

    let token = spawn_queue.token();
    for home in homes.iter().filter(|h| room_distance(h.name, target_room) <= MAX_SPAWN_DISTANCE) {
        let request = SpawnRequest::new(
            format!("Squad-{:?} {}", slot.role, target_room),
            &body,
            priority,
            Some(token),
            create_spawn_callback(slot.role, slot_index, target_room, squad_entity),
        );
        spawn_queue.request(home.entity, request);
    }
}

/// Mint a `SquadContext` bound to the objective and claim it. Members spawn next
/// tick once the lazily-created component exists (the AttackMission create-then-
/// wait discipline).
fn field_new_squad(
    updater: &Read<LazyUpdate>,
    entities: &Entities,
    queue: &mut CombatObjectiveQueue,
    obj_id: ObjectiveId,
    composition: &SquadComposition,
    target: (SquadTarget, RoomName),
    now: u32,
) {
    let mut ctx = SquadContext::from_composition(composition);
    ctx.objective_id = Some(obj_id);
    ctx.target = Some(target.0);

    let squad_entity = updater
        .create_entity(entities)
        .with(ctx)
        .marked::<SerializeMarker>()
        .build();

    queue.claim(obj_id, squad_entity);
    // P-OBJ #23: open the commitment lease so the objective outlives producer silence on stale intel
    // while this squad forms + travels (the manager refreshes it each tick the squad has a focus).
    queue.set_deadline(obj_id, Some(now + COMMITMENT_BUDGET));
}

/// Map the live squad state to the pure decision's combat-state subset.
fn squad_state_to_order(state: SquadState) -> SquadOrderState {
    match state {
        SquadState::Forming | SquadState::Rallying => SquadOrderState::Forming,
        SquadState::Moving => SquadOrderState::Moving,
        SquadState::Engaged => SquadOrderState::Engaged,
        SquadState::Retreating => SquadOrderState::Retreating,
        SquadState::Complete => SquadOrderState::Moving,
    }
}

/// Map the pure decision's combat state back to the live squad state.
fn order_state_to_squad(state: SquadOrderState) -> SquadState {
    match state {
        SquadOrderState::Forming => SquadState::Forming,
        SquadOrderState::Moving => SquadState::Moving,
        SquadOrderState::Engaged => SquadState::Engaged,
        SquadOrderState::Retreating => SquadState::Retreating,
    }
}

/// Where a room's combat DTOs came from — i.e. how TRUSTWORTHY "empty hostiles" is. The single source of
/// truth for intel reliability (returned by [`build_room_combat_dtos`] so callers never re-derive the
/// branch condition and risk drift). `Cached`/`LiveVisible` ⇒ reliable (empty means genuinely clear);
/// `None` ⇒ unreliable (empty means merely UNSEEN — never trust no-vision emptiness).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CombatIntelSource {
    /// Scouted `RoomData` ECS entity — cached last-scouted intel, reliable even without current live vision.
    Cached,
    /// No mapping entity, but the room is live-visible this tick (`game::rooms().get` is Some).
    LiveVisible,
    /// Neither — genuinely no information about the room (empty DTOs are merely UNSEEN, not clear).
    None,
}

impl CombatIntelSource {
    /// Whether the DTOs are TRUSTWORTHY: empty hostiles/towers genuinely mean clear, not merely unseen.
    fn is_reliable(self) -> bool {
        matches!(self, CombatIntelSource::Cached | CombatIntelSource::LiveVisible)
    }
}

/// Read a room's hostiles + structures into JS-free combat DTOs (the live adapter leaf;
/// the shared `squad_combat` adapters preserve ordering so the decision's tie-breaks match).
///
/// Also returns the [`CombatIntelSource`] (the DTO provenance) as the SINGLE SOURCE OF TRUTH for intel
/// reliability — callers (e.g. the rally-stall `target_is_uncontested` gate) read `source.is_reliable()`
/// instead of independently recomputing the cached/live branch condition (which would risk drift, and —
/// the rally-oscillation bug — flapped as a solo member crossed a room boundary toggling raw live vision).
fn build_room_combat_dtos(
    room_data: &ReadStorage<RoomData>,
    mapping: &EntityMappingData,
    room: RoomName,
) -> (Vec<CombatCreepDto>, Vec<CombatStructureDto>, CombatIntelSource) {
    // The cached path: the room has a RoomData ECS entity (registered in the mapping). `get_creeps`/
    // `get_structures` self-refresh from `game::rooms()` when stale, so this returns the live state.
    // Cached intel persists even when the room is not CURRENTLY live-visible — RELIABLE, and stable as a
    // member crosses the room boundary (the rally-oscillation fix relies on this stability).
    if let Some(rd) = mapping.get_room(&room).and_then(|e| room_data.get(e)) {
        let hostiles = rd
            .get_creeps()
            .map(|c| c.hostile().iter().map(creep_to_dto).collect())
            .unwrap_or_default();
        let structures = rd
            .get_structures()
            .map(|s| s.all().iter().map(structure_to_dto).collect())
            .unwrap_or_default();
        return (hostiles, structures, CombatIntelSource::Cached);
    }

    // FOCUS-ON-ARRIVAL FIX (Break #2 arrival half): the squad has just ENTERED `room`, so the room IS
    // visible (a member stands in it), but the mapping has not yet registered its RoomData entity this tick
    // (`mapping.get_room` is None on the arrival tick — the visibility/mapping timing hole). The pre-fix
    // path returned EMPTY DTOs → `decide_squad` found no focus → the squad logged IN_ROOM_NO_FOCUS and sat
    // until the lease lapsed (it never engaged, never razed the core — THE deep no-engage bug). Force a
    // direct LIVE re-read from `game::rooms()` so a focus is computed on the arrival tick. Inert when the
    // room is genuinely not visible (we have no vision — keep the empty result, the squad keeps closing).
    if let Some(live) = game::rooms().get(room) {
        let hostiles = live
            .find(find::HOSTILE_CREEPS, None)
            .iter()
            .map(creep_to_dto)
            .collect();
        let structures = live.find(find::STRUCTURES, None).iter().map(structure_to_dto).collect();
        return (hostiles, structures, CombatIntelSource::LiveVisible);
    }

    (Vec::new(), Vec::new(), CombatIntelSource::None)
}

/// ADR 0024 Stage 1 (live mirror of `screeps_combat_agent::pathing`): scales the [`ThreatField`]'s
/// raw incoming hits/tick into a small ADDITIVE per-tile traversal penalty — `add = (raw / DIV) cap
/// CAP` — kept tiny + HARD-CAPPED so a threatened tile is *preferred against* but always cheaply
/// PASSABLE (never impassable): a fully-threatened approach must stay traversable or the squad can
/// never close. Seed values; the EXP-*/`SquadTacticParams` sweep is the sanctioned tuner.
const THREAT_PATH_DIV: i32 = 150;
const THREAT_PATH_CAP: i32 = 8;

/// Build a room's movement cost matrix with terrain walls overlaid (the headless `LocalPathfinder`
/// reads walls from the matrix, so the `Terrain::Wall` overlay is mandatory). Extracted so the
/// per-room `PositionLayers` cache (build-once-per-room) and the kite search share one matrix build.
///
/// When `threat` is `Some`, the field is folded into the traversal cost (ADR 0024 Stage 1, "the
/// safest route") so live paths route AROUND tower/enemy kill-zones — the penalty is added ON TOP of
/// the live matrix (preserving road discounts / structure costs), skips impassable tiles, and clamps
/// below `u8::MAX` so no tile is ever sealed. Inert (byte-identical) when there are no threats.
fn build_target_matrix(
    cms: &mut CostMatrixSystem,
    opts: &CostMatrixOptions,
    room: RoomName,
    threat: Option<&ThreatField>,
) -> Option<LocalCostMatrix> {
    let mut matrix = cms.build_local_cost_matrix(room, opts).ok()?;
    if let Some(terrain) = game::map::get_room_terrain(room) {
        for x in 0..50u8 {
            for y in 0..50u8 {
                if terrain.get(x, y) == Terrain::Wall {
                    if let Ok(xy) = RoomXY::checked_new(x, y) {
                        matrix.set(xy, u8::MAX);
                    }
                }
            }
        }
    }
    if let Some(tf) = threat {
        for x in 0..50u8 {
            for y in 0..50u8 {
                let xy = match RoomXY::checked_new(x, y) {
                    Ok(xy) => xy,
                    Err(_) => continue,
                };
                let cur = matrix.get(xy);
                if cur == u8::MAX {
                    continue; // wall / impassable structure — never weaken it
                }
                let raw = tf.raw_at(Position::new(xy.x, xy.y, room));
                if raw <= 0 {
                    continue;
                }
                let add = (raw / THREAT_PATH_DIV).min(THREAT_PATH_CAP);
                if add > 0 {
                    matrix.set(xy, (cur as i32 + add).min(254) as u8);
                }
            }
        }
    }
    Some(matrix)
}

/// Build the squad view, run the pure `decide_squad`, and apply the result to the `SquadContext`
/// (state + per-member orders). The live adapter for P2.G3 tactics. (Many args: distinct ECS borrows
/// that can't be cheaply bundled — the live adapter shim, like the haul builders.)
#[allow(clippy::too_many_arguments)]
fn compute_squad_orders(
    room_data: &ReadStorage<RoomData>,
    mapping: &EntityMappingData,
    squad_contexts: &mut WriteStorage<SquadContext>,
    creep_owner: &ReadStorage<CreepOwner>,
    squad_entity: Entity,
    obj_id: ObjectiveId,
    target_room: RoomName,
    formation: bool,
    assault_mode: Option<screeps_combat_decision::force_sizing::AssaultMode>,
    room_layers: &mut HashMap<RoomName, (LocalCostMatrix, PositionLayers)>,
    debug: bool,
    requested_slots: usize,
    now: u32,
    deadline: Option<u32>,
    forming_progress: &mut SquadFormingProgress,
) {
    // Read the roster's cached status (immutable). `pos`/`has_ranged` feed the centroid + the kite
    // plan; `has_ranged` resolves the creep body (the adapter's job — the pure crate stays JS-free).
    let (member_views, current_state, retreat_threshold) = match squad_contexts.get(squad_entity) {
        Some(ctx) => (
            ctx.members
                .iter()
                .map(|m| {
                    // Resolve the body ONCE for has_ranged + per-tick outputs (the engage DMG reward's
                    // melee/ranged power, ADR 0019; + ADR 0025 dismantle/claim caps) and the creep id (so
                    // the EV kernel's heal intent can target this ally).
                    let (id, has_ranged, melee_power, ranged_power, dismantle_power, claim_power) = creep_owner
                        .get(m.entity)
                        .and_then(|co| co.owner.resolve())
                        .map(|c| {
                            let (mut atk, mut rng, mut work, mut claim) = (0u32, 0u32, 0u32, 0u32);
                            for p in c.body().iter().filter(|p| p.hits() > 0) {
                                match p.part() {
                                    Part::Attack => atk += 1,
                                    Part::RangedAttack => rng += 1,
                                    Part::Work => work += 1,
                                    Part::Claim => claim += 1,
                                    _ => {}
                                }
                            }
                            (
                                c.try_raw_id(),
                                rng > 0,
                                atk * screeps::constants::ATTACK_POWER,
                                rng * screeps::constants::RANGED_ATTACK_POWER,
                                work * screeps::constants::DISMANTLE_POWER,
                                claim * 300, // CONTROLLER_ATTACK_PER_PART (engine const; declaim is deferred in v1)
                            )
                        })
                        .unwrap_or((None, false, 0, 0, 0, 0));
                    SquadMemberView {
                        hits: m.current_hits,
                        hits_max: m.max_hits,
                        heal_power: m.heal_power,
                        pos: m.position,
                        has_ranged,
                        melee_power,
                        ranged_power,
                        damage_taken_last_tick: m.damage_taken_last_tick,
                        id,
                        dismantle_power,
                        claim_power,
                    }
                })
                .collect::<Vec<_>>(),
            squad_state_to_order(ctx.state),
            ctx.retreat_threshold,
        ),
        None => return,
    };
    if member_views.is_empty() {
        return;
    }

    // INTROSPECTION + INTEL-RELIABILITY: `build_room_combat_dtos` reports its DTO provenance as the SINGLE
    // SOURCE OF TRUTH (no re-derivation of the cached/live branch condition — that drift was the rally
    // oscillation: a separate `game::rooms().get().is_some()` flapped as a member crossed the boundary).
    // `Cached` = the scouted RoomData path; `LiveVisible` = the on-arrival `game::rooms()` fallback (Break #2
    // arrival half — the room is visible but its RoomData entity is not yet mapped this tick).
    let (hostiles, structures, intel_source) = build_room_combat_dtos(room_data, mapping, target_room);
    let dto_from_live_fallback = intel_source == CombatIntelSource::LiveVisible;

    // Enemy safe mode → all our combat in the room is nullified (engage-veto, ADR 0020 §8). Only known
    // when the room is visible; default false otherwise (we discover + retreat on arrival).
    let enemy_safe_mode = game::rooms()
        .get(target_room)
        .and_then(|r| r.controller())
        .map(|c| !c.my() && c.safe_mode().unwrap_or(0) > 0)
        .unwrap_or(false);

    let view = SquadView {
        members: &member_views,
        hostiles: &hostiles,
        structures: &structures,
        retreat_threshold,
        current_state,
        enemy_safe_mode,
        // Offense closes in and finishes the enemy (the close-to-kill gradient is now live). `Hold` is
        // for a future pin/harass objective. enemy_stalled stays false until SquadContext tracks the
        // no-progress counter (a fast-follow; the sim already validates the stalemate-disengage path).
        engage_objective: screeps_combat_decision::EngageObjective::Destroy,
        enemy_stalled: false,
        // ADR 0031 #39 P3: the drain stance is now THREADED from the oracle. `Some(Drain)` (the war producer
        // ran `plan_engagement` and picked the tower-drain for this objective) → `drain_stance = true`, so the
        // winnability path treats the FINITE towers as drainable (not a permanent unwinnable blocker) WHILE the
        // drain sustains, holds the falloff standoff, then advances once they're dry. `None`/`Some(Breach)` →
        // `false` → the byte-unchanged direct breach/engage path (the non-drain behavior is preserved exactly).
        drain_stance: matches!(assault_mode, Some(screeps_combat_decision::force_sizing::AssaultMode::Drain)),
    };

    // Build the target room's movement cost matrix (terrain walls baked in — the headless
    // `LocalPathfinder` reads walls from the matrix) plus the per-room `PositionLayers` (threat
    // field + reachability flood) ONCE per room and share across every squad targeting it — the
    // threat field and floods depend only on the room's enemies, not on which squad is asking
    // (ADR 0019 Stage 3b build-once-per-room). Same matrix recipe the squad anchor mover uses
    // (formation.rs); the search itself is the pure `LocalPathfinder`.
    if let std::collections::hash_map::Entry::Vacant(slot) = room_layers.entry(target_room) {
        let mut cache = CostMatrixCache::default();
        let mut cms = CostMatrixSystem::new(&mut cache, Box::new(screeps_rover::screeps_impl::ScreepsCostMatrixDataSource));
        let opts = CostMatrixOptions::default();
        // ADR 0024 Stage 1: the same field `build_room_layers` prices, folded into the movement matrix
        // so the kite/strategic path routes around exposure (the layers' own threat field is rebuilt
        // internally — identical inputs).
        let threat = build_room_threat_field(&hostiles, &structures);
        if let Some(matrix) = build_target_matrix(&mut cms, &opts, target_room, Some(&threat)) {
            let layers = build_room_layers(&hostiles, &structures, target_room, &matrix, MAX_KITE_OPS);
            slot.insert((matrix, layers));
        }
    }

    // ADR 0026 — pick the weight profile by objective class + room information (instead of one fixed
    // default). StructureBreach = an explicit dismantle objective OR a room whose only remaining hostiles
    // are structures (creeps cleared → switch to breaching the ring); everything else is open-creep
    // combat. Keys on `enemy_safe_mode` (the safe-mode veto) AND `assault_mode` (ADR 0031 #39 P3 — the
    // oracle's chosen mode the war producer attached): `Some(Drain)` → the `DrainBreach` strategy (the
    // patient drain profile that holds the standoff through the soak); `None`/`Some(Breach)` → the straight
    // breach (byte-unchanged). The strategy registry keys only on `Drain`, so `Breach` is inert here.
    let class = classify_objective(formation, !structures.is_empty(), !hostiles.is_empty());
    let strat_ctx = StrategyContext { class, info: StrategyInfo { enemy_safe_mode, assault_mode } };
    let tactics = decide_strategy(&strat_ctx, &default_strategies());

    let decision = match room_layers.get(&target_room) {
        Some((matrix, layers)) => {
            let mut room_cb = |_r: RoomName| Some(matrix.clone());
            decide_squad_with_pathing(&view, Some(layers), tactics, &mut room_cb, MAX_KITE_OPS)
        }
        None => {
            let mut room_cb = |_r: RoomName| None;
            decide_squad_with_pathing(&view, None, tactics, &mut room_cb, MAX_KITE_OPS)
        }
    };

    // Travel cohesion (P2.G4-O1): while the squad is still converging on the target room, the manager
    // advances the squad's footprint anchor toward the room centre — the rover `AnchorPath` via
    // `advance_squad_virtual_position` (cached, footprint-aware, holds-on-blocked). The job's
    // `MoveToRoom` reads `virtual_pos` and issues each member's `move_to` (§5 separation: the manager
    // decides the squad frame, the job owns movement issuance). Once every member has ARRIVED we drop
    // the anchor so the `Engaged` state kites via the pure `decide_movement` rather than
    // formation-follow — keeping G3 kiting intact; engaged formation/orientation is the separate O2.
    // This stops a squad from trickling into a contested room one creep at a time.
    let all_arrived = member_views
        .iter()
        .all(|m| m.pos.map(|p| p.room_name() == target_room).unwrap_or(false));
    // FIX B1 (engaged-en-route latch): whether ANY living member stands in the target room. The
    // `engaged_once` latch is gated on this so a squad whose VISIBLE target room has a hostile while it is
    // still TRAVELING (a proximity-free focus, no member in-room) does NOT latch engaged_once en route —
    // which would permanently kill its travel lease (`traveling` requires `!engaged_once`) and freeze it
    // mid-hop. Latch only once a member is actually IN the room (decide_squad still picks the focus per-tick;
    // only the PERMANENT latch is gated). Uncontested clears still latch on arrival — unchanged.
    let in_room_any = member_views
        .iter()
        .any(|m| m.pos.map(|p| p.room_name() == target_room).unwrap_or(false));

    // P-OBJ #23 killer diagnostic: the squad is fully in the target room but `decide_squad` found NOTHING
    // to attack. This one line classifies the live no-engage failure: hostiles=0 structs=0 => empty room
    // DTOs (visibility/mapping timing); structs>=1 focus=None => structure-focus selection bug;
    // safe_mode=true => correct veto. Repeats while stalled, which itself confirms a persistent stall.
    if debug && all_arrived && decision.focus.is_none() {
        log::info!(
            "[Lifecycle] IN_ROOM_NO_FOCUS squad={:?} room={} hostiles={} structs={} state={:?} safe_mode={} formation={}",
            squad_entity, target_room, hostiles.len(), structures.len(), current_state, enemy_safe_mode, formation
        );
    }

    // P-OBJ #23 RALLY-until-full gate (operator: wait + group up until the squad is ready, THEN go in
    // together). The full roster must be spawned AND present in the world before the squad leaves home —
    // otherwise the lone slot-0 lead departs alone, can't solo the objective, dies, and the squad wipes →
    // re-field → slot-0 forever (the actual invader no-engage root cause). Measured against the objective's
    // requested slot count so a death-degraded layout can't shrink "full".
    let member_positions: Vec<Option<Position>> = member_views.iter().map(|m| m.pos).collect();
    // ADR 0034 D5/D8: the member ENTITIES parallel to `member_views`/`member_positions` (same iteration
    // order — both derived from `ctx.members`), so the per-member rally-progress + solo-stall trackers can be
    // keyed by a STABLE id (the entity), not the volatile slice index. Captured here once.
    let member_entities: Vec<Entity> = squad_contexts
        .get(squad_entity)
        .map(|ctx| ctx.members.iter().map(|m| m.entity).collect())
        .unwrap_or_default();
    // Rally/deploy gate (FIX 1 — the rally-stall fix). A DEFENDED or UNKNOWN target keeps the hard full-roster
    // `squad_ready_to_depart`: the oracle sized it to be Lanchester-favorable, so the full roster is winnable
    // BY CONSTRUCTION and must enter together or the trickle is picked off. BUT a PROVEN-uncontested target —
    // a room we have TRUSTWORTHY intel for with no hostiles, no hostile towers, and no enemy safe mode — does
    // not need the last member (which can lose the within-tier spawn race on a young colony and deadlock the
    // all-or-nothing gate forever, the live W7N7 stall). An oversized force advancing + dismantling an
    // undefended core as members arrive is harmless, so deploy at the min-viable quorum.
    //
    // RALLY-OSCILLATION FIX: feed INTEL-RELIABILITY, not raw live vision. The pre-fix code passed
    // `room_visible = game::rooms().get(target_room).is_some()` — raw CURRENT live vision, which FLAPS as a
    // solo squad's member crosses the W6N5↔W7N5 boundary → `uncontested` flaps → `shared_rally_point` flips
    // the rally ROOM between the target and one-room-short → the squad chases a moving rally (a feedback loop:
    // rally depends on the squad's own vision, which depends on its position, which depends on the rally). We
    // now pass `intel_source.is_reliable()` (Cached OR LiveVisible). A MAPPED offense target (an assault
    // objective is ALWAYS mapped — it came from the war.rs offense scan over scouted threat rooms) has STABLE
    // reliable cached intel, so `uncontested` is stable as a member crosses the boundary — the loop is broken.
    // Still LOAD-BEARING for the trickle-guard: a GENUINELY-UNKNOWN room (source `None`: unmapped AND no live
    // vision) is NOT reliable → NOT uncontested → keep the hard full-roster rally (never trust no-vision
    // emptiness). The fix ONLY relaxes the requirement from CURRENT live vision to RELIABLE intel (cache counts).
    // ADR 0035 D3 (the C7 fix — RC-11 parity). The pre-fix uncontested classifier passed
    // `intel_source.is_reliable()` (Cached || LiveVisible) as the intel arg. But an empty-CACHED towered
    // room is RELIABLE-yet-VACUOUS: `is_reliable()=true` while the cache shows no towers because none were
    // VISIBLE last scout, not because there are none — so `uncontested` flipped true, `shared_rally_point`
    // staged AT the target centre, and the squad walked into the towers (the live W4N5 reach↔retreat
    // spiral). D9 already gated the win-or-stall FAST-PATH on `== LiveVisible` (deliberately NOT
    // `is_reliable()`), but the uncontested classifier on the SAME path still trusted `is_reliable()` — the
    // two intel predicates disagreed about what "real intel" means. Fix: feed the uncontested classifier the
    // SAME real-intel notion as the fast-path (`have_target_intel`, computed below) — a non-empty DTO set
    // (we actually SEE a hostile/structure) OR an on-arrival LIVE read. An empty-Cached towered room then
    // classifies CONTESTED → the rally stages ONE ROOM SHORT (out of tower range) → the squad masses + only
    // advances on the gather quorum, instead of trickling into tower range. A LEGITIMATE LiveVisible-empty
    // room (a member stands in it and SEES it clear) still classifies uncontested. `rally_intel_reliable`
    // (`is_reliable()`) is RETAINED for its legacy boundary-oscillation concern but is no longer the gate the
    // uncontested classifier reads — the two were conflated; this decouples them. Pure per-tick recompute of
    // the ephemeral DTOs + the existing `intel_source` — no serialized state, no WORLD_FORMAT_VERSION bump.
    let uncontested_intel =
        !hostiles.is_empty() || !structures.is_empty() || intel_source == CombatIntelSource::LiveVisible;
    let no_hostile_towers = !structures
        .iter()
        .any(|s| s.structure_type == StructureType::Tower && s.ownership == screeps_combat_decision::Ownership::Hostile);
    let uncontested = crate::military::formation::target_is_uncontested(
        uncontested_intel,
        hostiles.is_empty(),
        no_hostile_towers,
        !enemy_safe_mode,
    );
    // REACH BUG #2 — the PROCEED gate is Lanchester P(win)-driven (win-or-stall), NOT composition-
    // completeness (operator: combat-ev-economic-and-pwin-gating). The composition COUNT gate below
    // (`ready_to_depart_gate`) still SIZES the spawn and is the legacy/uncontested proceed path. But the
    // PRIMARY proceed decision is now: would the CURRENT PRESENT force WIN OR STALL (won't lose) against the
    // target's defense? If so, holding for the full roster is pointless — DEPLOY even with incomplete
    // archetypes. We reuse `present_force_wins_or_stalls`, which is the EXACT inverse of the present-force
    // RETREAT condition `decide_squad` uses (same `assess_engage` Lanchester model, same `ENGAGE_BALANCE_BAND`)
    // — so the proceed gate and the retreat gate can never disagree about what "losing" means. A force that
    // would LOSE does NOT proceed (no trickle-to-death: a losing present force keeps holding for more roster
    // via the count gate, and `present_force_wins_or_stalls` requires `our_strength > 0` so a zero-fighting
    // roster never deploys into a defended room). The view/centroid here are the SAME ones `decide_squad`
    // assessed this tick.
    let present_wins_or_stalls = screeps_combat_decision::present_force_wins_or_stalls(&view, decision.center);
    // RC-11 — the INTEL GATE on the win-or-stall fast-path. `present_force_wins_or_stalls` returns TRUE
    // VACUOUSLY against an UNSCOUTED target room: empty hostiles + empty structures (source `None`) give
    // `assess_engage` killable_dps=0/tower_dps=0 → unwinnable=false, enemy_strength~0, our_strength>0 →
    // the balance clamps to +1000 → "we win" — a win against ZERO VISIBLE enemies that may not be real. If
    // that fast-path fires while members are still rooms apart it latches the assault, anchors the cross-room
    // box formation on the first living member's room, and FREEZES the scattered members at static positions
    // (the live freeze-before-reaching bug). So gate the fast-path on REAL target intel: a non-empty DTO set
    // (we actually SEE a hostile/structure) OR an on-arrival live read (`LiveVisible`, a member stands in the
    // room). An empty `Cached`/`None` set does NOT satisfy it — the squad falls back to the gather-quorum
    // COUNT gate (members MASS at the rally via solo-travel BEFORE any formation assault), and the fast-path
    // re-enables the instant real DTOs arrive (room visible/cached non-empty). This PRESERVES the P(win)
    // win-or-stall for REAL-intel targets (operator directive, D7) but stops it firing on vacuous no-intel
    // wins. Pure read of the ephemeral DTOs + the existing `intel_source` — no serialized state, no WFV bump.
    // ADR 0035 D3: this is the SAME real-intel predicate the uncontested classifier now reads
    // (`uncontested_intel`, above) — ONE source of truth for "real intel" on this path (the C7 inconsistency
    // between the fast-path gate and the uncontested classifier is closed; they can no longer disagree).
    let have_target_intel = uncontested_intel;
    let fast_path_allowed = screeps_combat_decision::winnable_fast_path_allowed(present_wins_or_stalls, have_target_intel);
    let ready_to_depart = fast_path_allowed
        || crate::military::formation::ready_to_depart_gate(&member_positions, requested_slots, uncontested);

    if let Some(ctx) = squad_contexts.get_mut(squad_entity) {
        if !ready_to_depart {
            // RALLY/FORMING: hold at home and group up while the roster spawns. With MULTI-HOME SPAWN the
            // members are at DIFFERENT homes; a cross-room formation march toward one home would re-introduce
            // the very frozen-anchor stall this fix removes (and needlessly pull a member off its own spawn,
            // where the renew pass keeps it alive). So drop the formation anchor and issue NO travel order —
            // each freshly-spawned member simply HOLDS next to its own home spawn (renewable) until the rally
            // gate releases, at which point the SOLO-travel-to-shared-rally phase (below) takes over.
            ctx.squad_path = None;
            for member in ctx.members.iter_mut() {
                member.tick_orders = Some(TickOrders { movement: TickMovement::Hold, ..Default::default() });
            }
            if debug {
                log::info!(
                    "[Lifecycle] RALLY squad={:?} room={} present={}/{} uncontested={} (holding home until {})",
                    squad_entity, target_room, member_positions.iter().filter(|p| p.is_some()).count(),
                    requested_slots, uncontested, if uncontested { "quorum" } else { "full roster" }
                );
            }
        } else if !all_arrived {
            // ── MOVEMENT-STALL FIX (ADR 0028 K0): SOLO travel to a SHARED rally, THEN assault in formation.
            //
            // The squad spawned from MANY homes (multi-home spawn preserved) so its members are rooms apart.
            // Crossing as a cross-room box FORMATION freezes the anchor for scattered members (no member ever
            // meets the boundary cohesion quorum → virtual_pos stalls → each per-creep move becomes a
            // self-target no-op → the live "milling at home, fatigue=0, d=(stalled)" bug). So DECOUPLE travel
            // from formation:
            //   1. SOLO TRAVEL — each member paths INDIVIDUALLY to ONE shared rally point near the target
            //      (no box cohesion during transit; the robust fix that sidesteps the frozen anchor). The
            //      shared rally is derived deterministically each tick (no stored field → no WFV bump).
            //   2. GATHER QUORUM — once enough living members have converged at the shared rally (the UNIFIED
            //      `rally::gather_quorum_met` kernel the sim also calls), transition to the assault.
            //   3. ASSAULT — advance the box-formation anchor rally→target on the short final leg (cohesion
            //      applies HERE, where the members are already massed). This is where the anchor box belongs.
            // The assault target: a focus if we already see one, else the target-room centre.
            let assault_target = decision
                .focus
                .map(|f| f.pos)
                .unwrap_or_else(|| Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), target_room));
            // ADR 0034 D2/D3 (RC-2): derive the rally from the SCATTER-ROBUST kernel over the member
            // positions — for a far/cross-quadrant scatter it biases the staging room onto the FURTHEST
            // member's approach corridor and validates placement (on the approach line, strictly closer
            // to the target than the laggard, a real room), instead of feeding the raw (D1 world-coord)
            // centroid as the approach. For a same-room/tight squad it is byte-identical to the legacy
            // `shared_rally_point`. The centroid still feeds the engage/win-or-stall frame as
            // `decision.center` (computed once in `decide_squad`).
            let rally =
                screeps_combat_decision::rally::shared_rally_point_for_members(&member_positions, assault_target, uncontested);

            // ── ADR 0034 D4 + D8 (RC-3/RC-8 — member-side movement-failure escalation, NO silent retry) ──
            // Track each present member's room-distance to the rally; a member whose distance does NOT
            // decrease this tick made no solo-travel progress (blocked / NO_PATH / stuck behind impassable
            // terrain or a hostile room — the `MoveToRoom::move_to(rally)` the bot silently re-issued every
            // tick, RC-3). Its per-member STALL counter increments; a member that closes resets it. Past the
            // tighter `SOLO_TRAVEL_STALL_WINDOW` (D8 — well before the coarse `MAX_TRAVEL_BUDGET`) the manager
            // RE-ASSESSES it OUT of the gather quorum (D4): it proceeds with the REACHABLE subset rather than
            // waiting forever on a member that cannot path to the rally. Scoped to a CONTESTED travel (the
            // uncontested gate already trickles at quorum 1); only fires when MORE THAN ONE member is present
            // (a lone member can't be "left behind"); and never excludes the LAST reachable member (always
            // keep >=1). All ephemeral per-member trackers (NO serialized field → no WORLD_FORMAT_VERSION bump).
            let mut excluded_member: Vec<bool> = vec![false; member_views.len()];
            if !uncontested {
                for (i, m) in member_views.iter().enumerate() {
                    let Some(pos) = m.pos else {
                        continue; // unspawned — no body to stall
                    };
                    let Some(&ent) = member_entities.get(i) else {
                        continue;
                    };
                    let key = (obj_id, ent.id());
                    let cur = room_distance(pos.room_name(), rally.room_name());
                    let at_rally = pos.get_range_to(rally) <= screeps_combat_decision::rally::RALLY_GATHER_RADIUS;
                    let prev = forming_progress.member_rally_dist.get(&key).copied();
                    let closing = at_rally || matches!(prev, Some(p) if cur < p) || prev.is_none();
                    forming_progress.member_rally_dist.insert(key, cur);
                    if closing {
                        forming_progress.member_solo_stall.remove(&key);
                    } else {
                        let s = forming_progress.member_solo_stall.entry(key).or_insert(0);
                        *s = s.saturating_add(1);
                    }
                }
                // Decide exclusions: members past the stall window. Keep at least ONE reachable member.
                let present_now = member_views.iter().filter(|m| m.pos.is_some()).count();
                if present_now > 1 {
                    let mut stalled: Vec<usize> = (0..member_views.len())
                        .filter(|&i| {
                            member_views[i].pos.is_some()
                                && member_entities
                                    .get(i)
                                    .and_then(|e| forming_progress.member_solo_stall.get(&(obj_id, e.id())))
                                    .is_some_and(|&s| s >= SOLO_TRAVEL_STALL_WINDOW)
                        })
                        .collect();
                    // Never strand the whole squad: leave at least one present member reachable.
                    if stalled.len() >= present_now {
                        stalled.truncate(present_now - 1);
                    }
                    for i in stalled {
                        excluded_member[i] = true;
                        if debug {
                            log::info!(
                                "[Lifecycle] ESCALATE-BLOCK squad={:?} obj={:?} member={:?} stalled>={} → re-assessed OUT of the gather quorum (reachable subset proceeds)",
                                squad_entity, obj_id, member_entities.get(i), SOLO_TRAVEL_STALL_WINDOW
                            );
                        }
                    }
                }
            }

            // Has a FIGHTER gathered at the rally OR already in the target room? (No healer-only assault.) A
            // fighter has melee or ranged. FIX A counts an in-target-room fighter as "gathered" so an
            // arrived member never fails the gather (the uncontested gathered>=1 / members-already-arrived
            // path) — a defender whose lead is already in the room keeps committing.
            let fighter_gathered = member_views.iter().any(|m| {
                m.pos
                    .map(|p| {
                        (p.get_range_to(rally) <= screeps_combat_decision::rally::RALLY_GATHER_RADIUS || p.room_name() == target_room)
                            && (m.has_ranged || m.melee_power > 0)
                    })
                    .unwrap_or(false)
            });
            // FIX A: members already IN the target room count as gathered (modeled as gathered-at-rally) so
            // arrived members can't fail the quorum. ADR 0034 D4: a RE-ASSESSED-OUT (excluded) member is
            // DROPPED from the gather positions — the squad no longer waits on it.
            let mut gather_positions: Vec<Option<Position>> = Vec::with_capacity(member_views.len());
            for (i, m) in member_views.iter().enumerate() {
                if excluded_member[i] {
                    continue; // re-assessed out (blocked past the stall window) — not in the quorum
                }
                if m.pos.map(|p| p.room_name() == target_room).unwrap_or(false) {
                    gather_positions.push(Some(rally)); // an in-room member is counted at the rally
                } else {
                    gather_positions.push(m.pos);
                }
            }
            // ADR 0034 D4: the gather denominator drops the re-assessed-out members too, so the contested
            // quorum is measured against who can ACTUALLY mass (the reachable subset), not the full roster a
            // blocked member would otherwise pin un-fillable forever (RC-10).
            let excluded_count = excluded_member.iter().filter(|e| **e).count();
            let effective_slots = requested_slots.saturating_sub(excluded_count);
            // REACH BUG #2 — the gather→assault transition is ALSO P(win)-driven: if the PRESENT (gathered)
            // force already WINS-OR-STALLS against the target, advance the assault without waiting for the
            // near-full roster to mass at the rally (the contested `gather_quorum_met` count quorum). The
            // count quorum stays as the legacy/under-strength path (a force that does NOT yet win-or-stall
            // still masses before committing — no trickle-to-death). Same win-or-stall predicate as the
            // proceed gate above, so the two cohesion gates agree.
            let count_quorum_met = screeps_combat_decision::rally::gather_quorum_met(
                &gather_positions,
                rally,
                effective_slots,
                uncontested,
                fighter_gathered,
                screeps_combat_decision::rally::RALLY_GATHER_RADIUS,
            );
            let quorum_now = fast_path_allowed || count_quorum_met;
            // FIX A (assault latch): once the gather quorum FIRST fires, LATCH the assault and thereafter take
            // the assault branch WITHOUT re-evaluating the quorum — so members dying/lagging crossing
            // enemy-held neighbours can't un-commit it (the contested in_room<->travel oscillation, BUG A).
            // The latch is an ephemeral per-objective flag (no WORLD_FORMAT_VERSION bump); on a VM reload the
            // squad re-derives the quorum from live positions (a massed bloc re-latches immediately).
            if quorum_now {
                forming_progress.assault_latched.insert(obj_id);
            }
            // RC-11 — the gather→assault vs solo-travel branch (the pure `squad_is_gathered`): the win-or-
            // stall fast-path is INTEL-GATED, so a vacuous no-intel win on a SCATTERED squad falls to the
            // count quorum (which a scattered roster does not meet) → solo-travel; a fired latch keeps a
            // committed assault. Same `present_wins_or_stalls`/`have_target_intel` inputs the proceed gate
            // used this tick, so the gates agree.
            let gathered = squad_is_gathered(
                present_wins_or_stalls,
                have_target_intel,
                count_quorum_met,
                forming_progress.assault_latched.contains(&obj_id),
            );

            if gathered {
                // ASSAULT: members are massed at the rally → advance the box-formation anchor rally→target
                // (cohesion on the short final leg). The job's `MoveToRoom`/`squad_has_anchor` follows it.
                crate::military::formation::advance_squad_virtual_position(ctx, assault_target);
            } else {
                // SOLO TRAVEL: drop the formation anchor (no cross-room box cohesion during transit) and
                // send each member INDIVIDUALLY to the shared rally. Setting per-member MoveTo orders here
                // (after dropping squad_path) means apply_squad_decision's non-engaged arm leaves them
                // intact (it only stamps Formation orders when a squad_path exists). Members converge solo;
                // the gather quorum then flips this to the assault branch next tick.
                //
                // ── ADR 0034 D6a (RC-7 — PRE-DEPARTURE LIFETIME GATE). Before committing a member to the long
                // `MoveTo(rally)` crawl, check its remaining TTL covers the journey (dist→rally + rally→target)
                // PLUS `FIGHT_BUFFER` ticks of fighting, via the SHARED `lifetime_sufficient_for_deployment`
                // kernel (the sim calls the identical fn). A member that cannot survive the journey HOLDS next
                // to its home spawn (where the Phase-B-renew, now RC-5-extended past the forming gate, tops it
                // up to sufficiency) instead of departing doomed to arrive low/dead → roster drop → quorum
                // oscillation. Once a renew lifts its TTL to `Commit`, it departs. The gate keys on the LIVE
                // `ticks_to_live()` (read fresh each tick) — ephemeral, no serialized state, no WFV bump. The
                // rally→target leg is `room_distance(rally, target)`; the per-member leg is `room_distance(pos,
                // rally)`. A member with no resolvable TTL (just-spawned, position-only) is committed normally
                // (the legacy behaviour) — the gate only HOLDS a member it can prove is too short-lived.
                let rally_to_target = room_distance(rally.room_name(), target_room);
                ctx.squad_path = None;
                for member in ctx.members.iter_mut() {
                    let mut hold_for_renew = false;
                    if let Some(pos) = member.position {
                        let ttl = creep_owner
                            .get(member.entity)
                            .and_then(|co| co.owner.resolve())
                            .and_then(|c| c.ticks_to_live());
                        if let Some(ttl) = ttl {
                            let dist_to_rally = room_distance(pos.room_name(), rally.room_name());
                            let decision = screeps_combat_decision::rally::lifetime_sufficient_for_deployment(
                                ttl,
                                dist_to_rally,
                                rally_to_target,
                                screeps_combat_decision::rally::FIGHT_BUFFER,
                                screeps_combat_decision::rally::RENEW_TARGET_TTL,
                            );
                            // Hold a member that is short of sufficiency (renewable: top it up at home;
                            // hopeless: recycle is a follow-up — for now HOLD so it can't trickle in doomed).
                            // A member already AT the rally (range <= gather radius) never holds — it has
                            // arrived; holding it would un-gather the bloc.
                            hold_for_renew = !matches!(decision, screeps_combat_decision::rally::CommitDecision::Commit)
                                && pos.get_range_to(rally) > screeps_combat_decision::rally::RALLY_GATHER_RADIUS;
                            if hold_for_renew && debug {
                                log::info!(
                                    "[Lifecycle] LIFETIME-HOLD squad={:?} obj={:?} member={:?} ttl={} dist_to_rally={} rally_to_target={} decision={:?} (hold+renew before the crawl, RC-7)",
                                    squad_entity, obj_id, member.entity, ttl, dist_to_rally, rally_to_target, decision
                                );
                            }
                        }
                    }
                    member.tick_orders = Some(TickOrders {
                        // Insufficient TTL → HOLD (next to the home spawn the renew pass tops it up at);
                        // otherwise solo-travel to the shared rally.
                        movement: if hold_for_renew { TickMovement::Hold } else { TickMovement::MoveTo(rally) },
                        ..Default::default()
                    });
                }
            }
            if debug {
                log::info!(
                    "[Lifecycle] TRAVEL squad={:?} room={} rally={:?} gathered={} uncontested={} ({})",
                    squad_entity, target_room, (rally.room_name(), rally.x().u8(), rally.y().u8()),
                    gathered, uncontested, if gathered { "assault: anchor rally->target" } else { "solo travel to shared rally" }
                );
            }
        } else if formation {
            // Arrived + FORMATION (siege, O2): keep the anchor and advance it toward the focus
            // (close to dismantle/weapon range) while ORIENTING the block toward the threat —
            // `reassign_slots` puts tanks/high-HP in the threat-facing slots, healers at the back
            // (`decide_squad.orientation` → `threat_direction`). The job's `squad_has_anchor`
            // branch then formation-follows. (Pure decision in the crate; manager applies; job moves.)
            if let Some(focus) = decision.focus {
                // A STRUCTURE focus (`focus.id` is None) sits on an IMPASSABLE tile: advancing the anchor
                // onto it pathfinds to range 0, finds no path, and reports `Blocked`, so the squad parks
                // SHORT of weapon range and never fires (the invader-core "enters but does nothing" bug,
                // ADR 0026 §9). Stand off one tile toward the squad so the formation holds in weapon range;
                // a creep focus keeps targeting the creep's tile (where the kite logic wants the anchor).
                let dest = match (focus.id, decision.center) {
                    (None, Some(center)) => crate::military::formation::standoff_one_tile(focus.pos, center),
                    _ => focus.pos,
                };
                crate::military::formation::advance_squad_virtual_position(ctx, dest);
            }
            ctx.threat_direction = decision.orientation;
            ctx.reassign_slots();
        } else {
            // Arrived + SKIRMISH: drop the anchor so `Engaged` kites via `decide_movement` (O1).
            ctx.squad_path = None;
        }
        apply_squad_decision(ctx, &decision, creep_owner, in_room_any);
        // ADR 0031 §2(g) FOLLOW-UP 1b — LIVE DRAIN WIRING. The drain tank-forward / healers-behind
        // per-member goals (`decision.member_goals`, stamped onto each member's `tick_orders.squad_movement`
        // in `apply_squad_decision` above) are honored IN-SIM but INERT on the live bot when a Dismantle is
        // in its FORMATION (anchor) phase: with an anchor the job takes `execute_formation_movement`
        // (slot-based), which IGNORES `squad_movement`; only the ANCHORLESS `execute_decide_movement` path
        // reads it. So for a DRAIN directive specifically, DROP the formation anchor here → the job routes
        // through the anchorless path next tick → each member moves to its `member_goal` (tank forward at the
        // standoff, healers one tile behind) — exactly what the sim proves. Scoped to DRAIN ONLY: a non-drain
        // formation (breach / normal siege) keeps its anchor + slots byte-unchanged; the single-member drain
        // is harmless (no slots to lose); on drain EXIT (towers dry → decision drops `Drain`, emits Advance)
        // this no longer fires → the squad re-forms/advances normally. Reuses the existing runtime anchor-drop
        // pattern (rally/solo-travel/skirmish above) → squad_path is set None at RUNTIME → no WFV bump.
        if should_drop_anchor_for_drain(&decision) {
            ctx.squad_path = None;
        }
        // ADR 0036 D4 — STRUCTURE-SIEGE REACH. A core/tower/spawn focus (`focus.id.is_none()`) sits on an
        // impassable tile, so the standoff anchor parks the formation SHORT of weapon range and the squad
        // never razes it (ADR 0026 §9). Drop the anchor — same runtime pattern as the drain drop above —
        // so the job routes ANCHORLESS and each member follows its kernel `member_goal` downhill to range
        // 3, then fires (D3). Scoped to a populated-plan structure siege (the helper requires Engaged +
        // member_goals set ⇒ the kernel ran ⇒ non-kiting), so a CREEP formation keeps its anchor + slots
        // byte-unchanged. (Drain already returned above for a Drain directive; this covers a normal siege.)
        if should_drop_anchor_for_structure_siege(&decision) {
            ctx.squad_path = None;
        }
    }

    // ── ADR 0035 D4 (the LOST-IN-ROOM verdict carrier — stamp for Phase A's `retreated_from_contact`).
    // The DANGER this fixes: deriving abandon from `ctx.state == Retreating` is WRONG because `Retreating` is
    // a SUPERSET of the lose verdict — `decide_squad` also retreats a WINNING fight on a critical-HP member
    // (`any_critical`), a low squad-average (`avg < retreat_threshold`), or a kiting stalemate
    // (`stalemate_disengage`). A squad WINNING a real fight whose focus-fired member dips <25% HP would then
    // read `retreated_from_contact=true` in Phase A → `unwinnable_contact` → the WINNABLE room is backed off
    // and the bloodied-but-winning squad retired MID-FIGHT (the false-abandon). So carry the GENUINE lose
    // verdict instead: `engaged_once && in_room_any && !present_wins_or_stalls` — the EXACT inverse of
    // `present_force_wins_or_stalls` over the REAL in-room view (in-room ⇒ LiveVisible ⇒ assessed over the
    // real towers, no vacuous win). `present_wins_or_stalls` + `in_room_any` are already computed THIS tick
    // above; `engaged_once` is re-read AFTER `apply_squad_decision` latched it. A critical/low-avg/stalemate
    // retreat on a winnable fight has `present_wins_or_stalls=true` ⇒ NOT lost ⇒ NOT abandoned (it holds /
    // wins). Membership insert/remove on the ephemeral `lost_in_room` set (NOT serialized → no WFV bump; no
    // iteration on a result-affecting path → determinism preserved). Phase A reads `contains` — the lose
    // verdict is CARRIED from Phase B, never recomputed in Phase A (the no-view-rebuild-in-A property).
    let engaged_once_for_lose = squad_contexts.get(squad_entity).map(|c| c.engaged_once).unwrap_or(false);
    let lost_in_room = engaged_once_for_lose && in_room_any && !present_wins_or_stalls;
    if lost_in_room {
        forming_progress.lost_in_room.insert(obj_id);
    } else {
        forming_progress.lost_in_room.remove(&obj_id);
    }

    // ───────────────────────── INTROSPECTION TRACE (logging only) ─────────────────────────
    // The full squad journey on one greppable family of lines, gated on the SAME `military.debug_log`
    // flag (free when off). NOTHING below mutates a gate/kernel input — it only reads already-computed
    // facts + the small `last_phase`/`last_engaged` trackers. Emitted on a PHASE CHANGE and on a throttled
    // heartbeat, plus explicit one-shot TRANSITION-EVENT lines. Keeps the existing `[Lifecycle]` lines
    // intact; adds `[SquadTrace]` so the two are independently greppable.
    if debug {
        // Post-decision squad facts (re-borrow immutably; `apply_squad_decision` may have latched engaged).
        let (post_state, engaged_once_now, focus_now) = squad_contexts
            .get(squad_entity)
            .map(|c| (c.state, c.engaged_once, c.focus_target.is_some()))
            .unwrap_or((SquadState::Forming, false, false));

        let present = member_positions.iter().filter(|p| p.is_some()).count();
        let in_room_any = member_views
            .iter()
            .any(|m| m.pos.map(|p| p.room_name() == target_room).unwrap_or(false));
        // Closest member's room-distance to the target (None ⇒ no member has a body yet).
        let target_dist = member_views
            .iter()
            .filter_map(|m| m.pos.map(|p| room_distance(p.room_name(), target_room)))
            .min();

        // Coarse phase from already-computed facts (introspection only — never a gate).
        let phase = if engaged_once_now {
            SquadPhase::Engaged
        } else if in_room_any {
            SquadPhase::InRoom
        } else if !ready_to_depart {
            // Below the rally gate: forming (incomplete roster) vs rallying (quorum/full, gate not released).
            if requested_slots > 0 && present >= requested_slots {
                SquadPhase::Rally
            } else {
                SquadPhase::Forming
            }
        } else {
            // Rally released, full roster present, not yet in-room → crossing.
            SquadPhase::Travel
        };

        let prev_phase = forming_progress.last_phase.get(&obj_id).copied();
        let prev_engaged = forming_progress.last_engaged.get(&obj_id).copied().unwrap_or(false);
        let phase_changed = prev_phase != Some(phase);
        let heartbeat = now.is_multiple_of(SQUAD_TRACE_HEARTBEAT);

        // ── Explicit one-shot TRANSITION-EVENT lines (fire on the edge). ──
        if phase_changed {
            match (prev_phase, phase) {
                // DEPLOY: the rally gate just RELEASED — the anchor switches home → target.
                (Some(SquadPhase::Forming) | Some(SquadPhase::Rally), SquadPhase::Travel)
                | (Some(SquadPhase::Forming) | Some(SquadPhase::Rally), SquadPhase::InRoom)
                | (Some(SquadPhase::Forming) | Some(SquadPhase::Rally), SquadPhase::Engaged) => {
                    log::info!(
                        "[SquadTrace] DEPLOY squad={:?} obj={:?} room={} present={}/{} uncontested={} (rally released; anchor home->target)",
                        squad_entity, obj_id, target_room, present, requested_slots, uncontested
                    );
                }
                _ => {}
            }
            // ARRIVED: first tick a member stands in the target room (Travel → InRoom/Engaged).
            if matches!(prev_phase, Some(SquadPhase::Travel)) && (phase == SquadPhase::InRoom || phase == SquadPhase::Engaged) {
                log::info!(
                    "[SquadTrace] ARRIVED squad={:?} obj={:?} room={} in_room=true present={}/{}",
                    squad_entity, obj_id, target_room, present, requested_slots
                );
            }
        }
        // TRAVEL progress/stall: while crossing, report the room distance + whether it is closing.
        if phase == SquadPhase::Travel {
            let prev_dist = forming_progress.last_target_dist.get(&obj_id).copied();
            let closing = match (target_dist, prev_dist) {
                (Some(cur), Some(prev)) => cur < prev,
                (Some(_), None) => true,
                _ => false,
            };
            if phase_changed || heartbeat {
                log::info!(
                    "[SquadTrace] TRAVEL squad={:?} obj={:?} room={} d={:?} ({})",
                    squad_entity, obj_id, target_room, target_dist, if closing { "progress" } else { "stalled" }
                );
            }
        }
        // FOCUS acquired / empty-DTO fallback (only meaningful once a member is in the room).
        if in_room_any {
            if focus_now && (phase_changed || heartbeat) {
                log::info!(
                    "[SquadTrace] FOCUS acquired squad={:?} obj={:?} room={} hostiles={} structs={} via={}",
                    squad_entity, obj_id, target_room, hostiles.len(), structures.len(),
                    if dto_from_live_fallback { "live-fallback" } else { "mapping" }
                );
            }
            if dto_from_live_fallback && decision.focus.is_none() && (phase_changed || heartbeat) {
                log::info!(
                    "[SquadTrace] FOCUS empty-DTO fallback squad={:?} obj={:?} room={} (game::rooms() re-read; hostiles={} structs={})",
                    squad_entity, obj_id, target_room, hostiles.len(), structures.len()
                );
            }
        }
        // ENGAGED: the engaged_once latch flipped false → true this tick.
        if engaged_once_now && !prev_engaged {
            log::info!(
                "[SquadTrace] ENGAGED squad={:?} obj={:?} room={} state={:?} focus={}",
                squad_entity, obj_id, target_room, post_state, focus_now
            );
        }

        // ── STATE-VECTOR + PER-MEMBER detail (on phase change OR heartbeat). ──
        if phase_changed || heartbeat {
            let forming_started = forming_progress.forming_started_at.get(&obj_id).copied();
            let departed = forming_progress.departed_at.get(&obj_id).copied();
            let forming_budget_left = forming_started.map(|s| MAX_FORMING_BUDGET.saturating_sub(now.saturating_sub(s)));
            let travel_budget_left = departed.map(|s| MAX_TRAVEL_BUDGET.saturating_sub(now.saturating_sub(s)));
            // Lease remaining (deadline - now); `None` if the objective is gone or no deadline stamped.
            let lease_left = deadline.map(|d| d.saturating_sub(now));
            log::info!(
                "[SquadTrace] STATE squad={:?} obj={:?} room={} phase={} state={:?} present={}/{} in_room={} dist={:?} engaged_once={} focus={} lease_left={:?} forming_budget_left={:?} travel_budget_left={:?} reason={}",
                squad_entity, obj_id, target_room, phase.label(), post_state, present, requested_slots,
                in_room_any, target_dist, engaged_once_now, focus_now, lease_left, forming_budget_left, travel_budget_left,
                if phase_changed { "phase-change" } else { "heartbeat" }
            );
            // PER-MEMBER detail companion line: name, room, (x,y), role, spawned (Some pos vs None body).
            if let Some(ctx) = squad_contexts.get(squad_entity) {
                for m in ctx.members.iter() {
                    let name = creep_owner
                        .get(m.entity)
                        .and_then(|co| co.owner.resolve())
                        .map(|c| c.name())
                        .unwrap_or_else(|| "<unspawned>".to_string());
                    let (room_s, x, y) = match m.position {
                        Some(p) => (p.room_name().to_string(), p.x().u8() as i32, p.y().u8() as i32),
                        None => ("?".to_string(), -1, -1),
                    };
                    log::info!(
                        "[SquadTrace]   MEMBER squad={:?} slot={} role={:?} name={} room={} pos=({},{}) spawned={}",
                        squad_entity, m.slot_index, m.role, name, room_s, x, y, m.position.is_some()
                    );
                }
            }
        }

        // Record this tick's phase / engaged latch for the next reconcile's edge detection.
        forming_progress.last_phase.insert(obj_id, phase);
        forming_progress.last_engaged.insert(obj_id, engaged_once_now);
    }
}

/// ADR 0031 §2(g) FOLLOW-UP 1b — should the formation anchor be dropped this tick because the squad is
/// in an ACTIVE drain? When `decide_squad` emits a `SquadMovement::Drain` directive, the per-member drain
/// goals (tank forward at the standoff, healers one tile behind) are stamped onto each member's
/// `tick_orders.squad_movement`, but the live job only READS `squad_movement` on the ANCHORLESS movement
/// path. Dropping the anchor for a `Drain` directive (and ONLY for `Drain`) forces that path so the goals
/// are honored live. Pure + testable so the drain-only scoping is provable offline without a live job.
fn should_drop_anchor_for_drain(decision: &SquadDecision) -> bool {
    matches!(decision.movement, SquadMovement::Drain { .. })
}

/// ADR 0036 D4 — should the formation anchor be dropped this tick because the squad is sieging a
/// STRUCTURE (a core/tower/spawn focus, `focus.id.is_none()`) with no kiting threat? The standoff anchor
/// (`standoff_one_tile`) parks the formation SHORT of weapon range against an impassable structure tile —
/// the ADR 0026 §9 "enters but does nothing" failure: the slotted members may never land within range 3
/// of the core, and the kernel's own approach gradient (the `member_goals` flood downhill to weapon
/// range) is INERT while an anchor is set (the job takes the slot-based formation mover). Dropping the
/// anchor — EXACTLY as the DRAIN path does — routes the job through the anchorless `execute_decide_movement`,
/// so each member moves to its kernel `member_goal` and closes to range, then fires (D3). Scoped to a
/// STRUCTURE focus with a populated kernel plan (`member_goals` set ⇒ the kernel ran ⇒ Engaged + NON-kiting:
/// the kernel block is gated on `!should_kite`, so a creep formation that needs to hold/kite has EMPTY
/// member_goals and keeps its anchor + slots byte-unchanged). Pure + testable so the scoping is provable
/// offline. Runtime anchor-drop (the same pattern as drain/rally/skirmish) → no WFV bump.
fn should_drop_anchor_for_structure_siege(decision: &SquadDecision) -> bool {
    matches!(decision.state, SquadOrderState::Engaged)
        && decision.focus.is_some_and(|f| f.id.is_none())
        && decision.member_goals.iter().any(|g| g.is_some())
}

/// Write a `SquadDecision` into the `SquadContext`: the combat state, the shared focus, and per-member
/// orders. The per-member `movement` stays `Formation` — for a manager squad (no anchor) the job
/// routes it through the pure `decide_movement` (§5 ⚑ job-owns-movement), reading the squad's shared
/// directive (`squad_movement`/`squad_center`/`squad_cohesion_radius`) the manager stamps here so the
/// block kites/advances as one. Heal *assignment* still reuses `SquadContext::compute_heal_assignments`
/// until that migrates into `decide_squad` (Step 7).
fn apply_squad_decision(ctx: &mut SquadContext, decision: &SquadDecision, creep_owner: &ReadStorage<CreepOwner>, in_room_any: bool) {
    ctx.state = order_state_to_squad(decision.state);
    // FIX B1: latch `engaged_once` ONLY when the squad is Engaged AND a member is actually IN the target
    // room. `decide_squad` sets `Engaged` purely from `focus.is_some()` with NO proximity gate (lib.rs), so a
    // far squad whose VISIBLE target room has a hostile would otherwise latch engaged_once while dist>0,
    // in_room=false — permanently killing its travel lease (`traveling` needs `!engaged_once`) → freeze
    // mid-hop. Gating the PERMANENT latch on in-room presence keeps the travel lease alive until arrival.
    // (decide_squad still computes the per-tick focus + Engaged state; only the latch is gated.)
    if ctx.state == SquadState::Engaged && in_room_any {
        ctx.engaged_once = true; // P-OBJ #23: latch reaching combat (drives resolve vs give-up in Phase A)
    }
    ctx.focus_target = decision.focus.map(|f| f.pos);

    match decision.state {
        SquadOrderState::Retreating => {
            ctx.issue_retreat_orders(None, Some(creep_owner));
        }
        SquadOrderState::Engaged => {
            // Per-member focus with damage spill (ADR 0020 §4.2); index aligns with view.members
            // (built from ctx.members in order). `None` ⇒ the shared focus.
            for (i, member) in ctx.members.iter_mut().enumerate() {
                let focus = decision.focus_assignments.get(i).copied().flatten().or(decision.focus);
                let attack_target = focus.map(|f| f.id.map(AttackTarget::Creep).unwrap_or(AttackTarget::Structure(f.pos)));
                // ADR 0019 §8: a member with its own goal (a pure-support healer's heal-coverage tile)
                // moves to that tile instead of the shared block directive; everyone else follows the
                // block. Only the anchorless `decide_movement` path reads `squad_movement`, so this is
                // inert for a siege formation (which keeps its healers-back slots).
                let squad_movement = decision
                    .member_goals
                    .get(i)
                    .copied()
                    .flatten()
                    .map(|goal| SquadMovement::Advance { goal, range: 0 })
                    .unwrap_or(decision.movement);
                member.tick_orders = Some(TickOrders {
                    attack_target,
                    movement: TickMovement::Formation,
                    squad_movement,
                    squad_center: decision.center,
                    squad_cohesion_radius: decision.cohesion_radius,
                    ..Default::default()
                });
            }
            // Apply the pure heal assignments (Step 7): resolve member indices → the target's creep
            // ObjectId, then set each assigned healer's heal_target. (Indices match `member_views`,
            // built in the same order as `ctx.members`.) Resolve first to avoid an aliasing borrow.
            let heal_targets: Vec<(usize, Option<ObjectId<Creep>>)> = decision
                .heal_assignments
                .iter()
                .map(|a| {
                    let target_id = ctx.members.get(a.target_idx).and_then(|m| creep_owner.get(m.entity)).map(|co| co.owner);
                    (a.healer_idx, target_id)
                })
                .collect();
            for (healer_idx, target_id) in heal_targets {
                if let Some(orders) = ctx.members.get_mut(healer_idx).and_then(|m| m.tick_orders.as_mut()) {
                    orders.heal_target = target_id;
                }
            }
        }
        // Forming / Moving (traveling, no engagement yet). When the manager has set a travel
        // anchor (O1), emit a bare `Formation` directive so the job's `MoveToRoom` follows the
        // anchor (cohesive travel) instead of self-driving per-creep. Without an anchor (no layout
        // / no path) this is a no-op and the job falls back to plain room navigation.
        _ => {
            if ctx.squad_path.is_some() {
                for member in ctx.members.iter_mut() {
                    member.tick_orders = Some(TickOrders {
                        movement: TickMovement::Formation,
                        ..Default::default()
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::military::objective_queue::FarmKind;

    fn room(name: &str) -> RoomName {
        name.parse().expect("valid room name")
    }

    /// ADR 0034 D4/D5/D8: `clear_member_trackers` drops ONLY the per-member travel trackers for the given
    /// objective (a per-`(obj_id, member)` keyed sweep) — another objective's members are untouched, so a
    /// retire/reassign of one squad never wipes a sibling's progress/stall state.
    #[test]
    fn clear_member_trackers_is_scoped_to_one_objective() {
        let mut fp = SquadFormingProgress::default();
        let a = ObjectiveId(7);
        let b = ObjectiveId(9);
        fp.member_rally_dist.insert((a, 100), 3);
        fp.member_rally_dist.insert((b, 100), 5);
        fp.member_target_dist.insert((a, 101), 2);
        fp.member_target_dist.insert((b, 102), 4);
        fp.member_solo_stall.insert((a, 100), 80);
        fp.member_solo_stall.insert((b, 103), 10);

        clear_member_trackers(&mut fp, a);

        assert!(fp.member_rally_dist.get(&(a, 100)).is_none(), "obj A rally tracker dropped");
        assert_eq!(fp.member_rally_dist.get(&(b, 100)).copied(), Some(5), "obj B rally tracker retained");
        assert!(fp.member_target_dist.get(&(a, 101)).is_none(), "obj A target tracker dropped");
        assert_eq!(fp.member_target_dist.get(&(b, 102)).copied(), Some(4), "obj B target tracker retained");
        assert!(fp.member_solo_stall.get(&(a, 100)).is_none(), "obj A stall tracker dropped");
        assert_eq!(fp.member_solo_stall.get(&(b, 103)).copied(), Some(10), "obj B stall tracker retained");
    }

    #[test]
    fn objective_target_maps_kind_to_squad_target_and_travel_room() {
        let r = room("W5N5");

        // Farm/Secure/Escort all reduce to "go clear the room".
        let (t, travel) = objective_target(&ObjectiveKind::Farm {
            kind: FarmKind::SourceKeeper,
            room: r,
        });
        assert!(matches!(t, SquadTarget::AttackRoom { room } if room == r));
        assert_eq!(travel, r);

        let (t, _) = objective_target(&ObjectiveKind::Defend { room: r });
        assert!(matches!(t, SquadTarget::DefendRoom { room } if room == r));

        let (t, _) = objective_target(&ObjectiveKind::Harass { room: r });
        assert!(matches!(t, SquadTarget::HarassRoom { room } if room == r));

        // Dismantle travels to the structure's room, targets the position.
        let pos = Position::new(RoomCoordinate::new(10).unwrap(), RoomCoordinate::new(10).unwrap(), r);
        let (t, travel) = objective_target(&ObjectiveKind::Dismantle { room: r, pos });
        assert!(matches!(t, SquadTarget::AttackStructure { position } if position == pos));
        assert_eq!(travel, r);

        // ADR 0027 v1.1 P2: Declaim travels to the controller's room, targets the controller tile.
        let ctrl = Position::new(RoomCoordinate::new(20).unwrap(), RoomCoordinate::new(20).unwrap(), r);
        let (t, travel) = objective_target(&ObjectiveKind::Declaim { room: r, controller: ctrl });
        assert!(matches!(t, SquadTarget::AttackController { position } if position == ctrl));
        assert_eq!(travel, r);
    }

    /// ADR 0027 v1.1 P2: a Declaim objective is its OWN capability class — a CLAIM declaimer is never
    /// reassigned onto combat work (it can't crack a core / clear creeps) and no combat squad is reassigned
    /// onto a Declaim (a RANGED squad can't `attackController`).
    #[test]
    fn declaim_is_a_dedicated_capability_class() {
        let r = room("W5N5");
        let ctrl = Position::new(RoomCoordinate::new(20).unwrap(), RoomCoordinate::new(20).unwrap(), r);
        assert_eq!(capability_class(&ObjectiveKind::Declaim { room: r, controller: ctrl }), CapabilityClass::Declaim);
        assert_ne!(capability_class(&ObjectiveKind::Declaim { room: r, controller: ctrl }), CapabilityClass::Offense);
        assert_ne!(capability_class(&ObjectiveKind::Declaim { room: r, controller: ctrl }), CapabilityClass::Defense);
    }

    #[test]
    fn room_distance_is_chebyshev() {
        assert_eq!(room_distance(room("W0N0"), room("W0N0")), 0);
        assert_eq!(room_distance(room("W1N1"), room("W4N1")), 3); // dx dominates
        assert_eq!(room_distance(room("W1N1"), room("W4N5")), 4); // dy dominates
    }

    /// RC-11 — the gather→ASSAULT vs SOLO-TRAVEL branch is CONDITIONAL on real intel + co-location, so a
    /// vacuous no-intel win on a SCATTERED squad does NOT latch the freeze, while a co-located squad with
    /// real (cached) intel still assaults (no regression to the reaching Entity-100 case).
    #[test]
    fn rc11_scattered_no_intel_solo_travels_but_colocated_with_intel_assaults() {
        // SCATTERED + UNSCOUTED (empty DTOs → present_wins_or_stalls vacuously TRUE, but have_target_intel
        // FALSE; the count quorum is NOT met because members are rooms apart) ⇒ SOLO-TRAVEL (gathered=false).
        // This is the Entity-414 freeze case (members in W9N8/W7N4/W2N5): the intel gate routes it to mass
        // at the rally instead of latching a cross-room formation assault.
        assert!(
            !squad_is_gathered(
                /*present_wins_or_stalls*/ true,
                /*have_target_intel*/ false,
                /*count_quorum_met*/ false,
                /*assault_latched*/ false
            ),
            "scattered + vacuous no-intel win must NOT assault — it solo-travels to the rally (RC-11 fix)"
        );
        // CO-LOCATED + REAL (cached) intel: the win-or-stall fast-path fires (intel present) ⇒ ASSAULT
        // (gathered=true). This is the reaching Entity-100 → W4N7 case — the fix must NOT regress it.
        assert!(
            squad_is_gathered(
                /*present_wins_or_stalls*/ true,
                /*have_target_intel*/ true,
                /*count_quorum_met*/ false,
                /*assault_latched*/ false
            ),
            "co-located squad WITH real intel still latches the assault — the win-or-stall is preserved (D7)"
        );
        // A scattered no-intel squad that has MASSED at the rally (count quorum met) also assaults — the
        // legacy count-gate path still works without the fast-path.
        assert!(
            squad_is_gathered(true, false, /*count_quorum_met*/ true, false),
            "once massed at the rally (count quorum met) the squad assaults via the legacy gate"
        );
        // And a previously-fired latch keeps the assault committed regardless (FIX-A preserved).
        assert!(
            squad_is_gathered(false, false, false, /*assault_latched*/ true),
            "a fired assault latch keeps the squad committed (FIX-A latch preserved)"
        );
    }

    #[test]
    fn forming_combat_squads_spawn_above_economy_bulk() {
        use crate::military::objective_queue::{OBJECTIVE_PRIORITY_CRITICAL, OBJECTIVE_PRIORITY_HIGH, OBJECTIVE_PRIORITY_LOW};
        // FIX 2: active offense (a MEDIUM objective, e.g. an invader core) MUST map to the dedicated
        // COMBAT_FORMING band — STRICTLY between the HIGH economy bulk and the CRITICAL miners — or the
        // spawnsystem head-of-line break strands its forming slots last-in-tier behind the economy bulk and
        // the roster never completes (the dead-stall root). Defense (HIGH) and any CRITICAL map there too.
        assert_eq!(spawn_priority_for(OBJECTIVE_PRIORITY_CRITICAL), SPAWN_PRIORITY_COMBAT_FORMING);
        assert_eq!(spawn_priority_for(OBJECTIVE_PRIORITY_HIGH), SPAWN_PRIORITY_COMBAT_FORMING);
        assert_eq!(
            spawn_priority_for(OBJECTIVE_PRIORITY_MEDIUM),
            SPAWN_PRIORITY_COMBAT_FORMING,
            "MEDIUM offense must form in the COMBAT_FORMING band, not be tied with / starved below the economy bulk"
        );
        // Low-priority farms stay below combat so they never preempt economy.
        assert_eq!(spawn_priority_for(OBJECTIVE_PRIORITY_LOW), SPAWN_PRIORITY_MEDIUM);

        // The band is STRICTLY between the HIGH economy bulk and the CRITICAL miners: forming squad slots
        // win the within-tier race against economy WITHOUT preempting energy income (miners stay first).
        assert!(
            SPAWN_PRIORITY_COMBAT_FORMING > SPAWN_PRIORITY_HIGH,
            "forming squad slots must outrank the HIGH economy bulk (haulers/upgraders/claim/mining)"
        );
        assert!(
            SPAWN_PRIORITY_COMBAT_FORMING < SPAWN_PRIORITY_CRITICAL,
            "forming squad slots must NOT preempt CRITICAL miners (income protected)"
        );
    }

    #[test]
    fn squad_is_wiped_only_after_spawning_then_losing_everyone() {
        assert!(!squad_is_wiped(0, 0), "fresh squad, nothing spawned yet → not wiped");
        assert!(!squad_is_wiped(4, 2), "still has living members → not wiped");
        assert!(squad_is_wiped(4, 0), "spawned members and all are gone → wiped");
    }

    #[test]
    fn rally_gate_picks_quorum_only_for_visible_clear_rooms() {
        // FIX 1: the manager composes `target_is_uncontested` (with the live `game::rooms()` visibility
        // flag) with `ready_to_depart_gate`. This test exercises that exact composition for the four cases:
        // visible+clear deploys at quorum, contested/unseen holds for the full roster.
        let p = Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), room("W7N7"));
        let three_of_five = [Some(p), Some(p), Some(p), None, None];
        let gate = |room_visible: bool, no_hostiles: bool, no_towers: bool, no_safe: bool| {
            let uncontested = crate::military::formation::target_is_uncontested(room_visible, no_hostiles, no_towers, no_safe);
            crate::military::formation::ready_to_depart_gate(&three_of_five, 5, uncontested)
        };
        // Visible + clear + no towers + no safe mode → uncontested → deploy at quorum with 3/5.
        assert!(gate(true, true, true, true), "visible + clear → quorum deploys 3/5");
        // UNSEEN room (empty DTOs, no_hostiles/no_towers read true) → full roster → hold at 3/5.
        assert!(!gate(false, true, true, true), "unseen room (empty DTOs) → full-roster gate holds 3/5");
        // Visible but a hostile creep / tower / safe mode → contested → full roster → hold at 3/5.
        assert!(!gate(true, false, true, true), "hostiles present → full-roster gate holds 3/5");
        assert!(!gate(true, true, false, true), "hostile tower present → full-roster gate holds 3/5");
        assert!(!gate(true, true, true, false), "enemy safe mode → full-roster gate holds 3/5");
    }

    #[test]
    fn forming_state_progress_is_bounded_to_increasing_present_count() {
        // FIX 2: a squad with members, not yet engaged, below the requested roster is FORMING; progress
        // is true ONLY when the present count grew since last reconcile (self-bounding).
        // present 4, prev 3, requested 5 → forming + progress (a member just appeared).
        assert_eq!(forming_state(true, false, 4, 5, 3), (true, true), "present grew → forming + progress");
        // present 3, prev 3 (flat — can't bank energy for #4) → forming but NO progress → kernel gives up.
        assert_eq!(forming_state(true, false, 3, 5, 3), (true, false), "flat present → forming, no progress");
        // full roster present (5/5) → NOT forming (the squad departs).
        assert_eq!(forming_state(true, false, 5, 5, 4), (false, false), "full roster → not forming");
        // engaged already → never forming (the lease refreshes via focus, not the forming path).
        assert_eq!(forming_state(true, true, 3, 5, 2), (false, false), "engaged → not forming");
        // no members / unknown roster → not forming (legacy preserved).
        assert_eq!(forming_state(false, false, 0, 5, 0), (false, false), "no members → not forming");
        assert_eq!(forming_state(true, false, 1, 0, 0), (false, false), "unknown roster size → not forming");
    }

    #[test]
    fn spawn_order_puts_fighters_before_support() {
        use screeps_combat_decision::bodies::CombatBodySpec;
        use screeps_combat_decision::composition::{BodyType, SquadRole};
        let slot = |role: SquadRole| SquadSlot { role, body_type: BodyType::Sized(CombatBodySpec::default()) };
        // A healer-front composition (assemble_force orders Healer first): Healer, Healer, RangedDPS, Tank.
        let slots = vec![slot(SquadRole::Healer), slot(SquadRole::Healer), slot(SquadRole::RangedDPS), slot(SquadRole::Tank)];
        let order = spawn_order_fighter_first(&slots);
        // The RangedDPS fighter (slot index 2) is attempted FIRST, support after — slot indices preserved.
        assert_eq!(order, vec![2, 0, 1, 3], "fighter (RangedDPS) spawns first, support after, indices preserved");
        // A dismantler + ranged + 2 healers: both fighters precede both healers, stable within each group.
        let siege = vec![slot(SquadRole::Healer), slot(SquadRole::Dismantler), slot(SquadRole::Healer), slot(SquadRole::RangedDPS)];
        assert_eq!(spawn_order_fighter_first(&siege), vec![1, 3, 0, 2], "fighters (Dismantler, RangedDPS) first, healers after");
        // An all-support (no fighter) roster keeps its original order (degenerate; no reorder).
        let support = vec![slot(SquadRole::Healer), slot(SquadRole::Tank)];
        assert_eq!(spawn_order_fighter_first(&support), vec![0, 1], "no fighters → original order");
    }

    /// EV-WIRING REGRESSION (ADR 0032 v1.1 verifier-found): the per-squad auction EV must price the hostile
    /// CREEP force. A room defended ONLY by hostile creeps (no energized towers, objective_hits=0) used to read
    /// as `undefended` in `pairing_p_win` (because `objective_ev_q` passed `enemy: None`; the scouted DPS had no
    /// channel that `pairing_p_win` reads — pre-ADR-0031-#41 it was written to the dead `DefenseProfile.enemy_dps`,
    /// now that field is gone) → P(win)=1.0 against a room full of
    /// attackers, inflating EV for creep-defended offense/defense. The fix builds an `EnemyForce` from the
    /// threat and passes it as the `enemy` arg. This test is deterministic + offline (no game state): it drives
    /// `objective_ev_q` exactly as the bot does and proves (a) a creep-defended objective now scores a LOWER EV
    /// than the same objective undefended (no free win against attackers), and (b) a genuinely UNDEFENDED
    /// objective still scores P(win)=1.0 (EV == value_e, no travel here).
    #[test]
    fn objective_ev_prices_enemy_creeps_no_free_win() {
        use crate::military::threatmap::{HostileCreepInfo, RoomThreatData};

        let r = room("W5N5");
        let kind = ObjectiveKind::Harass { room: r }; // Denial value_e — a creep-defended offense objective.
        let priority = crate::military::objective_queue::OBJECTIVE_PRIORITY_MEDIUM;

        // A real clearing force that CANNOT out-heal a heavy attacker (heal=0): it kills (structure_dps>0) but
        // dies under sustained incoming creep DPS → P(win) must drop below 1.
        let caps = SquadCapabilities { heal_per_tick: 0, structure_dps: 300, tank_effective_hp: 5_000 };

        // value_e is unaffected by defense, so EV is directly comparable across the two threat profiles.
        // No towers in EITHER case — the ONLY difference is the hostile-creep force.
        let val = value_e(project_value_kind(&kind, None), &project_intel(&kind, priority, 0.0, None, None));
        assert!(val > 0.0, "Denial value_e must be positive for a comparable EV");

        // (b) CONTROL — genuinely undefended (no intel at all): undefended binary → P(win)=1.0 → EV == value_e.
        let ev_undefended = objective_ev_q(caps, &kind, priority, 0.0, None, None, 1_500, 0);
        assert_eq!(
            ev_undefended,
            quantize_ev(val),
            "an UNDEFENDED objective (no threat) must keep P(win)=1.0 → EV == value_e"
        );

        // (a) Enemy CREEPS only — heavy attacker DPS, NO towers, no structure to kill (objective_hits=0).
        let attacker = HostileCreepInfo {
            position: Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), r),
            owner: "enemy".to_string(),
            hits: 2_000,
            hits_max: 2_000,
            melee_dps: 240.0,
            ranged_dps: 0.0,
            heal_per_tick: 0.0,
            tough_hp: 0.0,
            work_parts: 0,
            boosted: false,
        };
        let threat = RoomThreatData {
            estimated_attack_dps: 240.0, // a heavy attacker the heal-less squad cannot survive
            hostile_creeps: vec![attacker],
            ..Default::default() // NO towers (hostile_tower_positions empty), no safe mode, no breach hits
        };
        let ev_creep_defended = objective_ev_q(caps, &kind, priority, 0.0, Some(&threat), None, 1_500, 0);

        // The whole point: pricing the enemy creeps makes a creep-defended objective NO LONGER a free win.
        assert!(
            ev_creep_defended < ev_undefended,
            "creep-defended EV ({ev_creep_defended}) must be LOWER than undefended EV ({ev_undefended}) — \
             enemy creeps must be priced (P(win) < 1), no free win against attackers"
        );
        // And concretely below the certain-win value (P(win) strictly < 1).
        assert!(
            ev_creep_defended < quantize_ev(val),
            "creep-defended EV ({ev_creep_defended}) must be below the P(win)=1 value ({})",
            quantize_ev(val)
        );
    }

    #[test]
    fn only_dismantle_fights_as_a_formation() {
        let r = room("W5N5");
        let pos = Position::new(RoomCoordinate::new(10).unwrap(), RoomCoordinate::new(10).unwrap(), r);
        assert!(is_formation_objective(&ObjectiveKind::Dismantle { room: r, pos }));
        assert!(!is_formation_objective(&ObjectiveKind::Defend { room: r }));
        assert!(!is_formation_objective(&ObjectiveKind::Farm { kind: FarmKind::SourceKeeper, room: r }));
        assert!(!is_formation_objective(&ObjectiveKind::Harass { room: r }));
        assert!(!is_formation_objective(&ObjectiveKind::Secure { room: r }));
    }

    #[test]
    fn classify_objective_routes_breach_vs_open() {
        use CombatObjectiveClass::*;
        // Explicit dismantle objective → breach, regardless of room contents.
        assert_eq!(classify_objective(true, false, false), StructureBreach);
        assert_eq!(classify_objective(true, false, true), StructureBreach);
        // Non-formation: structures present + NO live hostiles → breach (creeps cleared, raze the ring).
        assert_eq!(classify_objective(false, true, false), StructureBreach);
        // Non-formation with live hostiles → open creep combat (kill the creeps first).
        assert_eq!(classify_objective(false, true, true), OpenCombat);
        assert_eq!(classify_objective(false, false, true), OpenCombat);
        // Empty room (no structures, no hostiles) → open (nothing to breach).
        assert_eq!(classify_objective(false, false, false), OpenCombat);
    }

    /// ADR 0032 v2 (same-tick DOUBLE-FILL guard): the registration predicate the spawn callback uses. A
    /// freshly-spawned creep is registered ONLY when its squad is alive AND the slot is not already filled.
    #[test]
    fn spawned_member_registration_is_gated_on_alive_and_unfilled_slot() {
        assert!(should_register_spawned_member(true, false), "alive + open slot → register the new member");
        assert!(
            !should_register_spawned_member(true, true),
            "alive but the slot is ALREADY filled (merge-transfer surplus) → do NOT register a second member"
        );
        assert!(!should_register_spawned_member(false, false), "squad dead → do NOT register");
        assert!(!should_register_spawned_member(false, true), "squad dead → do NOT register");
    }

    /// Sim/live PARITY (rally-oscillation fix): the LIVE intel-reliability decision
    /// (`CombatIntelSource::is_reliable`, which feeds `target_is_uncontested`) must agree with the decision
    /// kernel the rally tests actually prove (`rally::rally_intel_reliable`) for EVERY DTO source — so the
    /// offline oscillation proof genuinely covers the live path (the two logically-identical impls can't drift).
    #[test]
    fn combat_intel_source_reliability_matches_the_decision_kernel() {
        use screeps_combat_decision::rally::rally_intel_reliable;
        // Variant → the kernel's (mapped, live_visible) the live path encodes.
        assert_eq!(
            CombatIntelSource::Cached.is_reliable(),
            rally_intel_reliable(true, false),
            "Cached ⇔ mapped: reliable regardless of current live vision (the stability property)"
        );
        assert_eq!(
            CombatIntelSource::LiveVisible.is_reliable(),
            rally_intel_reliable(false, true),
            "LiveVisible ⇔ unmapped but live-visible: reliable"
        );
        assert_eq!(
            CombatIntelSource::None.is_reliable(),
            rally_intel_reliable(false, false),
            "None ⇔ neither: unreliable (never trust no-vision emptiness)"
        );
    }

    /// ADR 0032 v2 (same-tick DOUBLE-FILL guard, integration): a `SquadContext` whose `slot_index` was just
    /// filled by a merge transfer reports `is_slot_filled(slot_index) == true`, which drives the callback's
    /// `should_register_spawned_member` to FALSE — so the late spawn-callback never pushes a SECOND member at
    /// that slot (the over-roster bug). An untouched sibling slot still admits its member.
    #[test]
    fn is_slot_filled_blocks_a_second_member_at_a_merge_filled_slot() {
        use screeps_combat_decision::bodies::CombatBodySpec;
        use screeps_combat_decision::composition::{BodyType, FormationShape, SquadComposition, SquadRole, SquadSlot};
        use specs::WorldExt;

        // A 2-slot receiver composition (one RangedDPS, one Healer): slot 0 is the merge-filled pending slot,
        // slot 1 is still open.
        let sized_ranged = BodyType::Sized(CombatBodySpec { ranged_attack: 2, ..Default::default() });
        let sized_heal = BodyType::Sized(CombatBodySpec { heal: 2, ..Default::default() });
        let comp = SquadComposition {
            label: "Merge receiver".into(),
            slots: vec![
                SquadSlot { role: SquadRole::RangedDPS, body_type: sized_ranged },
                SquadSlot { role: SquadRole::Healer, body_type: sized_heal },
            ],
            formation_shape: FormationShape::Box2x2,
            formation_mode: Default::default(),
            retreat_threshold: 0.5,
        };

        let mut world = World::new();
        world.register::<SquadContext>();
        let transferred_creep = world.create_entity().build();
        let late_spawn_creep = world.create_entity().build();

        // The receiver fields slot 0 by a merge transfer (the deferred `apply_merges` `add_member`).
        let mut ctx = SquadContext::from_composition(&comp);
        ctx.add_member(transferred_creep, SquadRole::RangedDPS, 0);

        // The late spawn callback (queued by Phase B before the transfer applied) now runs and rechecks.
        let slot0_filled = ctx.is_slot_filled(0);
        let slot1_filled = ctx.is_slot_filled(1);
        assert!(slot0_filled, "the merge transfer filled slot 0");
        assert!(!slot1_filled, "slot 1 is still open");

        // The guard: slot 0 is filled → do NOT register a SECOND member there (the surplus recalls/recycles
        // via its squad-bound job, see `jobs::squad_combat::recall_decision`).
        assert!(
            !should_register_spawned_member(true, slot0_filled),
            "the late spawn must NOT add a second member at the merge-filled slot 0"
        );
        // Had the guard been bypassed, registering would over-roster the slot — prove that would be a dup.
        ctx.add_member(late_spawn_creep, SquadRole::RangedDPS, 0);
        let slot0_members = ctx.members.iter().filter(|m| m.slot_index == 0).count();
        assert_eq!(slot0_members, 2, "demonstrate the double-fill the guard PREVENTS in the callback");

        // A still-open sibling slot is admitted normally.
        assert!(should_register_spawned_member(true, slot1_filled), "an open sibling slot still admits its member");
    }

    /// ADR 0031 §2(g) FOLLOW-UP 1b — the DRAIN-only scoping of the live anchor-drop. The predicate the
    /// reconcile gate uses fires ONLY for a `SquadMovement::Drain` directive; every non-drain movement
    /// (Advance / Kite / Hold) keeps the formation anchor (byte-unchanged formation slots).
    #[test]
    fn drain_anchor_drop_predicate_fires_only_for_drain() {
        let r = room("W5N5");
        let goal = Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), r);
        let decision_for = |movement: SquadMovement| SquadDecision {
            state: SquadOrderState::Engaged,
            focus: None,
            movement,
            center: Some(goal),
            cohesion_radius: 1,
            heal_assignments: Vec::new(),
            focus_assignments: Vec::new(),
            orientation: None,
            member_goals: Vec::new(),
            member_intents: Vec::new(),
        };

        // DRAIN → drop the anchor (route through the anchorless path so member_goals are honored live).
        assert!(should_drop_anchor_for_drain(&decision_for(SquadMovement::Drain {
            goal,
            standoff_range: 6
        })));
        // Non-drain formations KEEP the anchor (formation slots byte-unchanged).
        assert!(!should_drop_anchor_for_drain(&decision_for(SquadMovement::Advance { goal, range: 0 })));
        assert!(!should_drop_anchor_for_drain(&decision_for(SquadMovement::Kite { goal })));
        assert!(!should_drop_anchor_for_drain(&decision_for(SquadMovement::Hold)));
    }

    #[test]
    fn structure_siege_anchor_drop_predicate_is_scoped_to_a_kernel_planned_structure_focus() {
        // ADR 0036 D4 — the anchor is dropped ONLY for a structure-focus siege with a populated kernel plan
        // (Engaged + focus.id.is_none() + member_goals set), so CREEP formations keep their anchor + slots
        // byte-unchanged and an empty-plan (kiting) structure case is untouched.
        use crate::combat::FocusTarget;
        let r = room("W5N5");
        let p = |x: u8, y: u8| Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), r);
        let goal = p(25, 25);
        let base = |state: SquadOrderState, focus: Option<FocusTarget>, member_goals: Vec<Option<Position>>| SquadDecision {
            state,
            focus,
            movement: SquadMovement::Advance { goal, range: 0 },
            center: Some(goal),
            cohesion_radius: 1,
            heal_assignments: Vec::new(),
            focus_assignments: Vec::new(),
            orientation: None,
            member_goals,
            member_intents: Vec::new(),
        };
        let struct_focus = Some(FocusTarget { pos: p(27, 25), id: None });
        let creep_focus = Some(FocusTarget { pos: p(27, 25), id: Some("0123456789abcdef01234567".parse::<RawObjectId>().unwrap()) });

        // FIRES: Engaged + structure focus + a populated kernel plan (non-kiting siege).
        assert!(should_drop_anchor_for_structure_siege(&base(SquadOrderState::Engaged, struct_focus, vec![Some(goal)])));
        // NOT a creep focus (a creep formation keeps its anchor).
        assert!(!should_drop_anchor_for_structure_siege(&base(SquadOrderState::Engaged, creep_focus, vec![Some(goal)])));
        // NOT when member_goals are empty (the kernel didn't run ⇒ kiting/forming — keep the anchor).
        assert!(!should_drop_anchor_for_structure_siege(&base(SquadOrderState::Engaged, struct_focus, vec![None, None])));
        // NOT when not Engaged (forming/moving keep their travel anchor).
        assert!(!should_drop_anchor_for_structure_siege(&base(SquadOrderState::Moving, struct_focus, vec![Some(goal)])));
        // NOT with no focus at all.
        assert!(!should_drop_anchor_for_structure_siege(&base(SquadOrderState::Engaged, None, vec![Some(goal)])));
    }

    /// ADR 0031 §2(g) FOLLOW-UP 1b — the LIVE drain routing, end-to-end over the reconcile drain-gate
    /// behavior. A Dismantle squad in an ACTIVE drain (`movement = Drain`, per-member `member_goals` = tank
    /// forward at the standoff, healers one tile behind, `squad_path` = Some(anchor)) must, after the gate:
    ///   1. drop its anchor (`squad_path == None` → `squad_has_anchor()` false → anchorless routing), AND
    ///   2. carry each member's drain goal as its `tick_orders.squad_movement == Advance{goal, range:0}`
    ///      (the directive the anchorless `decide_movement` reads → tank closes to standoff, healers hold a
    ///      tile back) — exactly what the sim proves.
    /// Control: a NON-drain Dismantle (`movement = Advance`, anchor set) KEEPS its anchor (formation slots
    /// byte-unchanged). The single-member drain is also covered (the anchor-drop is harmless there).
    #[test]
    fn drain_reconcile_drops_anchor_and_routes_member_goals_live() {
        use crate::military::squad::SquadPath;
        use screeps_combat_decision::bodies::CombatBodySpec;
        use screeps_combat_decision::composition::{BodyType, FormationShape, SquadComposition, SquadRole, SquadSlot};
        use screeps_rover::AnchorPath;
        use specs::WorldExt;

        let r = room("W5N5");
        // A drain comp: a TOUGH+HEAL tank (slot 0) + two Healers behind it.
        let tank = BodyType::Sized(CombatBodySpec { tough: 10, heal: 4, ..Default::default() });
        let healer = BodyType::Sized(CombatBodySpec { heal: 8, ..Default::default() });
        let comp = SquadComposition {
            label: "Drain".into(),
            slots: vec![
                SquadSlot { role: SquadRole::Tank, body_type: tank },
                SquadSlot { role: SquadRole::Healer, body_type: healer },
                SquadSlot { role: SquadRole::Healer, body_type: healer },
            ],
            formation_shape: FormationShape::Box2x2,
            formation_mode: Default::default(),
            retreat_threshold: 0.3,
        };

        let mut world = World::new();
        world.register::<SquadContext>();
        world.register::<CreepOwner>();
        let m0 = world.create_entity().build();
        let m1 = world.create_entity().build();
        let m2 = world.create_entity().build();
        world.maintain();
        // An empty CreepOwner storage — no member is resolved to a live creep (the member_goals stamping
        // does not touch creep_owner; the heal-assignment resolution simply yields None target_ids).
        let creep_owner = world.read_storage::<CreepOwner>();

        // The drain standoff goal (the tower nest) + the per-member drain goals the decision crate emits:
        // the tank forward AT the standoff, the two healers ONE TILE BEHIND it.
        let nest = Position::new(RoomCoordinate::new(25).unwrap(), RoomCoordinate::new(25).unwrap(), r);
        let tank_goal = Position::new(RoomCoordinate::new(18).unwrap(), RoomCoordinate::new(25).unwrap(), r); // at standoff
        let healer_goal = Position::new(RoomCoordinate::new(17).unwrap(), RoomCoordinate::new(25).unwrap(), r); // a tile back
        let member_goals = vec![Some(tank_goal), Some(healer_goal), Some(healer_goal)];

        let drain_decision = SquadDecision {
            state: SquadOrderState::Engaged,
            focus: None,
            movement: SquadMovement::Drain { goal: nest, standoff_range: 7 },
            center: Some(tank_goal),
            cohesion_radius: 1,
            heal_assignments: Vec::new(),
            focus_assignments: Vec::new(),
            orientation: None,
            member_goals: member_goals.clone(),
            member_intents: Vec::new(),
        };

        // ── DRAIN squad (multi-member): start in the formation/anchor phase. ──
        let mut ctx = SquadContext::from_composition(&comp);
        ctx.add_member(m0, SquadRole::Tank, 0);
        ctx.add_member(m1, SquadRole::Healer, 1);
        ctx.add_member(m2, SquadRole::Healer, 2);
        // The squad holds a formation anchor (the gather-quorum assault set it).
        ctx.squad_path = Some(SquadPath {
            anchor: AnchorPath::new(nest, nest),
            room_route: vec![r],
        });
        assert!(ctx.squad_path.is_some(), "precondition: the squad holds a formation anchor");

        // Reproduce the reconcile drain-gate exactly: stamp the decision, THEN the drain anchor-drop.
        apply_squad_decision(&mut ctx, &drain_decision, &creep_owner, true);
        if should_drop_anchor_for_drain(&drain_decision) {
            ctx.squad_path = None;
        }

        // (1) The anchor is dropped → the job takes the ANCHORLESS `decide_movement` path next tick.
        assert!(ctx.squad_path.is_none(), "drain drops the formation anchor → anchorless routing");
        // (2) Each member carries its OWN drain goal as Advance{goal, range:0} (what decide_movement reads).
        for (member, goal) in ctx.members.iter().zip(member_goals.iter()) {
            let orders = member.tick_orders.as_ref().expect("a drain member has tick_orders");
            match orders.squad_movement {
                SquadMovement::Advance { goal: g, range } => {
                    assert_eq!(Some(g), *goal, "the member moves to its own drain goal");
                    assert_eq!(range, 0, "the per-member goal is stamped at range 0");
                }
                other => panic!("a drain member must route its member_goal, got {other:?}"),
            }
        }

        // ── CONTROL: a NON-drain Dismantle (Advance) KEEPS its anchor (formation slots byte-unchanged). ──
        let advance_decision = SquadDecision {
            movement: SquadMovement::Advance { goal: nest, range: 0 },
            member_goals: Vec::new(), // a siege formation has no per-member goals
            ..drain_decision.clone()
        };
        let mut ctx2 = SquadContext::from_composition(&comp);
        ctx2.add_member(m0, SquadRole::Tank, 0);
        ctx2.add_member(m1, SquadRole::Healer, 1);
        ctx2.add_member(m2, SquadRole::Healer, 2);
        ctx2.squad_path = Some(SquadPath {
            anchor: AnchorPath::new(nest, nest),
            room_route: vec![r],
        });
        apply_squad_decision(&mut ctx2, &advance_decision, &creep_owner, true);
        if should_drop_anchor_for_drain(&advance_decision) {
            ctx2.squad_path = None;
        }
        assert!(ctx2.squad_path.is_some(), "a non-drain Dismantle KEEPS its formation anchor");

        // ── SINGLE-MEMBER drain: the anchor-drop is harmless (one member still routes its own goal). ──
        let solo_goals = vec![Some(tank_goal)];
        let solo_decision = SquadDecision {
            movement: SquadMovement::Drain { goal: nest, standoff_range: 7 },
            member_goals: solo_goals.clone(),
            ..drain_decision.clone()
        };
        let solo_comp = SquadComposition {
            label: "Solo drain".into(),
            slots: vec![SquadSlot {
                role: SquadRole::Tank,
                body_type: BodyType::Sized(CombatBodySpec { tough: 10, heal: 4, ..Default::default() }),
            }],
            formation_shape: FormationShape::None,
            formation_mode: Default::default(),
            retreat_threshold: 0.3,
        };
        let mut ctx3 = SquadContext::from_composition(&solo_comp);
        ctx3.add_member(m0, SquadRole::Tank, 0);
        ctx3.squad_path = Some(SquadPath {
            anchor: AnchorPath::new(nest, nest),
            room_route: vec![r],
        });
        apply_squad_decision(&mut ctx3, &solo_decision, &creep_owner, true);
        if should_drop_anchor_for_drain(&solo_decision) {
            ctx3.squad_path = None;
        }
        assert!(ctx3.squad_path.is_none(), "single-member drain still drops the anchor (harmless)");
        let solo_orders = ctx3.members[0].tick_orders.as_ref().expect("solo drain member has tick_orders");
        assert!(
            matches!(solo_orders.squad_movement, SquadMovement::Advance { goal, range: 0 } if goal == tank_goal),
            "the solo drain member routes its own goal"
        );
    }

    /// ADR 0036 D4 apply + D3 stamp (PROOF) — the LIVE wiring the eval CANNOT reach (it doesn't depend on
    /// the bot crate; `ManagedSimSquad` is anchorless, so the anchor-drop + the `AttackTarget` stamp have no
    /// analogue there). This unit-drives the EXACT reconcile Engaged arm for a STRUCTURE siege — the same
    /// two lines the manager runs at squad_manager.rs:2537-2539 (D4 anchor-drop) and 2765-2767 (D3 stamp):
    ///   1. D4 REACH: `apply_squad_decision` then `should_drop_anchor_for_structure_siege` drops the anchor
    ///      (`ctx.squad_path == None`) so the job routes ANCHORLESS to each member's `member_goal` (the
    ///      approach gradient closes to weapon range — the ADR 0026 §9 standoff-park fix).
    ///   2. D3 STAMP: every present member's `tick_orders.attack_target == AttackTarget::Structure(pos)` —
    ///      the position-only (`id: None`) focus the job's `resolve_focus` keeps + `translate_intents`
    ///      focus-fires by position (NOT the old `resolve_creep()` drop → undirected fire).
    /// RED-ability (both revert to master's 0-damage bug): (1) delete the `should_drop_anchor_for_structure_
    /// siege` block at squad_manager.rs:2537-2539 → `squad_path` stays `Some(anchor)` → the first assert
    /// fails (the formation parks short of range). (2) revert the D3 stamp so a structure focus stamps a
    /// creep target / no target → the `attack_target` assert fails. CONTROL: a CREEP focus keeps its anchor
    /// (formation slots byte-unchanged) and stamps `AttackTarget::Creep`.
    ///
    /// The `game::*` BOUNDARY documented (what stays live-only): `apply_squad_decision` needs only a `World`
    /// (for the entities), a `CreepOwner` storage (read as `None` here → heal targets resolve to `None`, fine
    /// for a non-heal structure siege), and the plain `SquadContext`/`SquadDecision` data — NO `game::*`. What
    /// remains live-only is (a) resolving `AttackTarget::Structure(pos)` → the game structure object at the
    /// tile (the job's `struct_at(pos)` in `translate_intents`, squad_combat.rs:564-583), and (b) the rover
    /// pathing that the dropped anchor unblocks; both are exercised on the private-server soak, not on host.
    #[test]
    fn structure_siege_reconcile_drops_anchor_and_stamps_structure_attack_target_live() {
        use crate::military::squad::SquadPath;
        use crate::combat::FocusTarget;
        use screeps_combat_decision::bodies::CombatBodySpec;
        use screeps_combat_decision::composition::{BodyType, FormationShape, SquadComposition, SquadRole, SquadSlot};
        use screeps_rover::AnchorPath;
        use specs::WorldExt;

        let r = room("W5N3");
        let p = |x: u8, y: u8| Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), r);
        // A bare-core RANGED quad (the NpcCore doctrine fields ranged, not WORK — cores are dismantle-immune).
        let ranged = BodyType::Sized(CombatBodySpec { ranged_attack: 4, ..Default::default() });
        let comp = SquadComposition {
            label: "Core siege".into(),
            slots: vec![
                SquadSlot { role: SquadRole::RangedDPS, body_type: ranged },
                SquadSlot { role: SquadRole::RangedDPS, body_type: ranged },
                SquadSlot { role: SquadRole::RangedDPS, body_type: ranged },
            ],
            formation_shape: FormationShape::Box2x2,
            formation_mode: Default::default(),
            retreat_threshold: 0.3,
        };

        let mut world = World::new();
        world.register::<SquadContext>();
        world.register::<CreepOwner>();
        let m0 = world.create_entity().build();
        let m1 = world.create_entity().build();
        let m2 = world.create_entity().build();
        world.maintain();
        // Empty CreepOwner storage: a structure siege stamps no heal targets, so no member needs a live creep.
        let creep_owner = world.read_storage::<CreepOwner>();

        // The core tile (impassable, id:None structure focus) + the kernel's per-member approach goals
        // (each ranged member's downhill tile toward weapon range 3 of the core).
        let core = p(27, 25);
        let g0 = p(24, 25);
        let g1 = p(24, 26);
        let g2 = p(24, 24);
        let member_goals = vec![Some(g0), Some(g1), Some(g2)];
        let struct_focus = Some(FocusTarget { pos: core, id: None });

        let decision = SquadDecision {
            state: SquadOrderState::Engaged,
            focus: struct_focus,
            movement: SquadMovement::Advance { goal: core, range: 0 },
            center: Some(p(24, 25)),
            cohesion_radius: 1,
            heal_assignments: Vec::new(),
            focus_assignments: Vec::new(), // no per-member spill → each member falls back to the shared focus
            orientation: None,
            member_goals: member_goals.clone(),
            member_intents: Vec::new(),
        };

        // Start in the formation/anchor phase (the gather-quorum assault set the standoff anchor).
        let mut ctx = SquadContext::from_composition(&comp);
        ctx.add_member(m0, SquadRole::RangedDPS, 0);
        ctx.add_member(m1, SquadRole::RangedDPS, 1);
        ctx.add_member(m2, SquadRole::RangedDPS, 2);
        ctx.squad_path = Some(SquadPath {
            anchor: AnchorPath::new(core, core),
            room_route: vec![r],
        });
        assert!(ctx.squad_path.is_some(), "precondition: the siege holds a formation (standoff) anchor");

        // Reproduce the reconcile Engaged arm EXACTLY: stamp the decision (D3 attack_target), THEN the D4
        // structure-siege anchor-drop (squad_manager.rs:2537-2539). The drain drop above does not fire here
        // (`movement` is Advance, not Drain), so this covers the NORMAL (non-drain) structure siege.
        apply_squad_decision(&mut ctx, &decision, &creep_owner, true);
        if should_drop_anchor_for_drain(&decision) {
            ctx.squad_path = None;
        }
        if should_drop_anchor_for_structure_siege(&decision) {
            ctx.squad_path = None;
        }

        // (1) D4 REACH: the anchor is dropped → anchorless routing next tick (the approach gradient closes).
        assert!(ctx.squad_path.is_none(), "D4: a structure siege drops the standoff anchor → anchorless approach");
        // (2) D3 STAMP: EVERY present member fires the SAME position-only structure focus (directed raze).
        assert_eq!(ctx.members.len(), 3, "all three ranged members present");
        for member in ctx.members.iter() {
            let orders = member.tick_orders.as_ref().expect("an Engaged member has tick_orders");
            // `AttackTarget` is Copy/Debug but not PartialEq (production; not touched here), so match it.
            assert!(
                matches!(orders.attack_target, Some(AttackTarget::Structure(t)) if t == core),
                "D3: the member focus-fires the core by position (id None) — not the OLD undirected drop, got {:?}",
                orders.attack_target
            );
        }
        // Each member also carries its own kernel approach goal (the anchorless mover reads this to close).
        for (member, goal) in ctx.members.iter().zip(member_goals.iter()) {
            let orders = member.tick_orders.as_ref().unwrap();
            assert!(
                matches!(orders.squad_movement, SquadMovement::Advance { goal: g, range: 0 } if Some(g) == *goal),
                "the member routes its own kernel member_goal toward weapon range"
            );
        }

        // ── CONTROL: a CREEP focus keeps its anchor (formation byte-unchanged) + stamps a Creep target. ──
        let live_creep: RawObjectId = "0123456789abcdef01234567".parse().unwrap();
        let creep_decision = SquadDecision {
            focus: Some(FocusTarget { pos: p(26, 25), id: Some(live_creep) }),
            member_goals: vec![None, None, None], // a kiting creep formation has no kernel approach plan
            ..decision.clone()
        };
        let mut ctx2 = SquadContext::from_composition(&comp);
        ctx2.add_member(m0, SquadRole::RangedDPS, 0);
        ctx2.add_member(m1, SquadRole::RangedDPS, 1);
        ctx2.add_member(m2, SquadRole::RangedDPS, 2);
        ctx2.squad_path = Some(SquadPath {
            anchor: AnchorPath::new(core, core),
            room_route: vec![r],
        });
        apply_squad_decision(&mut ctx2, &creep_decision, &creep_owner, true);
        if should_drop_anchor_for_drain(&creep_decision) {
            ctx2.squad_path = None;
        }
        if should_drop_anchor_for_structure_siege(&creep_decision) {
            ctx2.squad_path = None;
        }
        assert!(ctx2.squad_path.is_some(), "a CREEP formation KEEPS its anchor (D4 scoped to id.is_none())");
        // `AttackTarget` is Copy/Debug but not PartialEq (production; not touched here), so match it.
        assert!(
            matches!(ctx2.members[0].tick_orders.as_ref().unwrap().attack_target, Some(AttackTarget::Creep(id)) if id == live_creep),
            "a creep focus stamps a Creep attack_target (creep-fights untouched)"
        );
    }
}
