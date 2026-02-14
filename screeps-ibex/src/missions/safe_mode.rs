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

/// Threshold: if total hostile DPS exceeds this and we have critical structures
/// at risk, consider safe mode.
const SAFE_MODE_DPS_THRESHOLD: f32 = 300.0;

/// Minimum hits on a critical structure before we trigger safe mode.
/// If any spawn or storage drops below this, it's an emergency.
const CRITICAL_STRUCTURE_MIN_HITS: u32 = 5000;

/// Cooldown between safe mode evaluations (ticks).
const EVAL_INTERVAL: u32 = 5;

/// Mission to evaluate and activate safe mode as a last resort defense.
///
/// Safe mode prevents hostile creeps from performing any actions in the room
/// for 20,000 ticks. It should only be activated when:
/// 1. Critical structures (spawns, storage) are about to be destroyed.
/// 2. Towers and defenders cannot hold the room.
/// 3. Safe mode is available and not on cooldown.
///
/// This mission monitors the room and activates safe mode when conditions
/// are met, logging the decision for the player.
#[derive(ConvertSaveload)]
pub struct SafeModeMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    /// Tick when we last evaluated.
    last_eval_tick: u32,
    /// Whether safe mode has been activated by this mission (to avoid re-triggering).
    activated: bool,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SafeModeMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SafeModeMission {
            owner: owner.into(),
            room_data,
            last_eval_tick: 0,
            activated: false,
        };

        builder
            .with(MissionData::SafeMode(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SafeModeMission {
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
        if self.activated {
            "SafeMode [ACTIVATED]".to_string()
        } else {
            "SafeMode [monitoring]".to_string()
        }
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        if self.activated {
            crate::visualization::SummaryContent::Text("SafeMode [ACTIVATED]".to_string())
        } else {
            crate::visualization::SummaryContent::Text("SafeMode [monitoring]".to_string())
        }
    }

    fn run_mission(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
    ) -> Result<MissionResult, String> {
        let features = crate::features::features();

        if !features.military.safe_mode {
            return Ok(MissionResult::Running);
        }

        // If we already activated safe mode, just monitor until it expires.
        if self.activated {
            return Ok(MissionResult::Running);
        }

        let current_tick = game::time();
        if current_tick.saturating_sub(self.last_eval_tick) < EVAL_INTERVAL {
            return Ok(MissionResult::Running);
        }
        self.last_eval_tick = current_tick;

        let room_data = system_data
            .room_data
            .get(self.room_data)
            .ok_or("Expected room data")?;

        let room = match game::rooms().get(room_data.name) {
            Some(r) => r,
            None => return Ok(MissionResult::Running),
        };

        let structures = match room_data.get_structures() {
            Some(s) => s,
            None => return Ok(MissionResult::Running),
        };

        let creeps = match room_data.get_creeps() {
            Some(c) => c,
            None => return Ok(MissionResult::Running),
        };

        let hostiles = creeps.hostile();
        if hostiles.is_empty() {
            return Ok(MissionResult::Running);
        }

        // Calculate total hostile DPS.
        let mut total_hostile_dps: f32 = 0.0;
        let mut has_work_parts = false;

        for hostile in hostiles {
            for part_info in hostile.body().iter() {
                if part_info.hits() == 0 {
                    continue;
                }
                let boost_mult = if part_info.boost().is_some() { 4.0 } else { 1.0 };
                match part_info.part() {
                    Part::Attack => total_hostile_dps += 30.0 * boost_mult,
                    Part::RangedAttack => total_hostile_dps += 10.0 * boost_mult,
                    Part::Work => {
                        has_work_parts = true;
                        // Dismantle damage: 50 per WORK part per tick.
                        total_hostile_dps += 50.0 * boost_mult;
                    }
                    _ => {}
                }
            }
        }

        // Check if any critical structure is in danger.
        let mut critical_in_danger = false;

        // Check spawns.
        for spawn in structures.spawns() {
            if spawn.hits() < CRITICAL_STRUCTURE_MIN_HITS {
                warn!(
                    "[SafeMode] Spawn '{}' at critical HP: {}/{}",
                    spawn.name(),
                    spawn.hits(),
                    spawn.hits_max()
                );
                critical_in_danger = true;
            }
        }

        // Check if hostiles are dismantling (WORK parts near structures).
        if has_work_parts && total_hostile_dps > SAFE_MODE_DPS_THRESHOLD {
            // Check if any hostile with WORK parts is adjacent to a critical structure.
            for hostile in hostiles {
                let has_work = hostile.body().iter().any(|p| p.part() == Part::Work && p.hits() > 0);
                if !has_work {
                    continue;
                }

                for spawn in structures.spawns() {
                    if hostile.pos().get_range_to(spawn.pos()) <= 1 {
                        warn!(
                            "[SafeMode] Dismantler adjacent to spawn '{}'!",
                            spawn.name()
                        );
                        critical_in_danger = true;
                    }
                }
            }
        }

        if !critical_in_danger {
            return Ok(MissionResult::Running);
        }

        // Try to activate safe mode.
        let controller = match room.controller() {
            Some(c) => c,
            None => {
                warn!("[SafeMode] No controller in room {} -- cannot activate safe mode", room_data.name);
                return Ok(MissionResult::Running);
            }
        };

        // Check if safe mode is already active.
        if controller.safe_mode().unwrap_or(0) > 0 {
            info!("[SafeMode] Safe mode already active in room {}", room_data.name);
            self.activated = true;
            return Ok(MissionResult::Running);
        }

        // Check availability.
        if controller.safe_mode_available() == 0 {
            warn!("[SafeMode] No safe mode charges available in room {}", room_data.name);
            return Ok(MissionResult::Running);
        }

        // Check cooldown.
        if controller.safe_mode_cooldown().unwrap_or(0) > 0 {
            warn!(
                "[SafeMode] Safe mode on cooldown ({} ticks remaining) in room {}",
                controller.safe_mode_cooldown().unwrap_or(0),
                room_data.name
            );
            return Ok(MissionResult::Running);
        }

        // Check if upgrade is blocked (attack_controller was used recently).
        if controller.upgrade_blocked().unwrap_or(0) > 0 {
            warn!(
                "[SafeMode] Controller upgrade blocked ({} ticks remaining) -- cannot activate safe mode in room {}",
                controller.upgrade_blocked().unwrap_or(0),
                room_data.name
            );
            return Ok(MissionResult::Running);
        }

        // All checks passed -- activate safe mode.
        warn!(
            "[SafeMode] ACTIVATING SAFE MODE in room {} (hostile DPS: {:.0}, critical structures in danger)",
            room_data.name, total_hostile_dps
        );

        match controller.activate_safe_mode() {
            Ok(()) => {
                warn!("[SafeMode] Safe mode activated successfully in room {}", room_data.name);
                self.activated = true;
            }
            Err(e) => {
                warn!("[SafeMode] Failed to activate safe mode in room {}: {:?}", room_data.name, e);
            }
        }

        Ok(MissionResult::Running)
    }
}
