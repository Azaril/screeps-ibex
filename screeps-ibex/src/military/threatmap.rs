use crate::jobs::utility::dismantle::breach_path_total_hits;
use crate::jobs::utility::dismantlebehavior::breach_blockers;
use crate::room::data::{RoomData, RoomStructureData};
use screeps::*;
use screeps_foreman::terrain::FastRoomTerrain;
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
    /// Energy in each hostile tower, parallel to `hostile_tower_positions` (ADR 0020 §12.2). A drained
    /// tower (< `TOWER_ENERGY_COST`) deals no damage → the force-sizing oracle counts only energized
    /// towers for the out-heal requirement and sizes the tower-drain path from Σ energy. (`#[serde(default)]`
    /// is forward-compat only; bincode is positional, so the `WORLD_FORMAT_VERSION` 14→15 bump is the gate.)
    #[serde(default)]
    pub tower_energy: Vec<u32>,
    /// Rampart/wall hits on the BREACH CORRIDOR to the room's invader core — from `breach_path_blockers`,
    /// counting ONLY the corridor blockers, NOT a room-wide rampart sum (ADR 0020 §12.3). The breach-time
    /// input for the force-sizing oracle. 0 = no core / already reachable / not visible when last assessed.
    #[serde(default)]
    pub breach_rampart_hits: u32,
    /// Estimated defensive repair/tick of the breach target (tower repair of ramparts + enemy WORK
    /// repair), added to breach cost. 0 for invader cores (no repairers); computed for player targets in
    /// a later phase (P5). Reserved now so adding player-repair modelling later needs no further WFV bump.
    #[serde(default)]
    pub repair_per_tick: u32,
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
            let mut estimated_repair: u32 = 0;

            if let Some(creeps) = room_data.get_creeps() {
                for hostile in creeps.hostile() {
                    let info = analyze_hostile_creep(hostile);
                    estimated_dps += info.melee_dps + info.ranged_dps;
                    estimated_heal += info.heal_per_tick;
                    // Defenders repair the breach target (e.g. invader-stronghold creeps repairing
                    // ramparts). Conservative proxy: all hostile WORK repairs at REPAIR_POWER/part
                    // (over-estimating repair makes the breach oracle defer rather than feed a losing
                    // squad). 0 for level-0 cores (no defenders → trivial). The richer "which creeps
                    // actually repair the breach target + tower-heal-of-defenders" model is P-FORCE D5b.
                    estimated_repair += info.work_parts * REPAIR_POWER;
                    hostile_creep_infos.push(info);
                }
            }

            // Gather hostile tower positions + energy from structures (ADR 0020 §12.2: a drained tower
            // deals no damage, so the force oracle needs per-tower energy, not just positions).
            let mut hostile_tower_positions = Vec::new();
            let mut tower_energy = Vec::new();
            if let Some(structures) = room_data.get_structures() {
                for tower in structures.towers() {
                    if !tower.my() {
                        hostile_tower_positions.push(tower.pos());
                        tower_energy.push(tower.store().get_used_capacity(Some(ResourceType::Energy)));
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
            let has_invader_core = room_data.get_structures().map(|s| !s.invader_cores().is_empty()).unwrap_or(false);

            // Breach-corridor rampart hits to the invader core (ADR 0020 §12.3) — the breach-cost input
            // for the force-sizing oracle. Bounded to core rooms (one Dijkstra, rare) so it stays cheap.
            let breach_rampart_hits = if has_invader_core {
                room_data.get_structures().map(|s| breach_rampart_hits_to_core(room_data, &s)).unwrap_or(0)
            } else {
                0
            };

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
                        tower_energy,
                        breach_rampart_hits,
                        repair_per_tick: estimated_repair,
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

/// Breach-corridor rampart hits to the room's invader core (ADR 0020 §12.3): the cheapest breach cost
/// from the core's nearest room edge, counting ONLY the corridor blockers (the breach-relevant
/// ramparts/walls), NOT a room-wide rampart sum. Reuses the shared `breach_path_blockers` Dijkstra
/// kernel (no one-off scan). Returns 0 when there is no core, terrain is unavailable, or the core is
/// already reachable. `u32::MAX` horizon so every dismantlable blocker stays on-corridor (the oracle,
/// not the horizon, decides feasibility).
fn breach_rampart_hits_to_core(room_data: &RoomData, structures: &RoomStructureData) -> u32 {
    let core_pos = match structures.invader_cores().first() {
        Some(core) => core.pos(),
        None => return 0,
    };
    let room = match game::rooms().get(room_data.name) {
        Some(room) => room,
        None => return 0,
    };
    let terrain = FastRoomTerrain::new(room.get_terrain().get_raw_buffer().to_vec());
    let is_wall = |x: u8, y: u8| terrain.is_wall(x, y);
    let blockers = breach_blockers(structures.all(), u32::MAX);

    let (gx, gy) = (core_pos.x().u8(), core_pos.y().u8());
    let start = nearest_edge_tile(gx, gy);
    breach_path_total_hits(&is_wall, &blockers, start, (gx, gy)).unwrap_or(0)
}

/// The room-edge tile closest to `(x, y)` — the shortest breach approach to a core (one representative
/// entry; a core's rampart shell is ~symmetric so a single nearest-edge corridor is a sound estimate).
fn nearest_edge_tile(x: u8, y: u8) -> (u8, u8) {
    let (to_left, to_right, to_top, to_bottom) = (x, 49 - x, y, 49 - y);
    let nearest = to_left.min(to_right).min(to_top).min(to_bottom);
    if nearest == to_left {
        (0, y)
    } else if nearest == to_right {
        (49, y)
    } else if nearest == to_top {
        (x, 0)
    } else {
        (x, 49)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::military::{NPC_INVADER, NPC_SOURCE_KEEPER};
    use screeps::RoomCoordinate;

    #[test]
    fn nearest_edge_tile_projects_to_the_closest_edge() {
        assert_eq!(nearest_edge_tile(3, 25), (0, 25), "near the left edge");
        assert_eq!(nearest_edge_tile(46, 25), (49, 25), "near the right edge");
        assert_eq!(nearest_edge_tile(25, 4), (25, 0), "near the top edge");
        assert_eq!(nearest_edge_tile(25, 45), (25, 49), "near the bottom edge");
    }

    // Informational snapshot of current `classify_threat` behavior. This is
    // NOT a spec -- it pins what the classifier does today so behavioral
    // drift during Phase 0 (and the rewrite increments) is visible.

    fn hostile(owner: &str, melee_dps: f32, ranged_dps: f32, heal_per_tick: f32, boosted: bool) -> HostileCreepInfo {
        HostileCreepInfo {
            position: Position::new(
                RoomCoordinate::new(25).expect("valid coordinate"),
                RoomCoordinate::new(25).expect("valid coordinate"),
                "E0N0".parse().expect("valid room name"),
            ),
            owner: owner.to_string(),
            hits: 1000,
            hits_max: 1000,
            melee_dps,
            ranged_dps,
            heal_per_tick,
            tough_hp: 0.0,
            work_parts: 0,
            boosted,
        }
    }

    #[test]
    fn classify_threat_snapshot_no_hostiles() {
        assert_eq!(classify_threat(&[], false, false), ThreatLevel::None);
        // An invader core with no creeps still registers as Invader.
        assert_eq!(classify_threat(&[], false, true), ThreatLevel::Invader);
        // Nukes dominate everything.
        assert_eq!(classify_threat(&[], true, false), ThreatLevel::NukeIncoming);
    }

    #[test]
    fn classify_threat_snapshot_npc_hostiles() {
        // Source Keepers alone are residents, not a threat.
        let sk = [hostile(NPC_SOURCE_KEEPER, 120.0, 0.0, 0.0, false)];
        assert_eq!(classify_threat(&sk, false, false), ThreatLevel::SourceKeeper);

        // Source Keepers plus an invader core escalate to Invader.
        assert_eq!(classify_threat(&sk, false, true), ThreatLevel::Invader);

        // Invader NPCs classify as Invader.
        let invader = [hostile(NPC_INVADER, 30.0, 0.0, 0.0, false)];
        assert_eq!(classify_threat(&invader, false, false), ThreatLevel::Invader);
    }

    #[test]
    fn classify_threat_snapshot_player_hostiles() {
        // Unarmed player creep: scout.
        let scout = [hostile("somePlayer", 0.0, 0.0, 0.0, false)];
        assert_eq!(classify_threat(&scout, false, false), ThreatLevel::PlayerScout);

        // Single armed, unboosted player creep: raid.
        let raid = [hostile("somePlayer", 30.0, 0.0, 0.0, false)];
        assert_eq!(classify_threat(&raid, false, false), ThreatLevel::PlayerRaid);

        // Any boosted attacker escalates to siege.
        let boosted = [hostile("somePlayer", 30.0, 0.0, 0.0, true)];
        assert_eq!(classify_threat(&boosted, false, false), ThreatLevel::PlayerSiege);

        // High sustained DPS with healing support escalates to siege.
        let sustained = [
            hostile("somePlayer", 150.0, 60.0, 0.0, false),
            hostile("somePlayer", 0.0, 0.0, 120.0, false),
        ];
        assert_eq!(classify_threat(&sustained, false, false), ThreatLevel::PlayerSiege);

        // Four or more armed player creeps escalate to siege.
        let four: Vec<_> = (0..4).map(|_| hostile("somePlayer", 30.0, 0.0, 0.0, false)).collect();
        assert_eq!(classify_threat(&four, false, false), ThreatLevel::PlayerSiege);
    }
}
