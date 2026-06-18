use super::data::*;
use super::operationsystem::*;
use crate::military::composition::SquadComposition;
use crate::military::objective_queue::{ForceRequirement, ObjectiveKind, ObjectiveOwner, ObjectiveRequest, OBJECTIVE_PRIORITY_HIGH};
use crate::military::threatmap::*;
use crate::missions::data::*;
use crate::missions::nuke_defense::*;
use crate::missions::safe_mode::*;
use crate::missions::squad_defense::*;
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

impl From<TargetSource> for super::attack::AttackReason {
    fn from(source: TargetSource) -> Self {
        match source {
            TargetSource::AttackFlag => Self::Flag,
            TargetSource::DefendFlag => Self::Flag,
            TargetSource::ThreatResponse => Self::ThreatResponse,
            TargetSource::Expansion => Self::Expansion,
            TargetSource::ResourceDenial => Self::ResourceDenial,
            TargetSource::InvaderCore { level } => Self::InvaderCore { level },
            TargetSource::InvaderCreeps => Self::InvaderCreeps,
            TargetSource::PowerBank { power, .. } => Self::PowerBank { power },
            TargetSource::ProactiveDefense => Self::ProactiveDefense,
        }
    }
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

/// War's own bookkeeping for one active child AttackOperation: the entity,
/// its target room, and the reason it was launched.
///
/// The reason is recorded HERE, at launch time, because war is the module
/// that decided it (AGENTS.md §8: keep the information with its owner) --
/// concurrency gates like the power-bank cap filter this list instead of
/// asking the generic operation surface what kind of attack an entity is.
#[derive(Clone, ConvertSaveload)]
pub struct ActiveAttack {
    entity: Entity,
    room: RoomName,
    reason: super::attack::AttackReason,
}

/// Unified military coordinator singleton. Manages both offense
/// (AttackOperations) and defense (SquadDefenseMission, NukeDefenseMission,
/// SafeModeMission, WallRepairMission).
///
/// Uses tiered cadences:
/// - Defense scan: every 1-2 ticks (cheap, checks owned rooms for threats)
/// - Offense evaluation: every 10-20 ticks (scores candidates, launches attacks)
/// - Heavy recompute: every 50+ ticks (full retarget, rebalance room assignments)
#[derive(Clone, ConvertSaveload)]
pub struct WarOperation {
    owner: EntityOption<Entity>,

    /// Tiered cadence tracking (tick numbers).
    last_defense_tick: Option<u32>,
    last_offense_tick: Option<u32>,
    last_recompute_tick: Option<u32>,

    /// Child AttackOperations (offense): entity + target room + launch reason.
    active_attacks: EntityVec<ActiveAttack>,

    /// Rooms with manually placed 'defend' flags (persisted so we don't
    /// re-scan flags every tick -- just refresh periodically).
    defend_flag_rooms: Vec<RoomName>,

    /// Maximum concurrent attack operations (scales with economy).
    max_concurrent_attacks: u32,

    /// Separate cap for power bank operations.
    max_concurrent_power_banks: u32,
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
            active_attacks: EntityVec::new(),
            defend_flag_rooms: Vec::new(),
            max_concurrent_attacks: 1,
            max_concurrent_power_banks: 1,
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

                let has_squad_defense = room_data.get_missions().iter().any(|mission_entity| {
                    system_data
                        .mission_data
                        .get(*mission_entity)
                        .as_mission_type::<SquadDefenseMission>()
                        .is_some()
                });

                if has_squad_defense {
                    return None;
                }

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

        for need in rooms_needing_defense {
            let escalation = DefenseEscalation::from_threat(need.estimated_dps, need.estimated_heal, need.hostile_count, need.any_boosted);

            // ── Migrated path (ADR 0008 §W1): route defense through the
            // SquadManager via a `Defend` objective instead of the legacy
            // squad-less SquadDefenseMission. Feature-flagged, default OFF — the
            // legacy `match escalation { … }` below is untouched when off. The
            // producer re-asserts every defense scan while the room warrants a
            // defender; when it stops (room safe / lost) the short TTL lapses and
            // the manager retires the squad.
            if system_data.features.military.manager_defense {
                let room_name = match system_data.room_data.get(need.room_entity) {
                    Some(rd) => rd.name,
                    None => continue,
                };
                let composition = match escalation {
                    DefenseEscalation::Quad => SquadComposition::quad_ranged(),
                    DefenseEscalation::Duo => SquadComposition::duo_attack_heal(),
                    DefenseEscalation::Solo => SquadComposition::solo_ranged(),
                };
                system_data.combat_objective_queue.request(
                    ObjectiveRequest::new(
                        ObjectiveKind::Defend { room: room_name },
                        OBJECTIVE_PRIORITY_HIGH,
                        ForceRequirement::single(composition),
                    )
                    .owner(ObjectiveOwner::Defense)
                    .ttl(DEFEND_OBJECTIVE_TTL),
                    game::time(),
                );
                continue;
            }

            let room_data = match system_data.room_data.get_mut(need.room_entity) {
                Some(rd) => rd,
                None => continue,
            };

            info!(
                "[War] Starting {:?} squad defense for room: {} (dps={:.0}, heal={:.0}, count={})",
                escalation, room_data.name, need.estimated_dps, need.estimated_heal, need.hostile_count
            );

            let mission_entity = match escalation {
                DefenseEscalation::Quad => SquadDefenseMission::build_quad(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    need.room_entity,
                    &home_rooms,
                )
                .build(),
                DefenseEscalation::Duo => SquadDefenseMission::build_duo(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    need.room_entity,
                    &home_rooms,
                )
                .build(),
                DefenseEscalation::Solo => SquadDefenseMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    need.room_entity,
                    &home_rooms,
                )
                .build(),
            };

            room_data.add_mission(mission_entity);
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

        for &defend_room in &self.defend_flag_rooms {
            let room_entity = (system_data.entities, &*system_data.room_data)
                .join()
                .find(|(_, rd)| rd.name == defend_room)
                .map(|(e, _)| e);

            if let Some(room_entity) = room_entity {
                let room_data = match system_data.room_data.get(room_entity) {
                    Some(rd) => rd,
                    None => continue,
                };

                let has_squad_defense = room_data.get_missions().iter().any(|mission_entity| {
                    system_data
                        .mission_data
                        .get(*mission_entity)
                        .as_mission_type::<SquadDefenseMission>()
                        .is_some()
                });

                if !has_squad_defense {
                    info!("[War] Creating defend-flag defense mission for room: {}", defend_room);
                    let room_data = match system_data.room_data.get_mut(room_entity) {
                        Some(rd) => rd,
                        None => continue,
                    };
                    let mission_entity = SquadDefenseMission::build_duo(
                        system_data.updater.create_entity(system_data.entities),
                        Some(runtime_data.entity),
                        room_entity,
                        &home_rooms,
                    )
                    .build();
                    room_data.add_mission(mission_entity);
                }
            }
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

                // Already has defense?
                let has_defense = room_data
                    .get_missions()
                    .iter()
                    .any(|me| system_data.mission_data.get(*me).as_mission_type::<SquadDefenseMission>().is_some());
                if has_defense {
                    return None;
                }

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

        for (room_entity, dps, heal, count) in remote_rooms_with_invaders {
            let room_data = match system_data.room_data.get_mut(room_entity) {
                Some(rd) => rd,
                None => continue,
            };

            // Escalate based on invader strength.
            let escalation = if dps > 100.0 || heal > 30.0 || count >= 3 {
                DefenseEscalation::Duo
            } else {
                DefenseEscalation::Solo
            };

            info!(
                "[War] Invaders in remote room {}: {:?} defense (dps={:.0}, heal={:.0}, count={})",
                room_data.name, escalation, dps, heal, count
            );

            let mission_entity = match escalation {
                DefenseEscalation::Duo => SquadDefenseMission::build_duo(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    room_entity,
                    &home_rooms,
                )
                .build(),
                _ => SquadDefenseMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(runtime_data.entity),
                    room_entity,
                    &home_rooms,
                )
                .build(),
            };

            room_data.add_mission(mission_entity);
        }
    }

    // ── Offense evaluation (every 10-20 ticks) ────────────────────────────

    fn run_offense_evaluation(&mut self, system_data: &mut OperationExecutionSystemData, runtime_data: &mut OperationExecutionRuntimeData) {
        let features = system_data.features;

        if !features.military.offense {
            if features.military.debug_log {
                info!("[War] Offense disabled by feature flag");
            }
            return;
        }

        // Clean up completed/dead attack operations and their room tracking.
        self.cleanup_dead_attacks(system_data);

        // Don't launch new attacks if at capacity.
        if self.active_attacks.len() as u32 >= self.max_concurrent_attacks {
            if features.military.debug_log {
                info!(
                    "[War] Offense at capacity ({}/{})",
                    self.active_attacks.len(),
                    self.max_concurrent_attacks
                );
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
                if self.is_attacking_room(room) {
                    continue;
                }
                candidates.push(AttackCandidate {
                    room,
                    source: TargetSource::AttackFlag,
                    score: 100.0,
                    tower_count: 0,
                    estimated_enemy_dps: 0.0,
                    estimated_enemy_heal: 0.0,
                    has_safe_mode: false,
                    estimated_roi: None,
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
                "[War] Offense scan: {} threat rooms, {} active attacks (cap {}), economy={}",
                threat_rooms.len(),
                self.active_attacks.len(),
                self.max_concurrent_attacks,
                system_data.economy.total_stored_energy
            );
        }

        for (room_entity, room_name, threat_data) in &threat_rooms {
            let room_name = *room_name;
            let room_entity = Some(*room_entity);

            if self.is_attacking_room(room_name) {
                continue;
            }

            // Skip rooms we own (defense handles those).
            let is_owned = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|d| d.owner().mine())
                .unwrap_or(false);
            if is_owned {
                continue;
            }

            // Skip stale data (older than 200 ticks).
            if current_tick.saturating_sub(threat_data.last_seen) > 200 {
                if war_debug {
                    info!(
                        "[War]   Skip {} -- stale data (age={})",
                        room_name,
                        current_tick.saturating_sub(threat_data.last_seen)
                    );
                }
                continue;
            }

            // Compute minimum distance from any home room.
            let min_distance = self.min_distance_to_homes(room_name, &home_rooms, system_data.pathfinder, current_tick);

            // Skip rooms that are too far away (> 10 hops).
            if min_distance > 10 {
                if war_debug {
                    info!("[War]   Skip {} -- too far (distance={})", room_name, min_distance);
                }
                continue;
            }

            // Check for invader cores in the room. `None` = no core present;
            // `Some(0)` is a level-0 "reserver" core NPC-reserving the room.
            // Presence must stay distinguishable from absence: collapsing
            // both to 0 made reserver cores untargetable, so they squatted
            // our remote-mining rooms until their collapse timer expired.
            let invader_core_level = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_structures())
                .and_then(|structures| structures.invader_cores().iter().map(|core| core.level()).max());

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
            if let Some(core_level) = invader_core_level {
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
                        candidates.push(AttackCandidate {
                            room: room_name,
                            source: TargetSource::InvaderCore { level: core_level },
                            score,
                            tower_count,
                            estimated_enemy_dps: threat_data.estimated_dps,
                            estimated_enemy_heal: threat_data.estimated_heal,
                            has_safe_mode: false,
                            estimated_roi: None,
                        });
                    }
                }
            }

            // ── Power bank targeting ─────────────────────────────────────
            if let Some((power, ticks_to_decay)) = power_bank_info {
                // Only farm power banks if we have significant economy and
                // enough time remaining to actually extract the power.
                let min_ticks_needed = power_bank_min_ticks_needed(min_distance);
                // Count only attacks launched as power-bank farms
                // (AttackReason::PowerBank) against the power-bank cap;
                // unrelated attacks must not consume power-bank slots
                // (IBEX-043). War recorded the reason at launch, so this is
                // a filter over its own bookkeeping.
                let power_bank_count = count_power_bank_attacks(&self.active_attacks);

                if power >= 1000
                    && ticks_to_decay > min_ticks_needed
                    && system_data.economy.total_stored_energy > 100_000
                    && power_bank_count < self.max_concurrent_power_banks
                {
                    let roi = power as f32 / 8_000.0; // Rough energy cost estimate.
                    let distance_penalty = min_distance as f32 * 2.0;
                    let decay_bonus = if ticks_to_decay > 3000 { 10.0 } else { 0.0 };
                    let score = 20.0 + (roi * 5.0).min(30.0) - distance_penalty + decay_bonus;

                    if score > 0.0 {
                        candidates.push(AttackCandidate {
                            room: room_name,
                            source: TargetSource::PowerBank { power, ticks_to_decay },
                            score,
                            tower_count: 0,
                            estimated_enemy_dps: 0.0,
                            estimated_enemy_heal: 0.0,
                            has_safe_mode: false,
                            estimated_roi: Some(roi),
                        });
                    }
                }
            }

            // ── Invader creeps in remote rooms ───────────────────────────
            // Invader creeps disrupting our reserved rooms should be cleared.
            // Source Keepers are permanent residents of SK rooms and should NOT
            // trigger an InvaderCreeps attack. Only actual Invader NPCs count.
            let has_invader_creeps = threat_data
                .hostile_creeps
                .iter()
                .any(|c| crate::military::is_invader_owner(&c.owner));
            let all_npc = threat_data.hostile_creeps.iter().all(|c| crate::military::is_npc_owner(&c.owner));
            let is_our_remote = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|d| d.reservation().mine())
                .unwrap_or(false);

            // A present core (any level, including a level-0 reserver) routes
            // through the InvaderCore candidate above instead -- killing the
            // core is what actually clears the room.
            if has_invader_creeps
                && all_npc
                && features.military.attack_invaders
                && is_our_remote
                && invader_core_level.is_none()
                && power_bank_info.is_none()
            {
                let distance_penalty = min_distance as f32 * 2.0;
                let score = 50.0 - distance_penalty;

                if score > 0.0 {
                    candidates.push(AttackCandidate {
                        room: room_name,
                        source: TargetSource::InvaderCreeps,
                        score,
                        tower_count: 0,
                        estimated_enemy_dps: threat_data.estimated_dps,
                        estimated_enemy_heal: threat_data.estimated_heal,
                        has_safe_mode: false,
                        estimated_roi: None,
                    });
                }
            }

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

        // ── 4. Launch attacks for top candidates ─────────────────────────

        for candidate in candidates {
            if self.active_attacks.len() as u32 >= self.max_concurrent_attacks {
                break;
            }

            info!(
                "[War] Launching AttackOperation for {} (source={:?}, score={:.1}, towers={}, dps={:.0}, heal={:.0})",
                candidate.room,
                candidate.source,
                candidate.score,
                candidate.tower_count,
                candidate.estimated_enemy_dps,
                candidate.estimated_enemy_heal
            );

            let reason: super::attack::AttackReason = candidate.source.into();
            let attack_entity = super::attack::AttackOperation::build_with_context(
                system_data.updater.create_entity(system_data.entities),
                Some(runtime_data.entity),
                candidate.room,
                reason.clone(),
            )
            .build();

            self.add_active_attack(attack_entity, candidate.room, reason);

            // Force a home room rebalance on the next tick so the new attack
            // gets home rooms assigned promptly rather than waiting up to
            // RECOMPUTE_CADENCE ticks.
            self.last_recompute_tick = None;
        }
    }

    // ── Heavy recompute (every 50+ ticks) ─────────────────────────────────

    fn run_heavy_recompute(&mut self, system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {
        let current_tick = game::time();

        // ── 1. Update concurrent attack limits based on economy ──────────

        // Scale attack capacity with room count and economy health.
        let base_attacks = system_data.economy.room_count.max(1);
        let economy_multiplier = if system_data.economy.total_stored_energy > 300_000 {
            2
        } else if system_data.economy.total_stored_energy > 100_000 {
            1
        } else {
            0
        };
        self.max_concurrent_attacks = base_attacks.saturating_sub(1).max(1) + economy_multiplier;
        self.max_concurrent_power_banks = std::cmp::min(2, system_data.economy.room_count);

        // ── 2. Propagate threat intel to active AttackOperations ─────────

        let war_debug = system_data.features.military.debug_log;

        for attack in self.active_attacks.iter() {
            let entity = attack.entity;
            if !system_data.entities.is_alive(entity) {
                continue;
            }
            let room_name = attack.room;

            // Read threat data from the room entity's component.
            let room_entity = system_data.mapping.get_room(&room_name);
            let threat_data = room_entity.and_then(|e| system_data.threat_data.get(e));

            if let Some(threat_data) = threat_data {
                let tower_count = threat_data.hostile_tower_positions.len() as u32;
                let player_hostiles: Vec<_> = threat_data
                    .hostile_creeps
                    .iter()
                    .filter(|c| !crate::military::is_npc_owner(&c.owner))
                    .collect();
                let hostile_count = player_hostiles.len() as u32;
                let enemy_dps: f32 = player_hostiles.iter().map(|c| c.melee_dps + c.ranged_dps).sum::<f32>() + tower_count as f32 * 600.0;
                let enemy_heal: f32 = player_hostiles.iter().map(|c| c.heal_per_tick).sum();
                let any_boosted = player_hostiles.iter().any(|c| c.boosted);

                if war_debug && (hostile_count > 0 || tower_count > 0) {
                    info!(
                        "[War] Threat update for {}: towers={}, dps={:.0}, heal={:.0}, hostiles={}, boosted={}, safe_mode={}/{}",
                        room_name,
                        tower_count,
                        enemy_dps,
                        enemy_heal,
                        hostile_count,
                        any_boosted,
                        threat_data.safe_mode_active,
                        threat_data.safe_mode_available
                    );
                }

                // Propagate updated intel to the AttackOperation via LazyUpdate
                // (we can't access the operations storage directly here).
                let safe_active = threat_data.safe_mode_active;
                let safe_available = threat_data.safe_mode_available;
                system_data.updater.exec_mut(move |world| {
                    if let Some(OperationData::Attack(ref mut attack_op)) = world.write_storage::<OperationData>().get_mut(entity) {
                        attack_op.update_threat_intel(tower_count, enemy_dps, enemy_heal, hostile_count, safe_active, safe_available);
                    }
                });
            }
        }

        // ── 3. Request visibility for rooms adjacent to our territory ────
        // This ensures threat data is fresh for rooms near our
        // borders, enabling proactive threat detection and target selection.

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

        // Also request visibility for rooms we're actively attacking but
        // don't currently have fresh data for.
        for attack in self.active_attacks.iter() {
            let room_entity = system_data.mapping.get_room(&attack.room);
            let has_fresh_data = room_entity
                .and_then(|e| system_data.threat_data.get(e))
                .map(|d| current_tick.saturating_sub(d.last_seen) < 50)
                .unwrap_or(false);

            if !has_fresh_data {
                system_data
                    .visibility
                    .request(VisibilityRequest::new(attack.room, 1.0, VisibilityRequestFlags::ALL));
            }
        }

        // ── 4. Detect threats near territory not yet being attacked ──────
        // Proactive defense: if a room adjacent to our territory has player
        // hostiles and we're not already responding, log it for the next
        // offense evaluation cycle to pick up.

        for (_, rd, threat_data) in (system_data.entities, &*system_data.room_data, system_data.threat_data).join() {
            let room_name = rd.name;
            if threat_data.threat_level < ThreatLevel::PlayerRaid {
                continue;
            }
            if self.is_attacking_room(room_name) {
                continue;
            }

            // Check if this room is adjacent to one of our rooms.
            let near_home = home_rooms.iter().any(|&home| {
                let route = system_data.pathfinder.route_distance(home, room_name, current_tick);
                route.reachable && route.hops <= 2
            });

            if near_home {
                debug!(
                    "[War] Proactive: player threat ({:?}) detected near territory in {} (dps={:.0}, heal={:.0})",
                    threat_data.threat_level, room_name, threat_data.estimated_dps, threat_data.estimated_heal
                );
            }
        }

        // ── 5. Rebalance home room assignments across active attacks ────
        self.reassign_home_rooms(system_data);
    }

    // ── Home room rebalancing ────────────────────────────────────────────────

    fn reassign_home_rooms(&self, system_data: &mut OperationExecutionSystemData) {
        let current_tick = game::time();
        let war_debug = system_data.features.military.debug_log;

        // Collect home rooms with spawns: (entity, room_name).
        let home_rooms: Vec<(Entity, RoomName)> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| {
                rd.get_dynamic_visibility_data().map(|d| d.owner().mine()).unwrap_or(false)
                    && rd.get_structures().map(|s| !s.spawns().is_empty()).unwrap_or(false)
            })
            .map(|(e, rd)| (e, rd.name))
            .collect();

        if home_rooms.is_empty() {
            return;
        }

        // Collect active attacks: (entity, target_room_name).
        let attacks: Vec<(Entity, RoomName)> = self
            .active_attacks
            .iter()
            .filter(|a| system_data.entities.is_alive(a.entity))
            .map(|a| (a.entity, a.room))
            .collect();

        if attacks.is_empty() {
            return;
        }

        // Build distance matrix: for each (attack_idx, home_idx) -> hops.
        let distances: Vec<Vec<u32>> = attacks
            .iter()
            .map(|(_, target)| {
                home_rooms
                    .iter()
                    .map(|(_, home_name)| {
                        let route = system_data.pathfinder.route_distance(*home_name, *target, current_tick);
                        if route.reachable {
                            route.hops
                        } else {
                            u32::MAX
                        }
                    })
                    .collect()
            })
            .collect();

        // Greedy assignment: each home room assigned to at most one attack.
        // First pass: assign the closest available home room to each attack.
        let mut assigned: Vec<Vec<usize>> = vec![Vec::new(); attacks.len()];
        let mut home_taken = vec![false; home_rooms.len()];

        // Sort attacks by fewest reachable home rooms (most constrained first).
        let mut attack_order: Vec<usize> = (0..attacks.len()).collect();
        attack_order.sort_by_key(|&ai| distances[ai].iter().filter(|&&d| d < u32::MAX).count());

        for &ai in &attack_order {
            // Find closest untaken home room.
            let best = distances[ai]
                .iter()
                .enumerate()
                .filter(|(hi, _)| !home_taken[*hi])
                .filter(|(_, &d)| d < u32::MAX)
                .min_by_key(|(_, &d)| d);

            if let Some((hi, _)) = best {
                assigned[ai].push(hi);
                home_taken[hi] = true;
            }
        }

        // Second pass: distribute remaining unassigned home rooms to the
        // closest attack that could use them (round-robin by distance).
        for hi in 0..home_rooms.len() {
            if home_taken[hi] {
                continue;
            }
            // Find the attack closest to this home room that has the fewest
            // assignments (prefer under-served attacks).
            let best_attack = attacks
                .iter()
                .enumerate()
                .filter(|(ai, _)| distances[*ai][hi] < u32::MAX)
                .min_by_key(|(ai, _)| (assigned[*ai].len(), distances[*ai][hi]));

            if let Some((ai, _)) = best_attack {
                assigned[ai].push(hi);
                home_taken[hi] = true;
            }
        }

        // Build EntityVec assignments and push via LazyUpdate.
        for (ai, home_indices) in assigned.iter().enumerate() {
            let attack_entity = attacks[ai].0;
            let mut rooms = EntityVec::new();
            for &hi in home_indices {
                rooms.push(home_rooms[hi].0);
            }

            if war_debug {
                let room_names: Vec<String> = home_indices.iter().map(|&hi| home_rooms[hi].1.to_string()).collect();
                info!("[War] Assign {} -> spawn: [{}]", attacks[ai].1, room_names.join(", "));
            }

            // Don't overwrite existing home rooms with an empty assignment.
            // This can happen when route distances are unreachable or all
            // home rooms were consumed by higher-priority attacks.
            if rooms.is_empty() {
                if war_debug {
                    info!("[War] Skipping empty home room assignment for {} (keeping existing)", attacks[ai].1);
                }
                continue;
            }

            system_data.updater.exec_mut(move |world| {
                // Collect mission entities before mutating operation storage.
                let mission_entities: Vec<Entity> = {
                    let ops = world.read_storage::<OperationData>();
                    if let Some(OperationData::Attack(ref attack_op)) = ops.get(attack_entity) {
                        attack_op.mission_entities().to_vec()
                    } else {
                        Vec::new()
                    }
                };

                // Update the operation's home rooms.
                if let Some(OperationData::Attack(ref mut attack_op)) = world.write_storage::<OperationData>().get_mut(attack_entity) {
                    attack_op.set_home_rooms(rooms.clone());
                }

                // Propagate to child AttackMissions.
                let missions = world.read_storage::<MissionData>();
                for mission_entity in mission_entities {
                    if let Some(MissionData::AttackMission(ref cell)) = missions.get(mission_entity) {
                        cell.borrow_mut().set_home_rooms(rooms.clone());
                    }
                }
            });
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────────

    /// Check if we already have an active attack targeting this room.
    fn is_attacking_room(&self, room: RoomName) -> bool {
        self.active_attacks.iter().any(|a| a.room == room)
    }

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

    /// Get the attack operation entity targeting a given room, if any.
    pub fn get_attack_for_room(&self, room: RoomName) -> Option<Entity> {
        self.active_attacks.iter().find(|a| a.room == room).map(|a| a.entity)
    }

    /// Record a newly launched attack with the reason war launched it for.
    fn add_active_attack(&mut self, entity: Entity, room: RoomName, reason: super::attack::AttackReason) {
        self.active_attacks.push(ActiveAttack { entity, room, reason });
    }

    /// Remove an active attack by entity.
    fn remove_active_attack(&mut self, entity: Entity) {
        self.active_attacks.retain(|a| a.entity != entity);
    }

    /// Clean up dead/completed attack operations from tracking.
    fn cleanup_dead_attacks(&mut self, system_data: &OperationExecutionSystemData) {
        self.active_attacks.retain(|a| system_data.entities.is_alive(a.entity));
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

/// Power-bank launch feasibility window (P1.D5 / ADR 0013 D1.2): the
/// farm only fits the bank's decay window when kill time AT THE DUO'S
/// CAPPED DPS + travel + serial duo spawn + a margin all fit. The
/// pre-D5 window (`dist·50 + 500`) ignored kill time entirely (~3.3k
/// ticks), green-lighting banks that could never be finished.
fn power_bank_min_ticks_needed(min_distance: u32) -> u32 {
    /// Engine: POWER_BANK_HITS.
    const BANK_HITS: u32 = 2_000_000;
    /// bodies.rs `power_bank_attacker_body` cap (healer-matched).
    const DUO_ATTACK_PARTS: u32 = 20;
    /// ATTACK_POWER = 30 hits/part/tick.
    const DUO_DPS: u32 = DUO_ATTACK_PARTS * 30;
    /// Serial spawn of the duo: (20 ATTACK + 20 MOVE) + (25 HEAL +
    /// 25 MOVE) parts × 3 ticks/part.
    const DUO_SPAWN_TICKS: u32 = (40 + 50) * 3;
    /// Slack for rally, lair dodges, and the loot haul-out.
    const MARGIN_TICKS: u32 = 200;

    let kill_ticks = BANK_HITS / DUO_DPS;
    kill_ticks + (min_distance * 50) + DUO_SPAWN_TICKS + MARGIN_TICKS
}

/// Count active attacks that war launched as power-bank farms
/// (`AttackReason::PowerBank`). Feeds the `max_concurrent_power_banks` gate
/// (IBEX-043): the reason is war's own bookkeeping, recorded at launch.
fn count_power_bank_attacks(attacks: &[ActiveAttack]) -> u32 {
    attacks
        .iter()
        .filter(|a| matches!(a.reason, super::attack::AttackReason::PowerBank { .. }))
        .count() as u32
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

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for WarOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn child_complete(&mut self, child: Entity) {
        self.remove_active_attack(child);
    }

    fn repair_entity_refs(&mut self, is_valid: &dyn Fn(Entity) -> bool) {
        let before = self.active_attacks.len();
        self.active_attacks.retain(|a| {
            let valid = is_valid(a.entity);
            if !valid {
                error!("INTEGRITY: dead attack entity {:?} removed from WarOperation", a.entity);
            }
            valid
        });
        if self.active_attacks.len() < before {
            warn!(
                "INTEGRITY: removed {} dead attack ref(s) from WarOperation",
                before - self.active_attacks.len()
            );
        }
    }

    fn describe_operation(&self, ctx: &OperationDescribeContext) -> SummaryContent {
        let features = ctx.features;
        let mut children = Vec::new();

        // Offense section.
        {
            let mut offense_items = Vec::new();
            for attack in self.active_attacks.iter() {
                offense_items.push(format!("-> {}", attack.room));
            }
            let label = if features.military.offense {
                if offense_items.is_empty() {
                    format!("Offense: ON (cap {})", self.max_concurrent_attacks)
                } else {
                    format!("Offense: {}/{} active", offense_items.len(), self.max_concurrent_attacks)
                }
            } else {
                "Offense: OFF".to_string()
            };
            if offense_items.is_empty() {
                children.push(SummaryContent::Text(label));
            } else {
                children.push(SummaryContent::Lines {
                    header: label,
                    items: offense_items,
                });
            }
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

    /// P1.D5 / ADR 0013 D1.2: the feasibility window must include kill
    /// time — at 20×ATTACK (600 dps) a 2M bank takes 3333 ticks, so
    /// even a zero-distance bank needs >3.5k ticks of decay left; the
    /// pre-D5 window claimed 500.
    #[test]
    fn power_bank_window_includes_kill_time() {
        let zero_dist = power_bank_min_ticks_needed(0);
        assert_eq!(zero_dist, 3333 + 270 + 200, "{zero_dist}");
        // Distance adds 50 ticks/room.
        assert_eq!(power_bank_min_ticks_needed(4) - zero_dist, 200);
        // A freshly-spawned bank (5000 decay) IS farmable nearby…
        assert!(power_bank_min_ticks_needed(5) < 5000);
        // …but never at 25+ rooms out.
        assert!(power_bank_min_ticks_needed(25) > 5000);
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

    // Pin (IBEX-043): the power-bank concurrency gate counts only attacks
    // war launched as power-bank farms (AttackReason::PowerBank), using the
    // reason recorded in war's own ActiveAttack bookkeeping -- never "all
    // active attacks", and never a predicate on the generic operation
    // surface (AGENTS.md §8).

    use super::super::attack::AttackReason;

    fn active_attack(world: &mut specs::World, reason: AttackReason) -> ActiveAttack {
        ActiveAttack {
            entity: world.create_entity().build(),
            room: "E1N1".parse().expect("valid room name"),
            reason,
        }
    }

    #[test]
    fn power_bank_counter_counts_only_power_bank_attacks() {
        let mut world = specs::World::new();

        // Three non-power-bank attacks plus one power-bank farm: the counter
        // feeding `max_concurrent_power_banks` must see exactly 1, so
        // unrelated attacks cannot exhaust the power-bank slots.
        let attacks = vec![
            active_attack(&mut world, AttackReason::Flag),
            active_attack(&mut world, AttackReason::InvaderCreeps),
            active_attack(&mut world, AttackReason::ThreatResponse),
            active_attack(&mut world, AttackReason::PowerBank { power: 4000 }),
        ];

        assert_eq!(count_power_bank_attacks(&attacks), 1);
        assert_eq!(count_power_bank_attacks(&[]), 0);
    }

    #[test]
    fn power_bank_counter_reason_matrix() {
        let mut world = specs::World::new();

        let reasons = [
            (AttackReason::Flag, 0),
            (AttackReason::ThreatResponse, 0),
            (AttackReason::Expansion, 0),
            (AttackReason::ResourceDenial, 0),
            (AttackReason::InvaderCore { level: 2 }, 0),
            (AttackReason::InvaderCreeps, 0),
            (AttackReason::SourceKeeper, 0),
            (AttackReason::PowerBank { power: 3000 }, 1),
            (AttackReason::ProactiveDefense, 0),
        ];

        for (reason, expected) in reasons {
            let attacks = [active_attack(&mut world, reason.clone())];
            assert_eq!(
                count_power_bank_attacks(&attacks),
                expected,
                "power-bank count mismatch for reason {:?}",
                reason
            );
        }
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
