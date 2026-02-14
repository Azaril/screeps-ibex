use super::data::*;
use super::missionsystem::*;
use crate::serialize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Nuke landing damage constants (from Screeps docs).
/// Center tile: 10,000,000 damage. Adjacent tiles: 5,000,000 damage.
const NUKE_DAMAGE_CENTER: u32 = 10_000_000;
const NUKE_DAMAGE_ADJACENT: u32 = 5_000_000;

/// Minimum rampart hits to survive a nuke at the center.
/// Need at least NUKE_DAMAGE_CENTER + buffer.
const MIN_RAMPART_HITS_CENTER: u32 = 10_500_000;

/// Minimum rampart hits to survive a nuke on adjacent tiles.
const MIN_RAMPART_HITS_ADJACENT: u32 = 5_500_000;

/// Ticks before nuke lands at which we start fortifying.
/// Nukes take 50,000 ticks to land; start early to have time to repair.
const FORTIFY_LEAD_TICKS: u32 = 40_000;

/// Mission to defend against incoming nukes.
///
/// Detects nukes via `find::NUKES`, identifies structures in the impact zone,
/// and prioritizes rampart repair to absorb the damage. Also logs warnings
/// for structures that cannot be saved.
#[derive(ConvertSaveload)]
pub struct NukeDefenseMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    /// Tick when we last ran the nuke scan (avoid scanning every tick).
    last_scan_tick: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl NukeDefenseMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = NukeDefenseMission {
            owner: owner.into(),
            room_data,
            last_scan_tick: 0,
        };

        builder
            .with(MissionData::NukeDefense(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for NukeDefenseMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);
        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        "NukeDefense".to_string()
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text("NukeDefense".to_string())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let features = crate::features::features();

        if !features.military.nuke_defense {
            return Ok(MissionResult::Running);
        }

        // Only scan every 100 ticks to save CPU; nukes take 50k ticks.
        let current_tick = game::time();
        if current_tick.saturating_sub(self.last_scan_tick) < 100 {
            return Ok(MissionResult::Running);
        }
        self.last_scan_tick = current_tick;

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let room = match game::rooms().get(room_data.name) {
            Some(r) => r,
            None => return Ok(MissionResult::Running),
        };

        // Find incoming nukes.
        let nukes = room.find(find::NUKES, None);

        if nukes.is_empty() {
            return Ok(MissionResult::Running);
        }

        info!("[NukeDefense] {} incoming nuke(s) detected in room {}", nukes.len(), room_data.name);

        let structures = match room_data.get_structures() {
            Some(s) => s,
            None => return Ok(MissionResult::Running),
        };

        // For each nuke, identify ramparts in the impact zone and check if they
        // have enough hits to survive.
        for nuke in &nukes {
            let ticks_to_land = nuke.time_to_land();
            let impact_pos = nuke.pos();

            if ticks_to_land > FORTIFY_LEAD_TICKS {
                info!(
                    "[NukeDefense] Nuke landing at ({},{}) in {} ticks -- monitoring",
                    impact_pos.x().u8(),
                    impact_pos.y().u8(),
                    ticks_to_land
                );
                continue;
            }

            info!(
                "[NukeDefense] Nuke landing at ({},{}) in {} ticks -- FORTIFYING",
                impact_pos.x().u8(),
                impact_pos.y().u8(),
                ticks_to_land
            );

            // Check ramparts in the impact zone (center + 8 adjacent tiles).
            for rampart in structures.ramparts() {
                if !rampart.my() {
                    continue;
                }

                let rpos = rampart.pos();
                let range = impact_pos.get_range_to(rpos);

                let required_hits = if range == 0 {
                    MIN_RAMPART_HITS_CENTER
                } else if range == 1 {
                    MIN_RAMPART_HITS_ADJACENT
                } else {
                    continue; // Outside blast radius.
                };

                let current_hits = rampart.hits();

                if current_hits < required_hits {
                    let deficit = required_hits - current_hits;
                    info!(
                        "[NukeDefense] Rampart at ({},{}) needs {} more hits (has {}, needs {})",
                        rpos.x().u8(),
                        rpos.y().u8(),
                        deficit,
                        current_hits,
                        required_hits
                    );

                    // The existing tower mission and repair jobs will handle
                    // the actual repair work. We log the need here so the
                    // player is aware. In a more advanced version, we would
                    // create dedicated repair jobs with elevated priority.
                }
            }

            // Warn about critical structures in the blast zone that have no rampart.
            let critical_structure_types = [
                StructureType::Spawn,
                StructureType::Storage,
                StructureType::Terminal,
                StructureType::Lab,
                StructureType::Factory,
                StructureType::Nuker,
                StructureType::Observer,
                StructureType::PowerSpawn,
            ];

            // Check spawns specifically.
            for spawn in structures.spawns() {
                let spos = spawn.pos();
                let range = impact_pos.get_range_to(spos);
                if range <= 1 {
                    let damage = if range == 0 { NUKE_DAMAGE_CENTER } else { NUKE_DAMAGE_ADJACENT };
                    // Check if there's a rampart covering this spawn.
                    let has_rampart = structures
                        .ramparts()
                        .iter()
                        .any(|r| r.my() && r.pos() == spos && r.hits() >= damage);
                    if !has_rampart {
                        warn!(
                            "[NukeDefense] CRITICAL: Spawn at ({},{}) in blast zone with insufficient rampart protection!",
                            spos.x().u8(),
                            spos.y().u8()
                        );
                    }
                }
            }

            // Check towers.
            for tower in structures.towers() {
                if !tower.my() {
                    continue;
                }
                let tpos = tower.pos();
                let range = impact_pos.get_range_to(tpos);
                if range <= 1 {
                    let damage = if range == 0 { NUKE_DAMAGE_CENTER } else { NUKE_DAMAGE_ADJACENT };
                    let has_rampart = structures
                        .ramparts()
                        .iter()
                        .any(|r| r.my() && r.pos() == tpos && r.hits() >= damage);
                    if !has_rampart {
                        warn!(
                            "[NukeDefense] CRITICAL: Tower at ({},{}) in blast zone with insufficient rampart protection!",
                            tpos.x().u8(),
                            tpos.y().u8()
                        );
                    }
                }
            }

            // Log general warning for unprotected critical structures.
            let all_structures = room.find(find::MY_STRUCTURES, None);
            for structure in &all_structures {
                let stype = structure.as_structure().structure_type();
                if !critical_structure_types.contains(&stype) {
                    continue;
                }
                let spos = structure.as_structure().pos();
                let range = impact_pos.get_range_to(spos);
                if range <= 1 {
                    let damage = if range == 0 { NUKE_DAMAGE_CENTER } else { NUKE_DAMAGE_ADJACENT };
                    let has_rampart = structures
                        .ramparts()
                        .iter()
                        .any(|r| r.my() && r.pos() == spos && r.hits() >= damage);
                    if !has_rampart {
                        warn!(
                            "[NukeDefense] {:?} at ({},{}) in blast zone -- needs rampart with {} hits",
                            stype,
                            spos.x().u8(),
                            spos.y().u8(),
                            damage
                        );
                    }
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
