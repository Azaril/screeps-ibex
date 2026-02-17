use super::data::*;
use super::operationsystem::*;
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
    /// Determine escalation level from threat analysis.
    fn from_threat(estimated_dps: f32, estimated_heal: f32, hostile_count: usize, any_boosted: bool) -> Self {
        if (any_boosted && estimated_dps > 200.0)
            || (estimated_heal > 100.0 && estimated_dps > 150.0)
            || hostile_count >= 4
        {
            DefenseEscalation::Quad
        } else if estimated_dps > 60.0
            || estimated_heal > 20.0
            || hostile_count >= 2
            || any_boosted
        {
            DefenseEscalation::Duo
        } else {
            DefenseEscalation::Solo
        }
    }
}

// ---------------------------------------------------------------------------
// WarOperation
// ---------------------------------------------------------------------------

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

    /// Child AttackOperation entities (offense), paired with their target room.
    /// Stored as parallel vecs for ConvertSaveload compatibility.
    active_attack_entities: EntityVec<Entity>,
    active_attack_rooms: Vec<RoomName>,

    /// Rooms with manually placed 'defend' flags (persisted so we don't
    /// re-scan flags every tick -- just refresh periodically).
    defend_flag_rooms: Vec<RoomName>,

    /// Maximum concurrent attack operations (scales with economy).
    max_concurrent_attacks: u32,

    /// Separate cap for power bank operations.
    max_concurrent_power_banks: u32,
}

// Cadence constants (ticks).
const DEFENSE_CADENCE: u32 = 2;
const OFFENSE_CADENCE: u32 = 15;
const RECOMPUTE_CADENCE: u32 = 50;

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
            active_attack_entities: EntityVec::new(),
            active_attack_rooms: Vec::new(),
            defend_flag_rooms: Vec::new(),
            max_concurrent_attacks: 1,
            max_concurrent_power_banks: 1,
        }
    }

    // ── Defense scan (every 1-2 ticks) ─────────────────────────────────────

    fn run_defense_scan(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) {
        let features = crate::features::features();

        if !features.military.defense {
            return;
        }

        // Collect home rooms (rooms with spawns).
        let home_rooms: Vec<Entity> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| {
                rd.get_dynamic_visibility_data()
                    .map(|d| d.owner().mine())
                    .unwrap_or(false)
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

                let has_nukes = room_data
                    .get_nukes()
                    .map(|n| n.has_incoming())
                    .unwrap_or(false);

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

                if !has_hostiles {
                    return None;
                }

                let creeps = room_data.get_creeps()?;
                let hostiles: Vec<_> = creeps
                    .hostile()
                    .iter()
                    .filter(|c| {
                        !crate::military::is_npc_owner(&c.owner().username())
                    })
                    .collect();

                if hostiles.is_empty() {
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
            let room_data = match system_data.room_data.get_mut(need.room_entity) {
                Some(rd) => rd,
                None => continue,
            };

            let escalation = DefenseEscalation::from_threat(
                need.estimated_dps,
                need.estimated_heal,
                need.hostile_count,
                need.any_boosted,
            );

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

        // ── Nuke defense, safe mode, wall repair ───────────────────────────

        for state in room_states {
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

        let remote_rooms_with_invaders: Vec<(Entity, f32, f32, usize)> =
            (system_data.entities, &*system_data.room_data)
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
                    let has_defense = room_data.get_missions().iter().any(|me| {
                        system_data
                            .mission_data
                            .get(*me)
                            .as_mission_type::<SquadDefenseMission>()
                            .is_some()
                    });
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

    fn run_offense_evaluation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) {
        let features = crate::features::features();

        if !features.military.offense {
            if features.military.debug_log {
                info!("[War] Offense disabled by feature flag");
            }
            return;
        }

        // Clean up completed/dead attack operations and their room tracking.
        self.cleanup_dead_attacks(system_data);

        // Don't launch new attacks if at capacity.
        if self.active_attack_entities.len() as u32 >= self.max_concurrent_attacks {
            if features.military.debug_log {
                info!(
                    "[War] Offense at capacity ({}/{})",
                    self.active_attack_entities.len(), self.max_concurrent_attacks
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
                rd.get_dynamic_visibility_data()
                    .map(|d| d.owner().mine())
                    .unwrap_or(false)
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
        let threat_rooms: Vec<(Entity, RoomName, RoomThreatData)> = (system_data.entities, &*system_data.room_data, system_data.threat_data)
            .join()
            .map(|(e, rd, td)| (e, rd.name, td.clone()))
            .collect();

        if war_debug {
            info!(
                "[War] Offense scan: {} threat rooms, {} active attacks (cap {}), economy={}",
                threat_rooms.len(),
                self.active_attack_entities.len(),
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
            let min_distance = self.min_distance_to_homes(
                room_name,
                &home_rooms,
                system_data.route_cache,
                current_tick,
            );

            // Skip rooms that are too far away (> 10 hops).
            if min_distance > 10 {
                if war_debug {
                    info!("[War]   Skip {} -- too far (distance={})", room_name, min_distance);
                }
                continue;
            }

            // Check for invader cores in the room.
            let invader_core_level = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_structures())
                .map(|structures| {
                    structures
                        .invader_cores()
                        .iter()
                        .map(|core| core.level())
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);

            // Check for power banks.
            let power_bank_info = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_structures())
                .and_then(|structures| {
                    structures.power_banks().first().map(|pb| {
                        (pb.power(), pb.ticks_to_decay())
                    })
                });

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
                    tower_count, invader_core_level,
                    power_bank_info.map(|(p, d)| format!("{}pw/{}t", p, d)).unwrap_or_else(|| "none".to_string()),
                    room_owner_hostile, has_safe_mode
                );
            }

            // ── Invader core targeting ────────────────────────────────────
            if invader_core_level > 0 && features.military.attack_invaders {
                // Only attack cores we can handle. Level 0 = reserver (easy),
                // levels 1-5 = stronghold (increasingly hard).
                let max_affordable_level = if system_data.economy.total_stored_energy > 200_000 {
                    5
                } else if system_data.economy.total_stored_energy > 100_000 {
                    3
                } else if system_data.economy.total_stored_energy > 30_000 {
                    1
                } else {
                    0
                };

                if invader_core_level <= max_affordable_level {
                    // Score: higher for lower levels (easier), closer rooms, and
                    // rooms we have interest in (reserved by us).
                    let is_our_remote = room_entity
                        .and_then(|e| system_data.room_data.get(e))
                        .and_then(|rd| rd.get_dynamic_visibility_data())
                        .map(|d| d.reservation().mine())
                        .unwrap_or(false);

                    let base_score = if is_our_remote { 60.0 } else { 30.0 };
                    let level_penalty = invader_core_level as f32 * 5.0;
                    let distance_penalty = min_distance as f32 * 3.0;
                    let score = base_score - level_penalty - distance_penalty;

                    if score > 0.0 {
                        candidates.push(AttackCandidate {
                            room: room_name,
                            source: TargetSource::InvaderCore {
                                level: invader_core_level,
                            },
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
                let min_ticks_needed = (min_distance * 50) + 500;
                let power_bank_count = self
                    .active_attack_rooms
                    .iter()
                    .zip(self.active_attack_entities.iter())
                    .filter(|(_, _)| true) // Count all active -- we'll filter below
                    .count() as u32;

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
                            source: TargetSource::PowerBank {
                                power,
                                ticks_to_decay,
                            },
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
            let all_npc = threat_data
                .hostile_creeps
                .iter()
                .all(|c| crate::military::is_npc_owner(&c.owner));
            let is_our_remote = room_entity
                .and_then(|e| system_data.room_data.get(e))
                .and_then(|rd| rd.get_dynamic_visibility_data())
                .map(|d| d.reservation().mine())
                .unwrap_or(false);

            if has_invader_creeps
                && all_npc
                && features.military.attack_invaders
                && is_our_remote
                && invader_core_level == 0
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
            if room_owner_hostile
                && features.military.attack_players
                && !all_npc
                && threat_data.threat_level >= ThreatLevel::PlayerScout
            {
                // Only target hostile player rooms if we have strong economy
                // and the room is close enough to be worth contesting.
                if system_data.economy.total_stored_energy > 150_000
                    && min_distance <= 6
                {
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
                        i + 1, c.room, c.score, c.source, c.tower_count,
                        c.estimated_enemy_dps, c.estimated_enemy_heal,
                        c.has_safe_mode, c.estimated_roi
                    );
                }
            }
        }

        // ── 4. Launch attacks for top candidates ─────────────────────────

        for candidate in candidates {
            if self.active_attack_entities.len() as u32 >= self.max_concurrent_attacks {
                break;
            }

            info!(
                "[War] Launching AttackOperation for {} (source={:?}, score={:.1}, towers={}, dps={:.0}, heal={:.0})",
                candidate.room, candidate.source, candidate.score,
                candidate.tower_count, candidate.estimated_enemy_dps, candidate.estimated_enemy_heal
            );

            let reason: super::attack::AttackReason = candidate.source.into();
            let attack_entity = super::attack::AttackOperation::build_with_context(
                system_data.updater.create_entity(system_data.entities),
                Some(runtime_data.entity),
                candidate.room,
                reason,
            )
            .build();

            self.add_active_attack(attack_entity, candidate.room);

            // Force a home room rebalance on the next tick so the new attack
            // gets home rooms assigned promptly rather than waiting up to
            // RECOMPUTE_CADENCE ticks.
            self.last_recompute_tick = None;
        }
    }

    // ── Heavy recompute (every 50+ ticks) ─────────────────────────────────

    fn run_heavy_recompute(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        _runtime_data: &mut OperationExecutionRuntimeData,
    ) {
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

        let war_debug = crate::features::features().military.debug_log;

        for i in 0..self.active_attack_entities.len() {
            let entity = self.active_attack_entities[i];
            if !system_data.entities.is_alive(entity) {
                continue;
            }
            let room_name = match self.active_attack_rooms.get(i) {
                Some(r) => *r,
                None => continue,
            };

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
                let enemy_dps: f32 = player_hostiles.iter().map(|c| c.melee_dps + c.ranged_dps).sum::<f32>()
                    + tower_count as f32 * 600.0;
                let enemy_heal: f32 = player_hostiles.iter().map(|c| c.heal_per_tick).sum();
                let any_boosted = player_hostiles.iter().any(|c| c.boosted);

                if war_debug && (hostile_count > 0 || tower_count > 0) {
                    info!(
                        "[War] Threat update for {}: towers={}, dps={:.0}, heal={:.0}, hostiles={}, boosted={}, safe_mode={}/{}",
                        room_name, tower_count, enemy_dps, enemy_heal, hostile_count, any_boosted,
                        threat_data.safe_mode_active, threat_data.safe_mode_available
                    );
                }

                // Propagate updated intel to the AttackOperation via LazyUpdate
                // (we can't access the operations storage directly here).
                let safe_active = threat_data.safe_mode_active;
                let safe_available = threat_data.safe_mode_available;
                system_data.updater.exec_mut(move |world| {
                    if let Some(OperationData::Attack(ref mut attack_op)) =
                        world.write_storage::<OperationData>().get_mut(entity)
                    {
                        attack_op.update_threat_intel(
                            tower_count,
                            enemy_dps,
                            enemy_heal,
                            hostile_count,
                            safe_active,
                            safe_available,
                        );
                    }
                });
            }
        }

        // ── 3. Request visibility for rooms adjacent to our territory ────
        // This ensures threat data is fresh for rooms near our
        // borders, enabling proactive threat detection and target selection.

        let home_rooms: Vec<RoomName> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| {
                rd.get_dynamic_visibility_data()
                    .map(|d| d.owner().mine())
                    .unwrap_or(false)
            })
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
        for room_name in &self.active_attack_rooms {
            let room_entity = system_data.mapping.get_room(room_name);
            let has_fresh_data = room_entity
                .and_then(|e| system_data.threat_data.get(e))
                .map(|d| current_tick.saturating_sub(d.last_seen) < 50)
                .unwrap_or(false);

            if !has_fresh_data {
                system_data.visibility.request(VisibilityRequest::new(
                    *room_name,
                    1.0,
                    VisibilityRequestFlags::ALL,
                ));
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
                let route = system_data
                    .route_cache
                    .get_route_distance(home, room_name, current_tick);
                route.reachable && route.hops <= 2
            });

            if near_home {
                info!(
                    "[War] Proactive: player threat ({:?}) detected near territory in {} (dps={:.0}, heal={:.0})",
                    threat_data.threat_level, room_name, threat_data.estimated_dps, threat_data.estimated_heal
                );
            }
        }

        // ── 5. Rebalance home room assignments across active attacks ────
        self.reassign_home_rooms(system_data);
    }

    // ── Home room rebalancing ────────────────────────────────────────────────

    fn reassign_home_rooms(
        &self,
        system_data: &mut OperationExecutionSystemData,
    ) {
        let current_tick = game::time();
        let war_debug = crate::features::features().military.debug_log;

        // Collect home rooms with spawns: (entity, room_name).
        let home_rooms: Vec<(Entity, RoomName)> = (system_data.entities, &*system_data.room_data)
            .join()
            .filter(|(_, rd)| {
                rd.get_dynamic_visibility_data()
                    .map(|d| d.owner().mine())
                    .unwrap_or(false)
                    && rd.get_structures().map(|s| !s.spawns().is_empty()).unwrap_or(false)
            })
            .map(|(e, rd)| (e, rd.name))
            .collect();

        if home_rooms.is_empty() {
            return;
        }

        // Collect active attacks: (entity, target_room_name).
        let mut attacks: Vec<(Entity, RoomName)> = Vec::new();
        for i in 0..self.active_attack_entities.len() {
            let entity = self.active_attack_entities[i];
            if !system_data.entities.is_alive(entity) {
                continue;
            }
            if let Some(&target_room) = self.active_attack_rooms.get(i) {
                attacks.push((entity, target_room));
            }
        }

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
                        let route = system_data
                            .route_cache
                            .get_route_distance(*home_name, *target, current_tick);
                        if route.reachable { route.hops } else { u32::MAX }
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
        attack_order.sort_by_key(|&ai| {
            distances[ai]
                .iter()
                .filter(|&&d| d < u32::MAX)
                .count()
        });

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
                let room_names: Vec<String> = home_indices
                    .iter()
                    .map(|&hi| home_rooms[hi].1.to_string())
                    .collect();
                info!(
                    "[War] Assign {} -> spawn: [{}]",
                    attacks[ai].1,
                    room_names.join(", ")
                );
            }

            // Don't overwrite existing home rooms with an empty assignment.
            // This can happen when route distances are unreachable or all
            // home rooms were consumed by higher-priority attacks.
            if rooms.is_empty() {
                if war_debug {
                    info!(
                        "[War] Skipping empty home room assignment for {} (keeping existing)",
                        attacks[ai].1
                    );
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
                if let Some(OperationData::Attack(ref mut attack_op)) =
                    world.write_storage::<OperationData>().get_mut(attack_entity)
                {
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
        self.active_attack_rooms.contains(&room)
    }

    /// Compute the minimum route distance from any home room to a target room.
    fn min_distance_to_homes(
        &self,
        target: RoomName,
        home_rooms: &[RoomName],
        route_cache: &mut crate::military::economy::RoomRouteCache,
        current_tick: u32,
    ) -> u32 {
        home_rooms
            .iter()
            .map(|&home| {
                let route = route_cache.get_route_distance(home, target, current_tick);
                if route.reachable { route.hops } else { u32::MAX }
            })
            .min()
            .unwrap_or(u32::MAX)
    }

    /// Get the attack operation entity targeting a given room, if any.
    pub fn get_attack_for_room(&self, room: RoomName) -> Option<Entity> {
        self.active_attack_rooms
            .iter()
            .position(|r| *r == room)
            .and_then(|idx| self.active_attack_entities.get(idx).copied())
    }

    /// Add a new active attack, maintaining parallel vecs.
    fn add_active_attack(&mut self, entity: Entity, room: RoomName) {
        self.active_attack_entities.push(entity);
        self.active_attack_rooms.push(room);
    }

    /// Remove an active attack by entity, maintaining parallel vecs.
    fn remove_active_attack(&mut self, entity: Entity) {
        if let Some(idx) = self.active_attack_entities.iter().position(|e| *e == entity) {
            self.active_attack_entities.remove(idx);
            if idx < self.active_attack_rooms.len() {
                self.active_attack_rooms.remove(idx);
            }
        }
    }

    /// Clean up dead/completed attack operations from tracking.
    fn cleanup_dead_attacks(&mut self, system_data: &OperationExecutionSystemData) {
        let mut i = 0;
        while i < self.active_attack_entities.len() {
            if !system_data.entities.is_alive(self.active_attack_entities[i]) {
                self.active_attack_entities.remove(i);
                if i < self.active_attack_rooms.len() {
                    self.active_attack_rooms.remove(i);
                }
            } else {
                i += 1;
            }
        }

        // Safety: ensure vecs stay in sync.
        self.active_attack_rooms.truncate(self.active_attack_entities.len());
    }

    fn should_run_tier(&self, last_tick: Option<u32>, cadence: u32) -> bool {
        last_tick.map(|t| game::time() - t >= cadence).unwrap_or(true)
    }

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
        let before = self.active_attack_entities.len();
        let mut i = 0;
        while i < self.active_attack_entities.len() {
            if !is_valid(self.active_attack_entities[i]) {
                error!(
                    "INTEGRITY: dead attack entity {:?} removed from WarOperation",
                    self.active_attack_entities[i]
                );
                self.active_attack_entities.remove(i);
                if i < self.active_attack_rooms.len() {
                    self.active_attack_rooms.remove(i);
                }
            } else {
                i += 1;
            }
        }
        self.active_attack_rooms.truncate(self.active_attack_entities.len());
        if self.active_attack_entities.len() < before {
            warn!(
                "INTEGRITY: removed {} dead attack ref(s) from WarOperation",
                before - self.active_attack_entities.len()
            );
        }
    }

    fn describe_operation(&self, _ctx: &OperationDescribeContext) -> SummaryContent {
        let features = crate::features::features();
        let mut children = Vec::new();

        // Offense section.
        {
            let mut offense_items = Vec::new();
            for i in 0..self.active_attack_rooms.len() {
                if i < self.active_attack_entities.len() {
                    offense_items.push(format!("-> {}", self.active_attack_rooms[i]));
                }
            }
            let label = if features.military.offense {
                if offense_items.is_empty() {
                    format!("Offense: ON (cap {})", self.max_concurrent_attacks)
                } else {
                    format!(
                        "Offense: {}/{} active",
                        offense_items.len(),
                        self.max_concurrent_attacks
                    )
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
        // Defense scan: every 1-2 ticks (fast, cheap).
        if self.should_run_tier(self.last_defense_tick, DEFENSE_CADENCE) {
            self.last_defense_tick = Some(game::time());
            self.run_defense_scan(system_data, runtime_data);
        }

        // Offense evaluation: every 10-20 ticks.
        if self.should_run_tier(self.last_offense_tick, OFFENSE_CADENCE) {
            self.last_offense_tick = Some(game::time());
            self.run_offense_evaluation(system_data, runtime_data);
        }

        // Heavy recompute: every 50+ ticks.
        if self.should_run_tier(self.last_recompute_tick, RECOMPUTE_CADENCE) {
            self.last_recompute_tick = Some(game::time());
            self.run_heavy_recompute(system_data, runtime_data);
        }

        // WarOperation is a singleton -- it never completes.
        Ok(OperationResult::Running)
    }
}
