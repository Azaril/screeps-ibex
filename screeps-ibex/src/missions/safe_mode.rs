use super::data::*;
use super::missionsystem::*;
use super::utility::*;
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

/// Cooldown between safe mode evaluations (ticks).
const EVAL_INTERVAL: u32 = 5;

/// A critical structure counts as "about to be destroyed" below this fraction
/// of its max hits (2/5 ⇒ 2000 for a 5000-hit spawn). The old absolute floor
/// (`CRITICAL_STRUCTURE_MIN_HITS = 5000`) EQUALLED spawn max hits, so any
/// scratch armed the trigger and a single poke could spend the scarce,
/// irreversible safe-mode charge (combat review 2026-07-09 D2).
fn critical_floor(hits_max: u32) -> u32 {
    (hits_max * 2) / 5
}

/// Pure arming decision (host-testable; the D2 pins live on this).
///
/// Safe mode only blocks hostile ACTIONS, so the low-HP arm additionally
/// requires the hostiles to have actual damage output — a damaged spawn plus a
/// harmless scout must never spend the charge. The dismantler arm keeps its
/// original shape: burst DPS above [`SAFE_MODE_DPS_THRESHOLD`] with a
/// WORK-carrier adjacent to a spawn is an emergency at any HP.
fn safe_mode_should_arm(spawn_hits: &[(u32, u32)], hostile_dps: f32, dismantler_adjacent_to_spawn: bool) -> bool {
    let critical_low = spawn_hits.iter().any(|&(hits, max)| hits < critical_floor(max));
    (critical_low && hostile_dps > 0.0) || (dismantler_adjacent_to_spawn && hostile_dps > SAFE_MODE_DPS_THRESHOLD)
}

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

    fn get_room(&self) -> Option<Entity> {
        Some(self.room_data)
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

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        // Ownership-subordinate (ADR 0017 §13): this mission dies with the room. Gated before the
        // feature/activated/interval early-outs — each of those returns Running unconditionally, so
        // a mission on a lost room would otherwise zombie forever.
        {
            let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
            if !is_valid_home_room(room_data) {
                return Err(format!("Safe mode room {} is no longer an owned home room", room_data.name));
            }
        }

        let features = system_data.features;

        if !features.military.safe_mode {
            return Ok(MissionResult::Running);
        }

        let current_tick = game::time();
        if current_tick.saturating_sub(self.last_eval_tick) < EVAL_INTERVAL {
            return Ok(MissionResult::Running);
        }
        self.last_eval_tick = current_tick;

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let room = match game::rooms().get(room_data.name) {
            Some(r) => r,
            None => return Ok(MissionResult::Running),
        };

        // While a safe mode WE latched is still running, just monitor — but the latch
        // clears the moment it expires, so the room can safe-mode again in a later
        // emergency. The old latch was permanent: a room could auto-safe-mode at most
        // once EVER (combat review 2026-07-09 D3). The serialized field is kept (its
        // removal would be a WFV shape change); only its lifetime changed.
        if self.activated {
            let still_active = room.controller().map(|c| c.safe_mode().unwrap_or(0) > 0).unwrap_or(true);
            if still_active {
                return Ok(MissionResult::Running);
            }
            info!("[SafeMode] Safe mode expired in room {} -- re-arming the evaluator", room_data.name);
            self.activated = false;
        }

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

        // Gather the arming-decision inputs (the decision itself is the pure
        // `safe_mode_should_arm` kernel above).
        let spawn_hits: Vec<(u32, u32)> = structures.spawns().iter().map(|s| (s.hits(), s.hits_max())).collect();
        for &(hits, max) in &spawn_hits {
            if hits < critical_floor(max) {
                warn!("[SafeMode] Spawn at critical HP: {}/{} (floor {})", hits, max, critical_floor(max));
            }
        }

        let mut dismantler_adjacent_to_spawn = false;
        if has_work_parts {
            for hostile in hostiles {
                let has_work = hostile.body().iter().any(|p| p.part() == Part::Work && p.hits() > 0);
                if !has_work {
                    continue;
                }

                for spawn in structures.spawns() {
                    if hostile.pos().get_range_to(spawn.pos()) <= 1 {
                        warn!("[SafeMode] Dismantler adjacent to spawn '{}'!", spawn.name());
                        dismantler_adjacent_to_spawn = true;
                    }
                }
            }
        }

        if !safe_mode_should_arm(&spawn_hits, total_hostile_dps, dismantler_adjacent_to_spawn) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// D2 pin: spawn max hits is 5000 and the OLD floor was exactly 5000, so any
    /// scratch (4999/5000) armed the trigger and could spend the irreversible
    /// safe-mode charge. A scratch must never arm, at any hostile DPS.
    #[test]
    fn scratched_spawn_never_arms() {
        assert!(!safe_mode_should_arm(&[(4999, 5000)], 300.0, false));
        assert!(!safe_mode_should_arm(&[(4999, 5000)], 10_000.0, false));
        // Healthy spawn, plain hostiles: nothing to do.
        assert!(!safe_mode_should_arm(&[(5000, 5000)], 500.0, false));
    }

    /// D2 pin: the low-HP arm requires actual hostile damage output — a damaged
    /// spawn plus a harmless scout (0 DPS) must not spend the charge; the same
    /// damage with any real DPS must.
    #[test]
    fn deep_damage_arms_only_with_damage_output() {
        assert!(safe_mode_should_arm(&[(1999, 5000)], 10.0, false));
        assert!(!safe_mode_should_arm(&[(1999, 5000)], 0.0, false));
    }

    /// The dismantler arm keeps its original shape: WORK burst above the DPS
    /// threshold with a carrier adjacent to a spawn is an emergency at ANY HP;
    /// below the threshold it is not.
    #[test]
    fn dismantler_burst_arms_at_full_hp() {
        assert!(safe_mode_should_arm(&[(5000, 5000)], 350.0, true));
        assert!(!safe_mode_should_arm(&[(5000, 5000)], 250.0, true));
    }

    /// The floor is 2/5 of max hits (2000 for a 5000-hit spawn) — strictly below
    /// max, so the D2 class (floor == max) cannot silently return.
    #[test]
    fn critical_floor_is_a_real_fraction_of_max() {
        assert_eq!(critical_floor(5000), 2000);
        assert!(critical_floor(5000) < 5000);
        assert!(!safe_mode_should_arm(&[(2000, 5000)], 100.0, false), "at the floor: not below it");
        assert!(safe_mode_should_arm(&[(1999, 5000)], 100.0, false), "one below the floor arms");
    }
}
