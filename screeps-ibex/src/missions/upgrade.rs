use super::constants::*;
use super::data::*;
use super::missionsystem::*;
use crate::jobs::data::*;
use crate::jobs::upgrade::*;
use crate::room::data::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use lerp::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct UpgradeMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    upgraders: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl UpgradeMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = UpgradeMission::new(owner, room_data);

        builder
            .with(MissionData::Upgrade(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> UpgradeMission {
        UpgradeMission {
            owner: owner.into(),
            room_data,
            upgraders: EntityVec::new(),
        }
    }

    pub fn can_run(room_data: &RoomData) -> bool {
        room_data
            .get_structures()
            .map(|s| s.controllers().iter().any(|c| c.my()))
            .unwrap_or(false)
    }

    fn create_handle_upgrader_spawn(
        mission_entity: Entity,
        home_room: Entity,
        allow_harvest: bool,
    ) -> crate::spawnsystem::SpawnQueueCallback {
        Box::new(move |spawn_system_data, name| {
            let name = name.to_string();

            spawn_system_data.updater.exec_mut(move |world| {
                let creep_job = JobData::Upgrade(UpgradeJob::new(home_room, allow_harvest));

                let creep_entity = crate::creep::spawning::build(world.create_entity(), &name).with(creep_job).build();

                if let Some(mut mission_data) = world
                    .write_storage::<MissionData>()
                    .get_mut(mission_entity)
                    .as_mission_type_mut::<UpgradeMission>()
                {
                    mission_data.upgraders.push(creep_entity);
                }
            });
        })
    }

    /// Compute the minimum number of WORK parts needed for an upgrader to
    /// restore the controller's downgrade timer from `current_ttd` back to
    /// the safe threshold (`max_ticks / 2`) within one creep lifetime.
    ///
    /// Assumes the creep has a container of energy adjacent to the controller
    /// so pickup costs only 1 tick per refill. The body format at RCL > 3 is
    /// `[WORK, CARRY, MOVE, MOVE] + (W-1)*[WORK]`, giving 1 CARRY (50 cap),
    /// 2 MOVE, and W total WORK parts.
    ///
    /// Model per refill cycle (W work parts, 1 carry = 50 energy):
    ///   - upgrade_ticks = floor(50 / W)
    ///   - pickup_ticks  = 1 (withdraw from adjacent container)
    ///   - cycle_ticks   = upgrade_ticks + pickup_ticks
    ///   - Each upgrade tick restores CONTROLLER_DOWNGRADE_RESTORE (100) ticks
    ///     but the timer also decays by 1, so net = 99 per upgrade tick.
    ///   - The pickup tick contributes 0 restore but still decays by 1.
    ///   - net_per_cycle = upgrade_ticks * 99 - 1
    ///
    /// Available lifetime ticks = CREEP_LIFE_TIME - spawn_ticks, where
    /// spawn_ticks = body_part_count * CREEP_SPAWN_TIME.
    fn work_parts_for_upkeep(current_ttd: u32, max_ticks: u32) -> usize {
        let safe_threshold = max_ticks / 2;
        if current_ttd >= safe_threshold {
            return 1;
        }
        let deficit = (safe_threshold - current_ttd) as f64;
        let net_restore_per_upgrade_tick = (CONTROLLER_DOWNGRADE_RESTORE as f64) - 1.0;

        // Try increasing WORK parts until the creep can cover the deficit.
        // The body is [WORK, CARRY, MOVE, MOVE] + (w-1)*[WORK], so total
        // parts = w + 3. Max body size is 50 parts, so w <= 47.
        for w in 1..=CONTROLLER_MAX_UPGRADE_PER_TICK {
            let body_parts = w + 3;
            let spawn_ticks = body_parts * CREEP_SPAWN_TIME;
            let lifetime = CREEP_LIFE_TIME.saturating_sub(spawn_ticks) as f64;

            let carry_cap = CARRY_CAPACITY as f64; // 50
            let upgrade_ticks_per_cycle = (carry_cap / w as f64).floor();
            if upgrade_ticks_per_cycle < 1.0 {
                continue;
            }
            let cycle_ticks = upgrade_ticks_per_cycle + 1.0; // +1 for pickup
            let net_per_cycle = upgrade_ticks_per_cycle * net_restore_per_upgrade_tick - 1.0;
            if net_per_cycle <= 0.0 {
                continue;
            }

            let cycles = (lifetime / cycle_ticks).floor();
            let total_restore = cycles * net_per_cycle;

            if total_restore >= deficit {
                return w as usize;
            }
        }

        // Fallback: cap at the max-level upgrade limit.
        CONTROLLER_MAX_UPGRADE_PER_TICK as usize
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for UpgradeMission {
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

    fn remove_creep(&mut self, entity: Entity) {
        self.upgraders.retain(|e| *e != entity);
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!("Upgrade - Upgraders: {}", self.upgraders.len())
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Upgrade - Upgraders: {}", self.upgraders.len()))
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        //TODO: Limit upgraders to CONTROLLER_MAX_UPGRADE_PER_TICK total work parts at max level.

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;
        let structures = room_data.get_structures().ok_or("Expected structure data")?;
        let creeps = room_data.get_creeps().ok_or("Expected creeps")?;
        let static_visibility_data = room_data.get_static_visibility_data().ok_or("Expected static visibility data")?;

        let controllers = structures.controllers();
        let storages = structures.storages();

        if !Self::can_run(room_data) {
            return Err("Upgrade room not owned by user".to_string());
        }

        let controller_level = controllers.iter().map(|c| c.level()).max().ok_or("Expected controller level")?;

        let has_excess_energy = {
            if !structures.storages().is_empty() {
                let energy: u32 = structures
                    .storages()
                    .iter()
                    .map(|storage| storage.store().get(ResourceType::Energy).unwrap_or(0))
                    .sum();

                energy >= get_desired_storage_amount(ResourceType::Energy) / 2
            } else if !structures.containers().is_empty() {
                structures
                    .containers()
                    .iter()
                    .any(|container| container.store().get(ResourceType::Energy).unwrap_or(0) as f32 / CONTAINER_CAPACITY as f32 > 0.75)
            } else {
                true
            }
        };

        let are_hostile_creeps = !creeps.hostile().is_empty();

        // Detect downgrade risk at any RCL. When the downgrade timer falls
        // below half of max we spawn an upkeep upgrader at critical priority,
        // sized so that it can restore the timer back to the safe threshold
        // within a single creep lifetime (assuming a container of energy is
        // adjacent to the controller).
        let downgrade_upkeep_parts: Option<usize> = controllers
            .iter()
            .filter_map(|controller| {
                let max_ticks = controller_downgrade(controller.level())?;
                let ttd = controller.ticks_to_downgrade()?;
                if ttd < max_ticks / 2 {
                    Some(Self::work_parts_for_upkeep(ttd, max_ticks))
                } else {
                    None
                }
            })
            .max();

        let downgrade_risk = downgrade_upkeep_parts.is_some();

        let at_max_level = controller_levels(controller_level as u32).is_none();

        //TODO: Need better calculation for maximum number of upgraders.
        let max_upgraders = if can_execute_cpu(CpuBar::MediumPriority) {
            if are_hostile_creeps || at_max_level {
                1
            } else if has_excess_energy {
                if controller_level <= 3 {
                    5
                } else {
                    3
                }
            } else {
                1
            }
        } else {
            1
        };

        let alive_upgraders = self
            .upgraders
            .iter()
            .filter(|entity| {
                system_data.creep_spawning.get(**entity).is_some()
                    || system_data
                        .creep_owner
                        .get(**entity)
                        .and_then(|creep_owner| creep_owner.owner.resolve())
                        .and_then(|creep| creep.ticks_to_live())
                        .map(|count| count > 100)
                        .unwrap_or(false)
            })
            .count();

        if alive_upgraders < max_upgraders {
            let work_parts_per_upgrader = if let Some(upkeep_parts) = downgrade_upkeep_parts {
                if self.upgraders.is_empty() {
                    // Downgrade risk with no upgrader at all: size the body
                    // to restore the timer in one lifetime.
                    Some(upkeep_parts)
                } else {
                    // Have an upgrader (possibly dying) — use the normal
                    // max-level cap for the replacement.
                    let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);
                    let work_parts = (work_parts_per_tick / (max_upgraders as f32)).ceil();
                    Some(work_parts as usize)
                }
            } else if at_max_level {
                // At max controller level the game caps upgrade throughput to
                // CONTROLLER_MAX_UPGRADE_PER_TICK energy per tick.
                let work_parts_per_tick = (CONTROLLER_MAX_UPGRADE_PER_TICK as f32) / (UPGRADE_CONTROLLER_POWER as f32);

                let work_parts = (work_parts_per_tick / (max_upgraders as f32)).ceil();

                Some(work_parts as usize)
            } else if has_excess_energy {
                Some(20)
            } else {
                let sources = static_visibility_data.sources();

                let energy_per_second = ((SOURCE_ENERGY_CAPACITY * sources.len() as u32) as f32) / (ENERGY_REGEN_TIME as f32);
                let upgrade_per_second = energy_per_second / (UPGRADE_CONTROLLER_POWER as f32);

                let parts_per_upgrader = ((upgrade_per_second / 2.0) / max_upgraders as f32).floor().max(1.0) as usize;

                Some(parts_per_upgrader)
            };

            let maximum_energy = if self.upgraders.is_empty() && downgrade_risk {
                room.energy_available().max(SPAWN_ENERGY_CAPACITY)
            } else {
                room.energy_capacity_available()
            };

            let body_definition = if controller_level <= 3 {
                crate::creep::SpawnBodyDefinition {
                    maximum_energy,
                    minimum_repeat: Some(0),
                    maximum_repeat: work_parts_per_upgrader,
                    pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
                    repeat_body: &[Part::Work, Part::Move],
                    post_body: &[],
                }
            } else {
                crate::creep::SpawnBodyDefinition {
                    maximum_energy,
                    minimum_repeat: Some(1),
                    maximum_repeat: work_parts_per_upgrader.map(|p| p - 1),
                    pre_body: &[Part::Work, Part::Carry, Part::Move, Part::Move],
                    repeat_body: &[Part::Work],
                    post_body: &[],
                }
            };

            if let Ok(body) = crate::creep::spawning::create_body(&body_definition) {
                let priority = if downgrade_risk && self.upgraders.is_empty() {
                    // Downgrade risk with no upgrader at all — override
                    // everything else and get a creep out immediately.
                    SPAWN_PRIORITY_CRITICAL
                } else if self.upgraders.is_empty() {
                    SPAWN_PRIORITY_HIGH
                } else if has_excess_energy && !storages.is_empty() && max_upgraders > 1 {
                    let interp = (alive_upgraders as f32) / ((max_upgraders - 1) as f32);

                    SPAWN_PRIORITY_HIGH.lerp_bounded(SPAWN_PRIORITY_MEDIUM, interp)
                } else if max_upgraders > 1 {
                    let interp = (alive_upgraders as f32) / ((max_upgraders - 1) as f32);

                    SPAWN_PRIORITY_MEDIUM.lerp_bounded(SPAWN_PRIORITY_LOW, interp)
                } else {
                    SPAWN_PRIORITY_MEDIUM
                };

                let allow_harvest = controller_level <= 3;

                let spawn_request = SpawnRequest::new(
                    "Upgrader".to_string(),
                    &body,
                    priority,
                    None,
                    Self::create_handle_upgrader_spawn(mission_entity, self.room_data, allow_harvest),
                );

                system_data.spawn_queue.request(self.room_data, spawn_request);
            }
        }

        Ok(MissionResult::Running)
    }
}
