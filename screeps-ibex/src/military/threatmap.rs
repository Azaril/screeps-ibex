use screeps::*;
use serde::{Deserialize, Serialize};
use specs::prelude::*;
use specs::Component;

/// Analyzed information about a single hostile creep.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostileCreepInfo {
    pub position: Position,
    pub owner: String,
    pub hits: u32,
    pub hits_max: u32,
    /// Melee damage per tick (ATTACK parts * 30).
    pub melee_dps: f32,
    /// Ranged damage per tick (RANGED_ATTACK parts * 10).
    pub ranged_dps: f32,
    /// Heal per tick (HEAL parts * 12 for adjacent, * 4 for ranged).
    pub heal_per_tick: f32,
    /// Total effective HP from TOUGH parts (accounting for boosts).
    pub tough_hp: f32,
    /// Number of WORK parts (relevant for dismantle damage).
    pub work_parts: u32,
    /// Whether any body part is boosted.
    pub boosted: bool,
}

/// Information about an incoming nuke.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NukeInfo {
    /// Game tick when the nuke will land.
    pub landing_tick: u32,
    /// Position the nuke is targeted at.
    pub impact_position: Position,
}

/// Threat classification for a room, driving defense escalation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ThreatLevel {
    /// No threats detected.
    #[default]
    None,
    /// Source Keepers only -- permanent room residents, not a threat to respond to.
    SourceKeeper,
    /// NPC invaders only (not Source Keepers).
    Invader,
    /// Player scout (unarmed creep, no attack parts).
    PlayerScout,
    /// Player raid (armed creeps, limited force).
    PlayerRaid,
    /// Player siege (sustained attack with significant force).
    PlayerSiege,
    /// Incoming nuke detected.
    NukeIncoming,
}

/// Per-room threat intelligence, attached as a component to room entities.
///
/// Persisted across VM reloads via the standard component serialization
/// pipeline (segments 50-52). Updated each tick by `ThreatAssessmentSystem`
/// for visible rooms; stale data is retained for rooms that lose visibility
/// (e.g. scout dies) and expired after `THREAT_DATA_MAX_AGE` ticks.
///
/// The component is removed from a room entity when the room is confirmed
/// safe (visible with no threats) or when the data expires.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Component)]
#[storage(DenseVecStorage)]
pub struct RoomThreatData {
    pub threat_level: ThreatLevel,
    #[serde(default)]
    pub hostile_creeps: Vec<HostileCreepInfo>,
    #[serde(default)]
    pub hostile_tower_positions: Vec<Position>,
    #[serde(default)]
    pub incoming_nukes: Vec<NukeInfo>,
    /// Game tick when this data was last updated.
    #[serde(default)]
    pub last_seen: u32,
    /// Total hostile damage per tick across all hostiles.
    #[serde(default)]
    pub estimated_dps: f32,
    /// Total hostile healing per tick across all hostiles.
    #[serde(default)]
    pub estimated_heal: f32,
    /// Whether safe mode is currently active on the room's controller.
    #[serde(default)]
    pub safe_mode_active: bool,
    /// Whether safe mode charges are available on the room's controller.
    #[serde(default)]
    pub safe_mode_available: bool,
}

/// Analyze a hostile creep's body to produce a `HostileCreepInfo`.
pub fn analyze_hostile_creep(creep: &Creep) -> HostileCreepInfo {
    let body = creep.body();

    let mut melee_dps: f32 = 0.0;
    let mut ranged_dps: f32 = 0.0;
    let mut heal_per_tick: f32 = 0.0;
    let mut tough_hp: f32 = 0.0;
    let mut work_parts: u32 = 0;
    let mut boosted = false;

    for part_info in body.iter() {
        // Only count parts that still have HP.
        if part_info.hits() == 0 {
            continue;
        }

        let boost_multiplier = part_info
            .boost()
            .map(|_| {
                boosted = true;
                // Conservative estimate: assume T3 boosts for threat assessment.
                // Real boost detection would require checking the specific compound.
                4.0_f32
            })
            .unwrap_or(1.0);

        match part_info.part() {
            Part::Attack => {
                melee_dps += 30.0 * boost_multiplier;
            }
            Part::RangedAttack => {
                ranged_dps += 10.0 * boost_multiplier;
            }
            Part::Heal => {
                heal_per_tick += 12.0 * boost_multiplier;
            }
            Part::Tough => {
                // Boosted tough reduces damage taken. T3 XGHO2 = 70% reduction.
                // Effective HP of the tough part itself.
                if part_info.boost().is_some() {
                    tough_hp += 100.0 / 0.3; // ~333 effective HP per boosted tough
                } else {
                    tough_hp += 100.0;
                }
            }
            Part::Work => {
                work_parts += 1;
            }
            _ => {}
        }
    }

    let owner = creep.owner().username();

    HostileCreepInfo {
        position: creep.pos(),
        owner,
        hits: creep.hits(),
        hits_max: creep.hits_max(),
        melee_dps,
        ranged_dps,
        heal_per_tick,
        tough_hp,
        work_parts,
        boosted,
    }
}

/// Classify the threat level of a set of hostile creeps.
pub fn classify_threat(hostile_creeps: &[HostileCreepInfo], has_nukes: bool, has_invader_core: bool) -> ThreatLevel {
    if has_nukes {
        return ThreatLevel::NukeIncoming;
    }

    if hostile_creeps.is_empty() {
        // An invader core without accompanying creeps should still
        // register as at least ThreatLevel::Invader so the offense
        // evaluation loop can see the room and plan an attack.
        if has_invader_core {
            return ThreatLevel::Invader;
        }
        return ThreatLevel::None;
    }

    let total_dps: f32 = hostile_creeps.iter().map(|c| c.melee_dps + c.ranged_dps).sum();
    let total_heal: f32 = hostile_creeps.iter().map(|c| c.heal_per_tick).sum();
    let any_boosted = hostile_creeps.iter().any(|c| c.boosted);
    let has_combat_parts = hostile_creeps
        .iter()
        .any(|c| c.melee_dps > 0.0 || c.ranged_dps > 0.0 || c.work_parts > 0);

    // Check if all hostiles are NPCs and distinguish Source Keepers from Invaders.
    let all_npc = hostile_creeps.iter().all(|c| super::is_npc_owner(&c.owner));
    let all_source_keepers = hostile_creeps.iter().all(|c| super::is_source_keeper_owner(&c.owner));
    let has_invaders = hostile_creeps.iter().any(|c| super::is_invader_owner(&c.owner));

    if all_npc {
        if all_source_keepers && !has_invader_core {
            // Only Source Keepers -- permanent residents, not a threat.
            return ThreatLevel::SourceKeeper;
        }
        // Has actual Invader NPCs, an invader core, or a mix with Source Keepers.
        return ThreatLevel::Invader;
    }

    // Mixed NPC + player, or pure player -- ignore the NPC classification
    // and fall through to player threat assessment.
    let _ = has_invaders;

    if !has_combat_parts {
        return ThreatLevel::PlayerScout;
    }

    // Siege: boosted attackers, or high sustained DPS with healing support.
    if any_boosted || (total_dps > 200.0 && total_heal > 100.0) || hostile_creeps.len() >= 4 {
        return ThreatLevel::PlayerSiege;
    }

    ThreatLevel::PlayerRaid
}

/// Maximum age (in ticks) before a RoomThreatData component is removed.
/// Prevents stale data from driving decisions long after the threat may
/// have moved.
const THREAT_DATA_MAX_AGE: u32 = 500;

/// System that populates `RoomThreatData` components on room entities each
/// tick from visible room data.
///
/// Runs in the pre-pass dispatcher after room data is updated.
///
/// - For visible rooms with threats: upserts the `RoomThreatData` component.
/// - For visible rooms with no threats: removes the component.
/// - For non-visible rooms: the component is retained from previous ticks.
/// - Stale components (older than `THREAT_DATA_MAX_AGE`) are removed.
pub struct ThreatAssessmentSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for ThreatAssessmentSystem {
    type SystemData = (
        Entities<'a>,
        ReadStorage<'a, crate::room::data::RoomData>,
        WriteStorage<'a, RoomThreatData>,
    );

    fn run(&mut self, (entities, room_data_storage, mut threat_data_storage): Self::SystemData) {
        let current_tick = game::time();

        // Expire stale threat data from rooms that haven't been refreshed.
        let stale_entities: Vec<Entity> = (&entities, &threat_data_storage)
            .join()
            .filter(|(_, td)| current_tick.saturating_sub(td.last_seen) > THREAT_DATA_MAX_AGE)
            .map(|(e, _)| e)
            .collect();

        for entity in stale_entities {
            threat_data_storage.remove(entity);
        }

        // Update threat data for visible rooms.
        for (entity, room_data) in (&entities, &room_data_storage).join() {
            let dynamic_vis = match room_data.get_dynamic_visibility_data() {
                Some(d) => d,
                None => continue,
            };

            // Only update rooms we can currently see.
            if !dynamic_vis.visible() {
                continue;
            }

            let mut hostile_creep_infos = Vec::new();
            let mut estimated_dps: f32 = 0.0;
            let mut estimated_heal: f32 = 0.0;

            if let Some(creeps) = room_data.get_creeps() {
                for hostile in creeps.hostile() {
                    let info = analyze_hostile_creep(hostile);
                    estimated_dps += info.melee_dps + info.ranged_dps;
                    estimated_heal += info.heal_per_tick;
                    hostile_creep_infos.push(info);
                }
            }

            // Gather hostile tower positions from structures.
            let mut hostile_tower_positions = Vec::new();
            if let Some(structures) = room_data.get_structures() {
                for tower in structures.towers() {
                    if !tower.my() {
                        hostile_tower_positions.push(tower.pos());
                    }
                }
            }

            // Detect incoming nukes via the game room object.
            let mut incoming_nukes = Vec::new();
            if let Some(room) = game::rooms().get(room_data.name) {
                let nukes = room.find(find::NUKES, None);
                for nuke in &nukes {
                    incoming_nukes.push(NukeInfo {
                        landing_tick: nuke.time_to_land() + current_tick,
                        impact_position: nuke.pos(),
                    });
                }
            }

            // Detect safe mode status from cached structure data.
            let (safe_mode_active, safe_mode_available) = room_data
                .get_structures()
                .and_then(|s| {
                    s.controllers()
                        .first()
                        .map(|c| (c.safe_mode().unwrap_or(0) > 0, c.safe_mode_available() > 0))
                })
                .unwrap_or((false, false));

            let has_nukes = !incoming_nukes.is_empty();
            let has_invader_core = room_data
                .get_structures()
                .map(|s| !s.invader_cores().is_empty())
                .unwrap_or(false);

            let threat_level = classify_threat(&hostile_creep_infos, has_nukes, has_invader_core);

            // Persist when there are threats, nukes, invader cores, or an
            // enemy room has safe mode (relevant for attack planning even if
            // no hostiles are currently present).
            let enemy_safe_mode_relevant = (safe_mode_active || safe_mode_available)
                && room_data
                    .get_structures()
                    .and_then(|s| s.controllers().first().map(|c| !c.my()))
                    .unwrap_or(false);

            if threat_level != ThreatLevel::None || !incoming_nukes.is_empty() || enemy_safe_mode_relevant || has_invader_core {
                // Upsert: insert or overwrite the component with fresh data.
                let _ = threat_data_storage.insert(
                    entity,
                    RoomThreatData {
                        threat_level,
                        hostile_creeps: hostile_creep_infos,
                        hostile_tower_positions,
                        incoming_nukes,
                        last_seen: current_tick,
                        estimated_dps,
                        estimated_heal,
                        safe_mode_active,
                        safe_mode_available,
                    },
                );
            } else {
                // Room is visible and has no threats -- remove stale component.
                threat_data_storage.remove(entity);
            }
        }
    }
}
