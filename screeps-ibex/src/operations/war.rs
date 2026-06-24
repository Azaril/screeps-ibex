use super::data::*;
use super::operationsystem::*;
use crate::military::composition::SquadComposition;
use crate::military::force_sizing::{
    assess, importance_margin, win_probability, DefenseProfile, ForceBudget, RequiredForce, TowerThreat, HOLD_MARGIN,
};
use crate::military::objective_queue::{
    ForceRequirement, ObjectiveKind, ObjectiveOwner, ObjectiveRequest, OBJECTIVE_PRIORITY_CRITICAL, OBJECTIVE_PRIORITY_HIGH,
    OBJECTIVE_PRIORITY_LOW, OBJECTIVE_PRIORITY_MEDIUM,
};
use crate::military::threatmap::*;
use crate::missions::data::*;
use crate::missions::nuke_defense::*;
use crate::missions::safe_mode::*;
use crate::missions::wall_repair::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::visualization::SummaryContent;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// TTL (ticks) for a `Defend` objective when defense is routed through the
/// `SquadManager` (W1). Short, so a cleared/lost room retires its defense squad
/// quickly — the defense scan re-asserts every 1–2 ticks while the threat stands,
/// so this only needs to exceed that cadence by a comfortable margin.
const DEFEND_OBJECTIVE_TTL: u32 = 60;

/// TTL (ticks) for an offense objective (O6) upserted by the offense scan.
/// `OFFENSE_CADENCE` re-asserts every 10 ticks (stretching to 40 under CPU
/// pressure), so this comfortably outlives the re-assert gap — a cleared room
/// (no core ⇒ no upsert) then lapses and the manager retires the siege squad.
const OFFENSE_OBJECTIVE_TTL: u32 = 100;

// ---------------------------------------------------------------------------
// Target scoring
// ---------------------------------------------------------------------------

/// Why we are targeting a room.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TargetSource {
    /// Manual 'attack' flag placed by player.
    AttackFlag,
    /// Manual 'defend' flag placed by player.
    DefendFlag,
    /// Reactive: hostiles detected in owned room.
    ThreatResponse,
    /// Room we want to expand to but is occupied.
    Expansion,
    /// Deny enemy remote mining income.
    ResourceDenial,
    /// Invader core at a specific level. Level 0 = room reserver, 1-5 = stronghold.
    InvaderCore { level: u8 },
    /// Invader creeps disrupting our remote mining rooms.
    InvaderCreeps,
    /// Power bank farming opportunity.
    PowerBank { power: u32, ticks_to_decay: u32 },
    /// Proactive: enemy activity detected near owned rooms.
    ProactiveDefense,
}

/// A scored attack candidate.
#[derive(Clone, Debug)]
pub struct AttackCandidate {
    pub room: RoomName,
    pub source: TargetSource,
    pub score: f32,
    pub tower_count: u32,
    pub estimated_enemy_dps: f32,
    pub estimated_enemy_heal: f32,
    pub has_safe_mode: bool,
    /// For power banks: estimated ROI (power value vs energy cost).
    pub estimated_roi: Option<f32>,
    /// Target tile for objective-driven offense that needs a position
    /// (e.g. `InvaderCore` → `Dismantle { pos }`). `None` for room-level reasons.
    pub target_pos: Option<Position>,
    /// The target's defense as the force-sizing oracle sees it (ADR 0020 §12) — built for `InvaderCore`
    /// candidates from the room's threat intel + the core; `None` for sources the oracle doesn't gate.
    pub defense: Option<DefenseProfile>,
}

/// Defense escalation level, replacing string-based "Solo"/"Duo"/"Quad".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefenseEscalation {
    /// Single ranged+heal defender.
    Solo,
    /// Attacker + healer pair.
    Duo,
    /// Four creeps in 2x2 formation.
    Quad,
}

impl DefenseEscalation {
    /// Determine escalation level from threat analysis. `pub` so the expansion
    /// escort (ADR 0017, deferred) can size its pre-clear squad with the same
    /// policy war uses for reactive defense.
    pub fn from_threat(estimated_dps: f32, estimated_heal: f32, hostile_count: usize, any_boosted: bool) -> Self {
        if (any_boosted && estimated_dps > 200.0) || (estimated_heal > 100.0 && estimated_dps > 150.0) || hostile_count >= 4 {
            DefenseEscalation::Quad
        } else if estimated_dps > 60.0 || estimated_heal > 20.0 || hostile_count >= 2 || any_boosted {
            DefenseEscalation::Duo
        } else {
            DefenseEscalation::Solo
        }
    }
}

/// Whether a hostile creep in an owned room warrants dispatching a defender.
///
/// `RoomDynamicVisibilityData::hostile_creeps()` only flags Attack /
/// RangedAttack / Work parts, so an enemy **CLAIM creep attacking the
/// controller** — which carries neither — slips through it entirely. In a
/// towerless RCL1-2 room nothing else engages such a creep, so it silently
/// declaims the room. This keys on body parts instead: armed creeps
/// (Attack/RangedAttack), dismantlers (Work), controller-attackers (Claim),
/// and healers sustaining them (Heal) are all worth a defender; pure
/// scouts/haulers (only Move/Carry/Tough) are not — they can't hurt us and
/// don't warrant a spawn. Pure and host-tested.
pub fn hostile_warrants_defender(parts: &[Part]) -> bool {
    parts
        .iter()
        .any(|p| matches!(p, Part::Attack | Part::RangedAttack | Part::Work | Part::Claim | Part::Heal))
}

// ---------------------------------------------------------------------------
// WarOperation
// ---------------------------------------------------------------------------

/// Unified military coordinator singleton. Both offense and defense are produced
/// as objectives on the `CombatObjectiveQueue` (fielded by the `SquadManager`):
/// offense → `Secure`/`Dismantle`/`Harass`, defense → `Defend`. Also owns the
/// non-squad utility missions (NukeDefense, SafeMode, WallRepair). The legacy
/// `AttackOperation`/`AttackMission` offense path was removed in P2.G4-O7.
///
/// Uses tiered cadences:
/// - Defense scan: every 1-2 ticks (cheap, checks owned rooms for threats)
/// - Offense evaluation: every 10-20 ticks (scores candidates, upserts objectives)
/// - Heavy recompute: every 50+ ticks (cap update + border visibility refresh)
#[derive(Clone, ConvertSaveload)]
pub struct WarOperation {
    owner: EntityOption<Entity>,

    /// Tiered cadence tracking (tick numbers).
    last_defense_tick: Option<u32>,
    last_offense_tick: Option<u32>,
    last_recompute_tick: Option<u32>,

    /// Rooms with manually placed 'defend' flags (persisted so we don't
    /// re-scan flags every tick -- just refresh periodically).
    defend_flag_rooms: Vec<RoomName>,

    /// Maximum concurrent attack operations (scales with economy).
    max_concurrent_attacks: u32,
}

// Cadence constants (ticks) — P1.B6 / IBEX-021: every tier ran at 1,
// making war the heaviest per-tick consumer (the review's death-spiral
// contributor). Raised to the values this struct's own doc comment
// always intended; the governor stretches the sheddable tiers further
// under pressure ([`effective_cadence`]).
const DEFENSE_CADENCE: u32 = 2;
const OFFENSE_CADENCE: u32 = 10;
const RECOMPUTE_CADENCE: u32 = 50;

/// Governor-coordinated cadence stretch (ADR 0004 shed order): defense
/// is in the never-shed set and keeps its base cadence at every tier;
/// offense/recompute stretch ×2 under Conserve and ×4 under Critical.
fn effective_cadence(base: u32, tier: crate::cpugovernor::Tier, is_defense: bool) -> u32 {
    if is_defense {
        return base;
    }
    match tier {
        crate::cpugovernor::Tier::Normal => base,
        crate::cpugovernor::Tier::Conserve => base.saturating_mul(2),
        crate::cpugovernor::Tier::Critical => base.saturating_mul(4),
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl WarOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = WarOperation::new(owner);

        builder.with(OperationData::War(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> WarOperation {
        WarOperation {
            owner: owner.into(),
            last_defense_tick: None,
            last_offense_tick: None,
            last_recompute_tick: None,
            defend_flag_rooms: Vec::new(),
            max_concurrent_attacks: 1,
        }
    }

    // ── Defense scan (every 1-2 ticks) ─────────────────────────────────────

    fn run_defense_scan(&mut self, system_data: &mut OperationExecutionSystemData, runtime_data: &mut OperationExecutionRuntimeData) {
        let features = system_data.features;

        if !features.military.defense {
            return;
        }

        // Collect home rooms (rooms with spawns).
        let home_rooms: Vec<Entity> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| {
                rd.get_dynamic_visibility_data().map(|d| d.owner().mine()).unwrap_or(false)
                    && rd.get_structures().map(|s| !s.spawns().is_empty()).unwrap_or(false)
            })
            .map(|(e, _)| e)
            .collect();

        if home_rooms.is_empty() {
            return;
        }

        // ── Collect rooms needing defense ──────────────────────────────────

        struct DefenseNeed {
            room_entity: Entity,
            estimated_dps: f32,
            estimated_heal: f32,
            hostile_count: usize,
            any_boosted: bool,
        }

        struct RoomDefenseState {
            room_entity: Entity,
            has_hostiles: bool,
            has_nukes: bool,
            has_nuke_defense_mission: bool,
            has_safe_mode_mission: bool,
            has_wall_repair_mission: bool,
        }

        let mut room_states: Vec<RoomDefenseState> = Vec::new();

        let rooms_needing_defense: Vec<DefenseNeed> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter_map(|(entity, room_data)| {
                let dynamic_vis = room_data.get_dynamic_visibility_data()?;

                if !dynamic_vis.owner().mine() || !dynamic_vis.visible() {
                    return None;
                }

                let has_hostiles = dynamic_vis.hostile_creeps();
                // `hostile_creeps()` only flags Attack/RangedAttack/Work, so an
                // enemy CLAIM creep neutralising the controller (or a lone
                // dismantler/healer) is invisible to it. In a towerless RCL1-2
                // room that creep declaims us with no response. `hostile_threat_creeps()`
                // flags any hostile with a non-Move/Tough part — a safe superset
                // pre-filter that catches the controller-attacker; the precise
                // "worth a defender" call is made on body parts below. Safe-mode /
                // wall-repair (room_states) stay keyed on a real armed assault.
                let has_threat = has_hostiles || dynamic_vis.hostile_threat_creeps();

                let has_nukes = room_data.get_nukes().map(|n| n.has_incoming()).unwrap_or(false);

                let missions = room_data.get_missions();
                let has_nuke_defense_mission = missions
                    .iter()
                    .any(|me| system_data.mission_data.get(*me).as_mission_type::<NukeDefenseMission>().is_some());
                let has_safe_mode_mission = missions
                    .iter()
                    .any(|me| system_data.mission_data.get(*me).as_mission_type::<SafeModeMission>().is_some());
                let has_wall_repair_mission = missions
                    .iter()
                    .any(|me| system_data.mission_data.get(*me).as_mission_type::<WallRepairMission>().is_some());

                room_states.push(RoomDefenseState {
                    room_entity: entity,
                    has_hostiles,
                    has_nukes,
                    has_nuke_defense_mission,
                    has_safe_mode_mission,
                    has_wall_repair_mission,
                });

                if !has_threat {
                    return None;
                }

                let creeps = room_data.get_creeps()?;
                // Defend an OWNED room against players AND NPC invaders — an
                // invader assault wrecks a towerless young colony just as a
                // player raid does, and the separate invader path below only
                // covers RESERVED remote rooms, never owned ones. Source
                // Keepers are excluded (permanent residents that never leave
                // their lair to attack a colony).
                let hostiles: Vec<_> = creeps
                    .hostile()
                    .iter()
                    .filter(|c| !crate::military::is_source_keeper_owner(&c.owner().username()))
                    .collect();

                if hostiles.is_empty() {
                    return None;
                }

                // Commit a defender only to hostiles that actually threaten the
                // room — armed creeps, dismantlers, controller-attackers (CLAIM),
                // or their healers. Ignore transient unarmed scouts/haulers so we
                // don't burn a spawn on a creep just passing through.
                let worth_defending = hostiles.iter().any(|c| {
                    let parts: Vec<Part> = c.body().iter().filter(|p| p.hits() > 0).map(|p| p.part()).collect();
                    hostile_warrants_defender(&parts)
                });

                if !worth_defending {
                    return None;
                }

                // No de-dup guard needed: the `Defend` objective upsert below is
                // idempotent (keyed by room), so re-asserting each scan is safe.

                let mut estimated_dps: f32 = 0.0;
                let mut estimated_heal: f32 = 0.0;
                let mut any_boosted = false;

                for hostile in &hostiles {
                    for part_info in hostile.body().iter() {
                        if part_info.hits() == 0 {
                            continue;
                        }
                        if part_info.boost().is_some() {
                            any_boosted = true;
                        }
                        match part_info.part() {
                            Part::Attack => estimated_dps += 30.0,
                            Part::RangedAttack => estimated_dps += 10.0,
                            Part::Heal => estimated_heal += 12.0,
                            _ => {}
                        }
                    }
                }

                Some(DefenseNeed {
                    room_entity: entity,
                    estimated_dps,
                    estimated_heal,
                    hostile_count: hostiles.len(),
                    any_boosted,
                })
            })
            .collect();

        // ── Create squad defense missions ──────────────────────────────────

        // Defense is produced as a `Defend` objective on the CombatObjectiveQueue
        // (ADR 0008 §W1/§5) — the `SquadManager` claims it, fields the threat-sized
        // squad with full G3 tactics (focus-fire / heal / cohesion), and retires it
        // when this producer stops re-asserting. The producer re-asserts every scan
        // while the (owned, visible) room warrants a defender; the gather scoping
        // (`owner().mine() && visible()`) preserves the ADR 0017 §13 ownership-
        // subordinate invariant — a lost room drops out, its TTL lapses, the squad
        // retires. (Replaces the removed squad-less `SquadDefenseMission`.)
        for need in rooms_needing_defense {
            let escalation = DefenseEscalation::from_threat(need.estimated_dps, need.estimated_heal, need.hostile_count, need.any_boosted);
            let room_name = match system_data.room_data.get(need.room_entity) {
                Some(rd) => rd.name,
                None => continue,
            };
            let composition = match escalation {
                DefenseEscalation::Quad => SquadComposition::quad_ranged(),
                DefenseEscalation::Duo => SquadComposition::duo_attack_heal(),
                DefenseEscalation::Solo => SquadComposition::solo_ranged(),
            };
            info!(
                "[War] Defend objective for owned room {} ({:?}, dps={:.0}, heal={:.0}, count={})",
                room_name, escalation, need.estimated_dps, need.estimated_heal, need.hostile_count
            );
            // Owned-room defense is CRITICAL — our base is under attack. Under the
            // manager's concurrency cap it must out-rank operator defend-flags (HIGH)
            // and remote-invader cleanup (MEDIUM) so a far owned room is never starved
            // by a lower-value defense (the equal-HIGH-priority starvation the review
            // flagged when all three contexts funnel through one capped manager).
            system_data.combat_objective_queue.request(
                ObjectiveRequest::new(
                    ObjectiveKind::Defend { room: room_name },
                    OBJECTIVE_PRIORITY_CRITICAL,
                    ForceRequirement::single(composition),
                )
                .owner(ObjectiveOwner::Defense)
                .ttl(DEFEND_OBJECTIVE_TTL),
                game::time(),
            );
        }

        // ── Nuke defense, safe mode, wall repair (home rooms only) ──────────
        // Only create these missions for rooms we control (have spawns). This
        // avoids running wall repair / safe mode / nuke defense in owned rooms
        // we don't want to manage (e.g. no spawns, or abandoned).
        let home_set: std::collections::HashSet<Entity> = home_rooms.iter().copied().collect();

        for state in room_states {
            if !home_set.contains(&state.room_entity) {
                continue;
            }

            let room_data = match system_data.room_data.get_mut(state.room_entity) {
                Some(rd) => rd,
                None => continue,
            };

            if state.has_nukes && !state.has_nuke_defense_mission && features.military.nuke_defense {
                info!("[War] Creating NukeDefenseMission for room: {}", room_data.name);
                let mission_entity = NukeDefenseMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    state.room_entity,
                )
                .build();
                room_data.add_mission(mission_entity);
            }

            if state.has_hostiles && !state.has_safe_mode_mission && features.military.safe_mode {
                info!("[War] Creating SafeModeMission for room: {}", room_data.name);
                let mission_entity = SafeModeMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    state.room_entity,
                )
                .build();
                room_data.add_mission(mission_entity);
            }

            if state.has_hostiles && !state.has_wall_repair_mission {
                info!("[War] Creating WallRepairMission for room: {}", room_data.name);
                let mission_entity = WallRepairMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    state.room_entity,
                )
                .build();
                room_data.add_mission(mission_entity);
            }
        }

        // ── Defend flags ───────────────────────────────────────────────────

        let mut defend_rooms: Vec<RoomName> = Vec::new();
        for flag in game::flags().values() {
            let name = flag.name();
            if name.to_lowercase().starts_with("defend") {
                defend_rooms.push(flag.pos().room_name());
            }
        }
        self.defend_flag_rooms = defend_rooms;

        // Operator `defend`-flag rooms → a `Defend` objective (duo). Re-asserted
        // each scan while the flag is present; addressed by RoomName so it works
        // even for a room we have no `RoomData` entity for yet (the squad travels
        // there). The manager retires it when the flag is removed (TTL lapse).
        for &defend_room in &self.defend_flag_rooms {
            system_data.combat_objective_queue.request(
                ObjectiveRequest::new(
                    ObjectiveKind::Defend { room: defend_room },
                    OBJECTIVE_PRIORITY_HIGH,
                    ForceRequirement::single(SquadComposition::duo_attack_heal()),
                )
                .owner(ObjectiveOwner::Defense)
                .ttl(DEFEND_OBJECTIVE_TTL),
                game::time(),
            );
        }

        // ── Remote room defense (invader creeps in reserved rooms) ────────
        // Invader creeps in rooms we've reserved disrupt remote mining.
        // Spawn a solo or duo defense depending on invader strength.

        let remote_rooms_with_invaders: Vec<(Entity, f32, f32, usize)> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter_map(|(entity, room_data)| {
                let dynamic_vis = room_data.get_dynamic_visibility_data()?;

                // Only defend rooms we've reserved (our remote mining rooms).
                if !dynamic_vis.reservation().mine() || !dynamic_vis.visible() {
                    return None;
                }

                if !dynamic_vis.hostile_creeps() {
                    return None;
                }

                // No de-dup guard: the `Defend` objective upsert below is idempotent.

                let creeps = room_data.get_creeps()?;
                // Only count actual Invader NPCs, not Source Keepers.
                // Source Keepers are permanent residents and should not
                // trigger defensive responses.
                let invaders: Vec<_> = creeps
                    .hostile()
                    .iter()
                    .filter(|c| crate::military::is_invader_owner(&c.owner().username()))
                    .collect();

                if invaders.is_empty() {
                    return None;
                }

                let mut dps: f32 = 0.0;
                let mut heal: f32 = 0.0;
                for inv in &invaders {
                    for part_info in inv.body().iter() {
                        if part_info.hits() == 0 {
                            continue;
                        }
                        match part_info.part() {
                            Part::Attack => dps += 30.0,
                            Part::RangedAttack => dps += 10.0,
                            Part::Heal => heal += 12.0,
                            _ => {}
                        }
                    }
                }

                Some((entity, dps, heal, invaders.len()))
            })
            .collect();

        // Invader creeps in a RESERVED remote → a `Defend` objective. NOTE: this
        // is the path that previously thrashed — a squad-less SquadDefenseMission
        // on a reserved (not owned) remote self-terminated every tick via the
        // ADR 0017 ownership-subordinate guard (`!owner().mine()`), so the producer
        // re-created it endlessly. As an objective there is no mission-internal
        // ownership self-termination; the producer re-asserts while invaders are
        // present and the manager retires the squad (TTL lapse) once they're gone.
        for (room_entity, dps, heal, count) in remote_rooms_with_invaders {
            let room_name = match system_data.room_data.get(room_entity) {
                Some(rd) => rd.name,
                None => continue,
            };
            // Escalate based on invader strength.
            let escalation = if dps > 100.0 || heal > 30.0 || count >= 3 {
                DefenseEscalation::Duo
            } else {
                DefenseEscalation::Solo
            };
            let composition = match escalation {
                DefenseEscalation::Duo => SquadComposition::duo_attack_heal(),
                _ => SquadComposition::solo_ranged(),
            };
            info!(
                "[War] Defend objective for remote room {} ({:?}, dps={:.0}, heal={:.0}, count={})",
                room_name, escalation, dps, heal, count
            );
            // Remote-invader cleanup is MEDIUM — below owned-room defense (CRITICAL)
            // and operator defend-flags (HIGH), above SK farming (LOW). So under the
            // concurrency cap, protecting our base + honoring operator intent comes
            // first, and a remote skirmish never starves them.
            system_data.combat_objective_queue.request(
                ObjectiveRequest::new(
                    ObjectiveKind::Defend { room: room_name },
                    OBJECTIVE_PRIORITY_MEDIUM,
                    ForceRequirement::single(composition),
                )
                .owner(ObjectiveOwner::Defense)
                .ttl(DEFEND_OBJECTIVE_TTL),
                game::time(),
            );
        }
    }

    // ── Offense evaluation (every 10-20 ticks) ────────────────────────────

    fn run_offense_evaluation(&mut self, system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {
        let features = system_data.features;

        if !features.military.offense {
            if features.military.debug_log {
                info!("[War] Offense disabled by feature flag");
            }
            return;
        }

        // Spawn pressure gate: don't attack if all spawns are busy.
        if system_data.economy.total_free_spawns == 0 {
            if features.military.debug_log {
                info!("[War] Offense skipped -- no free spawns");
            }
            return;
        }

        // Collect home rooms (entity + name) for distance scoring and spawn assignment.
        let home_room_entries: Vec<(Entity, RoomName)> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| {
                rd.get_dynamic_visibility_data().map(|d| d.owner().mine()).unwrap_or(false)
                    && rd.get_structures().map(|s| !s.spawns().is_empty()).unwrap_or(false)
            })
            .map(|(e, rd)| (e, rd.name))
            .collect();

        if home_room_entries.is_empty() {
            return;
        }

        let home_rooms: Vec<RoomName> = home_room_entries.iter().map(|(_, name)| *name).collect();

        let mut candidates: Vec<AttackCandidate> = Vec::new();

        // ── 1. Manual attack flags (highest priority) ────────────────────

        for flag in game::flags().values() {
            let name = flag.name();
            if name.to_lowercase().starts_with("attack") {
                let room = flag.pos().room_name();
                candidates.push(AttackCandidate {
                    room,
                    source: TargetSource::AttackFlag,
                    score: 100.0,
                    tower_count: 0,
                    estimated_enemy_dps: 0.0,
                    estimated_enemy_heal: 0.0,
                    has_safe_mode: false,
                    estimated_roi: None,
                    target_pos: None,
                    defense: None,
                });
            }
        }

        // ── 2. Scan room threat data for automatic targets ─────────────

        let current_tick = game::time();
        let war_debug = features.military.debug_log;

        // Collect rooms with threat data for iteration (avoids borrow conflicts
        // with system_data.room_data which is &mut).
        let threat_rooms: Vec<(Entity, RoomName, RoomThreatData)> =
            (system_data.entities, &*system_data.room_data, system_data.threat_data)
                .join()
                .map(|(e, rd, td)| (e, rd.name, td.clone()))
                .collect();

        if war_debug {
            info!(
                "[War] Offense scan: {} threat rooms (offense-objective cap {}), economy={}",
                threat_rooms.len(),
                self.max_concurrent_attacks,
                system_data.economy.total_stored_energy
            );
        }

        for (room_entity, room_name, threat_data) in &threat_rooms {
            let room_name = *room_name;
            let room_entity = Some(*room_entity);

            // (No "already attacking" skip needed — offense objectives are upserted
            // idempotently, so re-evaluating a room just refreshes its objective.)

            // Skip rooms we own (defense handles those).
            let is_owned = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|d| d.owner().mine())
                .unwrap_or(false);
            if is_owned {
                continue;
            }

            // Compute minimum distance from any home room.
            let min_distance = self.min_distance_to_homes(room_name, &home_rooms, system_data.pathfinder, current_tick);

            // Skip rooms that are too far away (> 10 hops) — not worth re-scouting either.
            if min_distance > 10 {
                if war_debug {
                    info!("[War]   Skip {} -- too far (distance={})", room_name, min_distance);
                }
                continue;
            }

            // Stale data (older than 200 ticks) on an in-range room we last saw long ago: REGISTER a
            // re-scout on the central visibility queue (do NOT dispatch a scout ourselves), then skip
            // this scan. So a core that deployed — or towers that energized — since our last visit get
            // re-evaluated once fresh intel lands, instead of being silently abandoned (the W5N3 soak
            // gap). Fulfillment is observer-preferred for free: an in-range RCL8 observer covers it with
            // no creep; a scout is spawned only if no observer covers it and we're under the mission cap
            // (and walled/defended rooms back off scouts but keep observer coverage). Mirrors
            // salvage.rs::request_intel (register-don't-dispatch). FOLLOW-UP (deeper, not done here): an
            // explicit per-tier re-scout *scheduler* owning the cadence + OBSERVE-only registration for
            // rooms confirmed in observer range — see docs/design/0021-strategic-visibility.md.
            if current_tick.saturating_sub(threat_data.last_seen) > 200 {
                system_data
                    .visibility
                    .request(VisibilityRequest::new(room_name, VISIBILITY_PRIORITY_MEDIUM, VisibilityRequestFlags::ALL));
                if war_debug {
                    info!(
                        "[War]   Skip {} -- stale data (age={}); requested re-scout",
                        room_name,
                        current_tick.saturating_sub(threat_data.last_seen)
                    );
                }
                continue;
            }

            // Check for invader cores in the room. `None` = no core present;
            // `Some(0)` is a level-0 "reserver" core NPC-reserving the room.
            // Presence must stay distinguishable from absence: collapsing
            // both to 0 made reserver cores untargetable, so they squatted
            // our remote-mining rooms until their collapse timer expired.
            // Capture the highest-level core's level *and* position — O6 fields a
            // `Dismantle { room, pos }` objective at the core, so it needs the tile.
            let invader_core = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_structures())
                .and_then(|structures| {
                    structures
                        .invader_cores()
                        .iter()
                        .max_by_key(|core| core.level())
                        .map(|core| (core.level(), core.pos(), core.hits()))
                });
            let invader_core_level = invader_core.map(|(level, _, _)| level);

            // Check for power banks.
            let power_bank_info = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_structures())
                .and_then(|structures| structures.power_banks().first().map(|pb| (pb.power(), pb.ticks_to_decay())));

            // Check room ownership for player targeting.
            let room_owner_hostile = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|d| d.owner().hostile())
                .unwrap_or(false);

            let has_safe_mode = threat_data.safe_mode_active || threat_data.safe_mode_available;

            let tower_count = threat_data.hostile_tower_positions.len() as u32;

            if war_debug {
                info!(
                    "[War]   Evaluating {} (dist={}, threat={:?}, dps={:.0}, heal={:.0}, towers={}, core_lvl={}, power_bank={}, hostile_owner={}, safe_mode={})",
                    room_name, min_distance, threat_data.threat_level,
                    threat_data.estimated_dps, threat_data.estimated_heal,
                    tower_count,
                    invader_core_level.map(|l| l.to_string()).unwrap_or_else(|| "none".to_string()),
                    power_bank_info.map(|(p, d)| format!("{}pw/{}t", p, d)).unwrap_or_else(|| "none".to_string()),
                    room_owner_hostile, has_safe_mode
                );
            }

            // ── Invader core targeting ────────────────────────────────────
            // Migrated to an objective (O6): the candidate carries the core's tile
            // and the launch loop upserts a `Dismantle { room, pos }` instead of
            // launching an `AttackOperation` — see the launch loop's source→objective
            // mapping. The affordability/interest gate is preserved here.
            if let Some((core_level, core_pos, core_hits)) = invader_core {
                if features.military.attack_invaders {
                    let is_our_remote = room_entity
                        .and_then(|e| system_data.room_data.get(e))
                        .and_then(|rd| rd.get_dynamic_visibility_data())
                        .map(|d| d.reservation().mine())
                        .unwrap_or(false);

                    let has_sources = room_entity
                        .and_then(|e| system_data.room_data.get(e))
                        .and_then(|rd| rd.get_static_visibility_data())
                        .map(|s| !s.sources().is_empty())
                        .unwrap_or(false);

                    if let Some(score) = invader_core_attack_score(
                        core_level,
                        min_distance,
                        system_data.economy.total_stored_energy,
                        is_our_remote,
                        has_sources,
                    ) {
                        // The defense the force-sizing oracle weighs (ADR 0020 §12). Tower ranges are
                        // measured to the core (the assault tile) — conservative, since towers cluster
                        // near it. Unknown per-tower energy (stale intel) ⇒ assume firing (a high value),
                        // never under-estimating the threat.
                        let towers: Vec<TowerThreat> = threat_data
                            .hostile_tower_positions
                            .iter()
                            .enumerate()
                            .map(|(i, tpos)| TowerThreat {
                                range_to_assault: tpos.get_range_to(core_pos),
                                energy: threat_data.tower_energy.get(i).copied().unwrap_or(1000),
                            })
                            .collect();
                        let defense = DefenseProfile {
                            towers,
                            breach_hits: threat_data.breach_rampart_hits,
                            objective_hits: core_hits,
                            enemy_dps: threat_data.estimated_dps,
                            repair_per_tick: threat_data.repair_per_tick as f32,
                            safe_mode: threat_data.safe_mode_active,
                        };
                        candidates.push(AttackCandidate {
                            room: room_name,
                            source: TargetSource::InvaderCore { level: core_level },
                            score,
                            tower_count,
                            estimated_enemy_dps: threat_data.estimated_dps,
                            estimated_enemy_heal: threat_data.estimated_heal,
                            has_safe_mode: false,
                            estimated_roi: None,
                            target_pos: Some(core_pos),
                            defense: Some(defense),
                        });
                    }
                }
            }

            // ── Power banks: intentionally NOT farmed (O5, 2026-06-18) ────
            // Power-bank farming was non-functional — the neutral bank is never
            // targeted by the combat decision (`get_hostile_structures` excludes
            // unowned structures; `select_focus_target` only picks hostile ones),
            // and there is no dropped-power collector — so the offense scan no
            // longer produces a candidate for it (it only wasted a duo + haulers
            // idling in a highway room). Real power-bank farming is a deferred
            // workstream: it needs a DEDICATED healed squad (the bank deals
            // `damage × POWER_BANK_HIT_BACK` back to attackers, so unhealed creeps
            // die) + a PREDICTIVE dropped-power collector timed to the crack. See
            // the master plan doc §5 and the pending power-bank ADR.
            // (`power_bank_info` is still surfaced in the diagnostics line above.)

            // ── Invader creeps in remote rooms (RECONCILED — no offense path) ──
            // Invader creeps disrupting a reserved remote are cleared by the
            // remote-invader `Defend` context in `run_defense_scan` (same trigger:
            // `reservation().mine() && visible() && hostile invaders`). Producing a
            // `Secure` objective here too would double-field the same room, so the
            // O6 migration DROPS the InvaderCreeps offense path. We still compute
            // `all_npc` — the resource-denial block below needs it to exclude
            // NPC-only rooms (those are policed, not contested as player targets).
            let all_npc = threat_data.hostile_creeps.iter().all(|c| crate::military::is_npc_owner(&c.owner));

            // ── Hostile player rooms (resource denial / expansion) ───────
            if room_owner_hostile && features.military.attack_players && !all_npc && threat_data.threat_level >= ThreatLevel::PlayerScout {
                // Only target hostile player rooms if we have strong economy
                // and the room is close enough to be worth contesting.
                if system_data.economy.total_stored_energy > 150_000 && min_distance <= 6 {
                    let distance_penalty = min_distance as f32 * 4.0;
                    let tower_penalty = tower_count as f32 * 5.0;
                    let safe_mode_penalty = if has_safe_mode { 20.0 } else { 0.0 };
                    let score = 40.0 - distance_penalty - tower_penalty - safe_mode_penalty;

                    if score > 0.0 {
                        candidates.push(AttackCandidate {
                            room: room_name,
                            source: TargetSource::ResourceDenial,
                            score,
                            tower_count,
                            estimated_enemy_dps: threat_data.estimated_dps,
                            estimated_enemy_heal: threat_data.estimated_heal,
                            has_safe_mode,
                            estimated_roi: None,
                            target_pos: None,
                            defense: None,
                        });
                    }
                }
            }
        }

        // ── 3. Deduplicate: keep highest-scored candidate per room ───────

        candidates.sort_by(|a, b| {
            a.room
                .cmp(&b.room)
                .then_with(|| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
        });
        candidates.dedup_by_key(|c| c.room);

        // Sort by score descending for launch priority.
        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        if war_debug {
            if candidates.is_empty() {
                info!("[War] No attack candidates after analysis");
            } else {
                info!("[War] {} attack candidate(s) after dedup:", candidates.len());
                for (i, c) in candidates.iter().enumerate() {
                    info!(
                        "[War]   #{}: {} score={:.1} source={:?} towers={} dps={:.0} heal={:.0} safe_mode={} roi={:?}",
                        i + 1,
                        c.room,
                        c.score,
                        c.source,
                        c.tower_count,
                        c.estimated_enemy_dps,
                        c.estimated_enemy_heal,
                        c.has_safe_mode,
                        c.estimated_roi
                    );
                }
            }
        }

        // ── 4. Field top candidates as offense objectives ──

        // Concurrent-offense budget = the count of Attack-owned objectives already
        // in the queue (all offense is objective-driven since O7; the manager fields
        // them up to its own MAX_CONCURRENT_SQUADS).
        let mut offense_count = system_data
            .combat_objective_queue
            .objectives
            .iter()
            .filter(|o| o.owner == ObjectiveOwner::Attack)
            .count() as u32;

        for candidate in candidates {
            // Map the candidate's reason to an offense objective kind + composition
            // + priority. Any source without a mapping is skipped (all offense is
            // objective-driven since O7 — there is no legacy launch fallback).
            let objective: Option<(ObjectiveKind, f32, SquadComposition)> = match candidate.source {
                // Invader core → ATTACK the core tile. A `StructureInvaderCore` is IMMUNE to dismantle
                // (the engine dismantle intent no-ops on any structure without a `CONSTRUCTION_COST`),
                // so it must be hit with ATTACK/RANGED_ATTACK. We field a ranged quad (`quad_ranged`),
                // NOT the WORK `siege_quad` whose dismantlers cannot damage the core. The squad's Engaged
                // seam (`decide_combat`) targets the core as a Hostile structure and shoots it; for a
                // deployed stronghold the ranged attackers also shoot down the ramparts before the core.
                //
                // WINNABILITY GATE (ADR 0020 §12): the force-sizing oracle weighs the real defense —
                // energized towers (drained ones deal 0), tower damage at the core, and out-heal
                // feasibility — against what one squad fields at our RCL within its on-site lifetime.
                // Winnable ⇒ engage (the oracle force-SIZES the HEALERS to out-heal the towers; the
                // ranged attackers stay template); unwinnable ⇒ skip (defer to G4-HEAVY) so we never
                // feed a squad to its death. FOLLOW-UP (docs/design/0020 §12.6 R-attack): force-size the
                // RANGED attack parts to the core's hits (the deferred R6 `ranged_parts`) so kill-time is
                // sized, not template.
                TargetSource::InvaderCore { .. } => {
                    let comp = SquadComposition::quad_ranged();
                    match (candidate.target_pos, candidate.defense.as_ref()) {
                        (Some(pos), Some(defense)) => {
                            match best_force_budget(&comp, &home_rooms, candidate.room, system_data.pathfinder) {
                                Some((budget, member_energy)) => {
                                    let a = assess(defense, &budget);
                                    if !a.winnable {
                                        info!(
                                            "[War]   Skip {} -- force oracle: not winnable for one squad ({}); defer to G4-HEAVY",
                                            candidate.room, a.reason
                                        );
                                        None
                                    } else {
                                        // R3: force-SIZE the squad's HEALERS to the Lanchester-winning
                                        // out-heal (the ranged attackers stay the `quad_ranged` template
                                        // until the R-attack follow-up sizes ranged parts), at the same
                                        // energy the spawn path uses. `None` ⇒ a home can't afford the
                                        // required heal ⇒ defer (G4-HEAVY).
                                        //
                                        // R5: over-invest by the objective's importance (a MEDIUM core lifts
                                        // the base hold-margin force ~1.17×) so higher-value targets field a
                                        // higher-P(win) squad. R4: log the fielded force's win confidence.
                                        let importance = ((OBJECTIVE_PRIORITY_MEDIUM - OBJECTIVE_PRIORITY_LOW)
                                            / (OBJECTIVE_PRIORITY_CRITICAL - OBJECTIVE_PRIORITY_LOW))
                                            .clamp(0.0, 1.0);
                                        let required = RequiredForce::from_assessment(&a).scaled(importance_margin(importance));
                                        match comp.sized_for(required, member_energy) {
                                            Some(sized) => {
                                                let pwin = win_probability(
                                                    required.heal_parts as f32 * 12.0,
                                                    a.required_heal_per_tick / HOLD_MARGIN,
                                                );
                                                info!(
                                                    "[War]   {} winnable via {:?} (~{} ticks): ranged quad, healers sized to {} heal parts (out-heal towers), P(win)~{:.0}% ({})",
                                                    candidate.room, a.mode, a.est_ticks, required.heal_parts, pwin * 100.0, a.reason
                                                );
                                                Some((ObjectiveKind::Dismantle { room: candidate.room, pos }, OBJECTIVE_PRIORITY_MEDIUM, sized))
                                            }
                                            None => {
                                                info!(
                                                    "[War]   Skip {} -- can't afford the required {} heal parts (out-heal towers) at {} energy; defer",
                                                    candidate.room, required.heal_parts, member_energy
                                                );
                                                None
                                            }
                                        }
                                    }
                                }
                                None => {
                                    info!("[War]   Skip {} -- no home room can reach it within a creep lifetime", candidate.room);
                                    None
                                }
                            }
                        }
                        _ => None,
                    }
                }
                // Operator attack flag → clear the room (HIGH: explicit operator intent).
                TargetSource::AttackFlag => Some((
                    ObjectiveKind::Secure { room: candidate.room },
                    OBJECTIVE_PRIORITY_HIGH,
                    SquadComposition::quad_ranged(),
                )),
                // Resource denial → harass a hostile player's remote (LOW: opportunistic).
                TargetSource::ResourceDenial => Some((
                    ObjectiveKind::Harass { room: candidate.room },
                    OBJECTIVE_PRIORITY_LOW,
                    SquadComposition::solo_harasser(),
                )),
                _ => None,
            };

            let Some((kind, priority, composition)) = objective else {
                continue;
            };

            // Always re-assert an EXISTING objective (refresh its TTL so the manager
            // keeps fielding it); gate only NEW offense on the cap. Candidates are
            // score-sorted desc, so a skipped new objective is the lowest-value one.
            let is_new = system_data.combat_objective_queue.find_by_kind(&kind).is_none();
            if is_new && offense_count >= self.max_concurrent_attacks {
                continue;
            }
            info!(
                "[War] Offense objective {:?} for {} (source={:?}, score={:.1})",
                kind, candidate.room, candidate.source, candidate.score
            );
            system_data.combat_objective_queue.request(
                ObjectiveRequest::new(kind, priority, ForceRequirement::single(composition))
                    .owner(ObjectiveOwner::Attack)
                    .ttl(OFFENSE_OBJECTIVE_TTL),
                game::time(),
            );
            if is_new {
                offense_count += 1;
            }
        }
    }

    // ── Heavy recompute (every 50+ ticks) ─────────────────────────────────

    fn run_heavy_recompute(&mut self, system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {
        // ── 1. Update the offense-objective cap based on economy ──────────

        // Scale capacity with room count and economy health.
        let base_attacks = system_data.economy.room_count.max(1);
        let economy_multiplier = if system_data.economy.total_stored_energy > 300_000 {
            2
        } else if system_data.economy.total_stored_energy > 100_000 {
            1
        } else {
            0
        };
        self.max_concurrent_attacks = base_attacks.saturating_sub(1).max(1) + economy_multiplier;

        // ── 2. Request visibility for rooms adjacent to our territory ────
        // This ensures threat data is fresh for rooms near our
        // borders, feeding the defense + offense scans' target selection.

        let home_rooms: Vec<RoomName> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| rd.get_dynamic_visibility_data().map(|d| d.owner().mine()).unwrap_or(false))
            .map(|(_, rd)| rd.name)
            .collect();

        // Request visibility for rooms adjacent to home rooms that have
        // stale or no visibility data (older than 100 ticks).
        for &home_room in &home_rooms {
            let home_entity = system_data.mapping.get_room(&home_room);
            let exits = home_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_static_visibility_data())
                .and_then(|s| s.exits())
                .cloned();

            if let Some(exits) = exits {
                for (_, neighbor_room) in &exits {
                    let neighbor_entity = system_data.mapping.get_room(neighbor_room);
                    let is_stale = neighbor_entity
                        .and_then(|e| system_data.room_data.get(e))
                        .and_then(|rd| rd.get_dynamic_visibility_data())
                        .map(|d| d.age() > 100)
                        .unwrap_or(true);

                    if is_stale {
                        system_data.visibility.request(VisibilityRequest::new_opportunistic(
                            *neighbor_room,
                            0.5,
                            VisibilityRequestFlags::ALL,
                        ));
                    }
                }
            }
        }

    }

    // ── Helpers ────────────────────────────────────────────────────────────

    /// Compute the minimum route distance from any home room to a target room.
    fn min_distance_to_homes(
        &self,
        target: RoomName,
        home_rooms: &[RoomName],
        pathfinder: &mut crate::pathing::pathfinderservice::PathfinderService,
        current_tick: u32,
    ) -> u32 {
        home_rooms
            .iter()
            .map(|&home| {
                let route = pathfinder.route_distance(home, target, current_tick);
                if route.reachable {
                    route.hops
                } else {
                    u32::MAX
                }
            })
            .min()
            .unwrap_or(u32::MAX)
    }

    fn should_run_tier(&self, last_tick: Option<u32>, cadence: u32) -> bool {
        cadence_elapsed(game::time(), last_tick, cadence)
    }
}

/// Whether `cadence` ticks have elapsed since `last_tick` (or it never ran).
///
/// Uses `saturating_sub` so a persisted tick from the "future" (private
/// server time reset, restored snapshot) yields "not elapsed yet" instead of
/// a u32 underflow, which would abort the tick under panic="abort"
/// (IBEX-044).
fn cadence_elapsed(now: u32, last_tick: Option<u32>, cadence: u32) -> bool {
    last_tick.map(|t| now.saturating_sub(t) >= cadence).unwrap_or(true)
}

/// Score an invader core as an attack candidate; `None` = don't attack.
///
/// `core_level` is the strongest core *present* in the room -- absence is the
/// caller's `Option`, never level 0. Level 0 is the "reserver" core that
/// NPC-reserves remote rooms (engine: cores collapse after 75k ±10% ticks).
/// Collapsing absence and level 0 to the same value previously made reserver
/// cores untargetable, so they sat NPC-reserving our remote-mining rooms
/// (which also aborts `MiningOutpostMission` via its hostile-reservation
/// gate) until the collapse timer expired.
///
/// Reserver cores carry no loot, so one is only worth killing when it blocks
/// a room we want to reserve/mine: a room whose reservation is ours, or a
/// source room within remote-mining range. The source-room fallback is
/// load-bearing -- the core *evicts* our reservation, so `is_our_remote` is
/// false in exactly the rooms it blocks. Strongholds (level 1+) carry loot
/// and are worth attacking anywhere in range, economy permitting. Cores must
/// be killed with ATTACK parts -- the engine rejects `dismantle` on them.
fn invader_core_attack_score(
    core_level: u8,
    min_distance: u32,
    total_stored_energy: u32,
    is_our_remote: bool,
    has_sources: bool,
) -> Option<f32> {
    // Only attack cores we can handle. Level 0 = reserver (easy),
    // levels 1-5 = stronghold (increasingly hard).
    let max_affordable_level = if total_stored_energy > 200_000 {
        5
    } else if total_stored_energy > 100_000 {
        3
    } else if total_stored_energy > 30_000 {
        1
    } else {
        0
    };

    if core_level > max_affordable_level {
        return None;
    }

    /// `MiningOutpostOperation::run_operation` gathers outpost candidates at
    /// BFS distance 1 from home rooms; keep in sync.
    const REMOTE_MINE_RANGE: u32 = 1;

    let wanted_remote = is_our_remote || (has_sources && min_distance <= REMOTE_MINE_RANGE);

    if core_level == 0 && !wanted_remote {
        // Deliberate exclusion: a reserver core in a room we don't mine
        // costs us nothing and pays no loot -- let it expire on its own.
        return None;
    }

    // Score: higher for lower levels (easier), closer rooms, and rooms we
    // have interest in.
    let base_score = if wanted_remote { 60.0 } else { 30.0 };
    let level_penalty = core_level as f32 * 5.0;
    let distance_penalty = min_distance as f32 * 3.0;
    let score = base_score - level_penalty - distance_penalty;

    (score > 0.0).then_some(score)
}

/// The best (longest on-site) [`ForceBudget`] for launching `comp` at `target` from any home room
/// (ADR 0020 §12.2): on-site ticks = `CREEP_LIFE_TIME − spawn − travel` (the operator's "creep
/// lifetime minus travel"), via [`SquadComposition::estimated_combat_time`]; capabilities auto-size to
/// the launching room's energy. Picks the home that yields the most on-site time (the manager will
/// likewise field from a viable in-range home). `None` if no home can reach the target.
fn best_force_budget(
    comp: &SquadComposition,
    home_rooms: &[RoomName],
    target: RoomName,
    pathfinder: &mut crate::pathing::pathfinderservice::PathfinderService,
) -> Option<(ForceBudget, u32)> {
    let mut best: Option<(ForceBudget, u32)> = None;
    for &home in home_rooms {
        let Some(room) = game::rooms().get(home) else {
            continue;
        };
        let energy_capacity = room.energy_capacity_available();
        let spawns = room.find(find::MY_SPAWNS, None).len().max(1) as u32;
        let Some(onsite) = comp.estimated_combat_time(pathfinder, home, target, energy_capacity, spawns) else {
            continue;
        };
        let caps = comp.capabilities(energy_capacity);
        let budget = ForceBudget {
            max_heal_per_tick: caps.heal_per_tick as f32,
            max_dismantle_dps: caps.structure_dps as f32,
            tank_effective_hp: caps.tank_effective_hp as f32,
            onsite_budget_ticks: onsite,
        };
        // Return the chosen home's energy too — R3 sizes the fielded composition at the SAME energy the
        // spawn path will use, so the affordability check and the actual spawn agree.
        if best.map(|(b, _)| onsite > b.onsite_budget_ticks).unwrap_or(true) {
            best = Some((budget, energy_capacity));
        }
    }
    best
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for WarOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn describe_operation(&self, ctx: &OperationDescribeContext) -> SummaryContent {
        let features = ctx.features;
        let mut children = Vec::new();

        // Offense section. Offense is now objective-driven (Secure/Dismantle/Harass
        // on the CombatObjectiveQueue, fielded by the SquadManager) — war no longer
        // tracks AttackOperations, so this reports the cap + on/off only.
        {
            let label = if features.military.offense {
                format!("Offense: ON (objective cap {})", self.max_concurrent_attacks)
            } else {
                "Offense: OFF".to_string()
            };
            children.push(SummaryContent::Text(label));
        }

        // Defense section.
        {
            let mut defense_items = Vec::new();
            for room in &self.defend_flag_rooms {
                defense_items.push(format!("flag: {}", room));
            }
            let label = if features.military.defense {
                "Defense: ON".to_string()
            } else {
                "Defense: OFF".to_string()
            };
            if defense_items.is_empty() {
                children.push(SummaryContent::Text(label));
            } else {
                children.push(SummaryContent::Lines {
                    header: label,
                    items: defense_items,
                });
            }
        }

        SummaryContent::Tree {
            label: "War".to_string(),
            children,
        }
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let tier = system_data.governor.tier;

        // Defense scan: never-shed — base cadence at every tier.
        if self.should_run_tier(self.last_defense_tick, effective_cadence(DEFENSE_CADENCE, tier, true)) {
            self.last_defense_tick = Some(game::time());
            self.run_defense_scan(system_data, runtime_data);
        }

        // Offense evaluation: sheddable — stretches under pressure.
        if self.should_run_tier(self.last_offense_tick, effective_cadence(OFFENSE_CADENCE, tier, false)) {
            self.last_offense_tick = Some(game::time());
            self.run_offense_evaluation(system_data, runtime_data);
        }

        // Heavy recompute: sheddable — stretches under pressure.
        if self.should_run_tier(self.last_recompute_tick, effective_cadence(RECOMPUTE_CADENCE, tier, false)) {
            self.last_recompute_tick = Some(game::time());
            self.run_heavy_recompute(system_data, runtime_data);
        }

        // WarOperation is a singleton -- it never completes.
        Ok(OperationResult::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1.B6 / IBEX-021: defense never stretches; offense/recompute
    /// stretch ×2/×4 under Conserve/Critical.
    #[test]
    fn war_cadences_stretch_with_the_governor() {
        use crate::cpugovernor::Tier;
        for tier in [Tier::Normal, Tier::Conserve, Tier::Critical] {
            assert_eq!(effective_cadence(DEFENSE_CADENCE, tier, true), DEFENSE_CADENCE, "{tier:?}");
        }
        assert_eq!(effective_cadence(OFFENSE_CADENCE, Tier::Normal, false), 10);
        assert_eq!(effective_cadence(OFFENSE_CADENCE, Tier::Conserve, false), 20);
        assert_eq!(effective_cadence(OFFENSE_CADENCE, Tier::Critical, false), 40);
        assert_eq!(effective_cadence(RECOMPUTE_CADENCE, Tier::Critical, false), 200);
        // The raise itself is load-bearing (IBEX-021: all three were 1).
        assert_eq!(DEFENSE_CADENCE, 2);
        assert_eq!(OFFENSE_CADENCE, 10);
        assert_eq!(RECOMPUTE_CADENCE, 50);
    }

    /// Regression: an enemy CLAIM creep attacking the controller of a
    /// towerless room must trigger a defender. `hostile_creeps()` (the old
    /// gate) only saw Attack/RangedAttack/Work, so this creep declaimed us
    /// unopposed. `hostile_warrants_defender` keys on body parts and catches it.
    #[test]
    fn unarmed_controller_attacker_warrants_a_defender() {
        // The bug: a CLAIM creep (the thing that declaims a room) was ignored.
        assert!(hostile_warrants_defender(&[Part::Claim, Part::Move]));
        // Dismantlers (Work) and lone healers (Heal) are threats too.
        assert!(hostile_warrants_defender(&[Part::Work, Part::Move]));
        assert!(hostile_warrants_defender(&[Part::Heal, Part::Move]));
        // Armed creeps are of course still defended.
        assert!(hostile_warrants_defender(&[Part::Attack, Part::Move]));
        assert!(hostile_warrants_defender(&[Part::RangedAttack, Part::Move]));
    }

    /// Transient unarmed creeps (scouts, haulers) must NOT burn a defender
    /// spawn — they can't hurt an owned room.
    #[test]
    fn unarmed_transient_creeps_do_not_warrant_a_defender() {
        // Pure scout.
        assert!(!hostile_warrants_defender(&[Part::Move, Part::Move]));
        // Hauler.
        assert!(!hostile_warrants_defender(&[Part::Carry, Part::Move]));
        // Tanky-but-toothless (Tough + Move only).
        assert!(!hostile_warrants_defender(&[Part::Tough, Part::Move]));
        // Empty body.
        assert!(!hostile_warrants_defender(&[]));
    }

    // Pin (IBEX-044): cadence checks must not underflow when the persisted
    // tick is ahead of the current time (private-server time reset, restored
    // snapshot). The boundary behavior is "cadence not elapsed yet" -- a
    // benign skip, never a panic.

    #[test]
    fn cadence_elapsed_never_run_returns_true() {
        assert!(cadence_elapsed(100, None, 1));
        assert!(cadence_elapsed(0, None, 50));
    }

    #[test]
    fn cadence_elapsed_normal_progression() {
        assert!(!cadence_elapsed(100, Some(100), 1));
        assert!(cadence_elapsed(101, Some(100), 1));
        assert!(!cadence_elapsed(149, Some(100), 50));
        assert!(cadence_elapsed(150, Some(100), 50));
    }

    #[test]
    fn cadence_elapsed_future_last_tick_is_benign() {
        // Stored tick ahead of "now" must not panic and must report
        // "not elapsed" for any cadence >= 1.
        assert!(!cadence_elapsed(100, Some(10_000), 1));
        assert!(!cadence_elapsed(0, Some(u32::MAX), 50));
    }

    // Pin: a level-0 "reserver" invader core must be a valid attack target
    // in rooms we want to reserve/mine. The old gate (`invader_core_level >
    // 0`, with absence collapsed to 0) made level 0 indistinguishable from
    // "no core", so reserver cores squatted our remote-mining rooms --
    // blocking our reservation AND aborting MiningOutpostMission via its
    // hostile-reservation gate -- until their ~75k-tick collapse timer
    // expired. The eviction also flips `reservation().mine()` to false, so
    // the source-room fallback (not the reservation check) is what makes
    // the blocked room recognizable as ours.

    #[test]
    fn reserver_core_in_blocked_remote_is_targeted() {
        // The incident shape: core stole the reservation (is_our_remote =
        // false), source room adjacent to home, empty war chest -- level 0
        // is always affordable.
        let score = invader_core_attack_score(0, 1, 0, false, true);
        assert_eq!(score, Some(57.0), "{score:?}");

        // Reservation still ours (core seen before the eviction lands):
        // targeted even without the source-room fallback.
        assert!(invader_core_attack_score(0, 1, 0, true, false).is_some());
    }

    #[test]
    fn reserver_core_outside_our_interest_is_left_to_expire() {
        // No sources: nothing to mine, no loot to win.
        assert_eq!(invader_core_attack_score(0, 1, 500_000, false, false), None);
        // Sources, but beyond remote-mining range (MiningOutpostOperation
        // gathers at BFS distance 1).
        assert_eq!(invader_core_attack_score(0, 2, 500_000, false, true), None);
    }

    #[test]
    fn stronghold_affordability_tiers() {
        // Strongholds gate on the war chest: >30k affords L1, >100k L3,
        // >200k L5.
        assert_eq!(invader_core_attack_score(1, 1, 30_000, false, false), None);
        assert!(invader_core_attack_score(1, 1, 30_001, false, false).is_some());
        assert_eq!(invader_core_attack_score(2, 1, 100_000, false, false), None);
        assert!(invader_core_attack_score(3, 1, 100_001, false, false).is_some());
        assert_eq!(invader_core_attack_score(4, 1, 200_000, false, false), None);
        assert!(invader_core_attack_score(5, 1, 200_001, false, false).is_some());
        // Level 0 is affordable even with an empty war chest.
        assert!(invader_core_attack_score(0, 1, 0, true, false).is_some());
    }

    /// Relation: among targetable cores, score strictly decreases with
    /// distance and with level -- closer and easier always sorts first.
    #[test]
    fn core_score_monotonic_in_distance_and_level() {
        for level in 0..=5u8 {
            for dist in 0..6u32 {
                let near = invader_core_attack_score(level, dist, 500_000, true, true);
                let far = invader_core_attack_score(level, dist + 1, 500_000, true, true);
                if let (Some(near), Some(far)) = (near, far) {
                    assert!(near > far, "level {level} dist {dist}: {near} <= {far}");
                }
            }
        }
        for level in 0..5u8 {
            let easy = invader_core_attack_score(level, 1, 500_000, true, true);
            let hard = invader_core_attack_score(level + 1, 1, 500_000, true, true);
            if let (Some(easy), Some(hard)) = (easy, hard) {
                assert!(easy > hard, "level {level}: {easy} <= {hard}");
            }
        }
    }
}
