use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
/// Tracks a hostile creep suspected of tower draining.
#[derive(Clone, Debug, Default, ConvertSaveload)]
pub struct DrainTracker {
    /// How many times this creep has been seen entering the room.
    enter_count: u32,
    /// How many times this creep has left the room (presumably to heal).
    exit_count: u32,
    /// Last tick the creep was seen in the room.
    last_seen_tick: u32,
    /// Whether the creep was in the room last tick.
    was_present: bool,
}

#[derive(ConvertSaveload)]
pub struct TowerMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    /// Per-creep drain tracking data, keyed by hostile creep name.
    /// Persisted across ticks so drain detection survives VM reloads.
    drain_trackers: EntityHashMap<String, DrainTracker>,
    /// Last tick when stale drain trackers were cleaned up.
    last_drain_cleanup: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TowerMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TowerMission::new(owner, room_data);

        builder
            .with(MissionData::Tower(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> TowerMission {
        TowerMission {
            owner: owner.into(),
            room_data,
            drain_trackers: EntityHashMap::new(),
            last_drain_cleanup: 0,
        }
    }

    /// Get the set of creep names confirmed as tower drainers.
    /// A creep is confirmed if it has entered the room at least twice
    /// and exited at least once (the enter/exit cycling pattern).
    fn get_confirmed_drainers(&self) -> std::collections::HashSet<String> {
        self.drain_trackers
            .iter()
            .filter(|(_, tracker)| tracker.enter_count >= 2 && tracker.exit_count >= 1)
            .map(|(name, _)| name.clone())
            .collect()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for TowerMission {
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
        "Tower".to_string()
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text("Tower".to_string())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let room_data_entity = self.room_data;

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |system, transfer, _room_name| {
                let room_data = system.get_room_data(room_data_entity).ok_or("Expected room data")?;
                let structures = room_data.get_structures().ok_or_else(|| {
                    let msg = format!("Expected structures - Room: {}", room_data.name);
                    log::warn!("{} at {}:{}", msg, file!(), line!());
                    msg
                })?;
                let creeps = room_data.get_creeps().ok_or_else(|| {
                    let msg = format!("Expected creeps - Room: {}", room_data.name);
                    log::warn!("{} at {}:{}", msg, file!(), line!());
                    msg
                })?;

                let towers = structures.towers();

                let hostile_creeps = creeps.hostile();
                let are_hostile_creeps = !hostile_creeps.is_empty();

                let priority = if are_hostile_creeps {
                    TransferPriority::High
                } else {
                    TransferPriority::Low
                };

                for tower in towers {
                    let tower_free_capacity = tower.store().get_free_capacity(Some(ResourceType::Energy));
                    if tower_free_capacity > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Tower(tower.remote_id()),
                            Some(ResourceType::Energy),
                            priority,
                            tower_free_capacity as u32,
                            TransferType::Haul,
                        );

                        transfer.request_deposit(transfer_request);
                    }
                }

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let structures = room_data.get_structures().ok_or_else(|| {
            let msg = format!("Expected structures - Room: {}", room_data.name);
            log::warn!("{} at {}:{}", msg, file!(), line!());
            msg
        })?;
        let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;
        let creeps = room_data.get_creeps().ok_or_else(|| {
            let msg = format!("Expected creeps - Room: {}", room_data.name);
            log::warn!("{} at {}:{}", msg, file!(), line!());
            msg
        })?;

        let towers = structures.towers();
        let my_towers: Vec<_> = towers.iter().filter(|t| t.my()).collect();

        if my_towers.is_empty() {
            return Ok(MissionResult::Running);
        }

        // Collect our tower positions for damage calculations.
        let tower_positions: Vec<Position> = my_towers.iter().map(|t| t.pos()).collect();

        // Analyze hostile creeps for coordinated targeting.
        let hostile_creeps = creeps.hostile();

        // ─── Update drain trackers ──────────────────────────────────────────
        // Track which hostile creeps are currently present.
        let current_tick = game::time();
        let mut currently_present: std::collections::HashSet<String> = std::collections::HashSet::new();

        for hostile in hostile_creeps.iter() {
            currently_present.insert(hostile.name());
        }

        for name in &currently_present {
            let tracker = self.drain_trackers.entry(name.clone()).or_default();

            // If the creep was not present last tick but is now, it re-entered.
            if !tracker.was_present {
                tracker.enter_count += 1;
            }
            tracker.last_seen_tick = current_tick;
            tracker.was_present = true;
        }

        // Mark creeps that left the room.
        for (name, tracker) in self.drain_trackers.iter_mut() {
            if !currently_present.contains(name) && tracker.was_present {
                tracker.exit_count += 1;
                tracker.was_present = false;
            }
        }

        // Periodic cleanup of stale trackers (every 100 ticks).
        if current_tick - self.last_drain_cleanup > 100 {
            self.drain_trackers.retain(|_, tracker| current_tick - tracker.last_seen_tick < 500);
            self.last_drain_cleanup = current_tick;
        }

        // Identify confirmed drain creeps.
        let confirmed_drainers = self.get_confirmed_drainers();

        if !confirmed_drainers.is_empty() {
            info!(
                "[Tower] Confirmed tower drain creeps in {}: {:?}",
                room_data.name, confirmed_drainers
            );
        }

        if !hostile_creeps.is_empty() {
            // Calculate per-hostile heal rate for net damage assessment.
            let hostile_infos: Vec<_> = hostile_creeps
                .iter()
                .map(|c| {
                    let heal_per_tick: f32 = c
                        .body()
                        .iter()
                        .filter(|p| p.hits() > 0 && p.part() == Part::Heal)
                        .map(|p| if p.boost().is_some() { 48.0 } else { 12.0 })
                        .sum();
                    let is_confirmed_drainer = confirmed_drainers.contains(&c.name());
                    (c, heal_per_tick, is_confirmed_drainer)
                })
                .collect();

            // Find the best target: prefer targets where we can do net positive damage.
            // Skip confirmed drainers -- they're wasting our energy on purpose.
            let best_target = hostile_infos
                .iter()
                .filter(|(_, _, is_drainer)| !is_drainer)
                .filter(|(c, heal, _)| {
                    // Only fire if we can do net damage (overcome healing).
                    let total_damage = crate::military::damage::total_tower_damage(&tower_positions, c.pos());
                    total_damage > *heal
                })
                .min_by(|(a, _, _), (b, _, _)| {
                    // Prefer dangerous creeps first.
                    let a_dangerous = a
                        .body()
                        .iter()
                        .any(|p| matches!(p.part(), Part::Attack | Part::RangedAttack | Part::Work));
                    let b_dangerous = b
                        .body()
                        .iter()
                        .any(|p| matches!(p.part(), Part::Attack | Part::RangedAttack | Part::Work));

                    match (a_dangerous, b_dangerous) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a.hits().cmp(&b.hits()),
                    }
                })
                .map(|(c, _, _)| *c);

            // Detect tower drain: hostile at room edge that can heal through all tower damage,
            // OR confirmed drainer based on enter/exit tracking.
            let is_drain = best_target.is_none()
                && hostile_infos.iter().any(|(c, heal, is_drainer)| {
                    *is_drainer || crate::military::damage::is_likely_tower_drain(c.pos(), *heal, &tower_positions)
                });

            if is_drain {
                // Tower drain detected: conserve energy.
                // Only fire at confirmed drainers if they come within max-damage range (range 5).
                // Fire at non-drainers normally.
                let non_drainer_target = hostile_infos
                    .iter()
                    .filter(|(_, _, is_drainer)| !is_drainer)
                    .min_by_key(|(c, _, _)| c.hits())
                    .map(|(c, _, _)| *c);

                let close_drainer = hostile_infos
                    .iter()
                    .filter(|(_, _, is_drainer)| *is_drainer)
                    .filter(|(c, _, _)| tower_positions.iter().any(|tp| tp.get_range_to(c.pos()) <= 5))
                    .min_by_key(|(c, _, _)| c.hits())
                    .map(|(c, _, _)| *c);

                let target = non_drainer_target.or(close_drainer);

                if let Some(target) = target {
                    for tower in &my_towers {
                        let _ = tower.attack(target);
                    }
                }
                // Otherwise, don't fire -- save energy against drainers.
            } else if let Some(target) = best_target {
                // Coordinated fire: all towers focus the same target.
                for tower in &my_towers {
                    let _ = tower.attack(target);
                }
            } else {
                // No target where we can do net damage. Check for any hostile we should still shoot.
                // Fall back to weakest non-drainer hostile.
                let weakest = hostile_creeps
                    .iter()
                    .filter(|c| !confirmed_drainers.contains(&c.name()))
                    .min_by_key(|c| c.hits());
                if let Some(target) = weakest {
                    for tower in &my_towers {
                        let _ = tower.attack(target);
                    }
                }
            }

            return Ok(MissionResult::Running);
        }

        // No hostiles -- heal friendly creeps or repair structures.
        let weakest_friendly_creep = creeps
            .friendly()
            .iter()
            .filter(|creep| creep.hits() < creep.hits_max())
            .min_by_key(|creep| creep.hits());

        let minimum_repair_priority = if dynamic_visibility_data.hostile_creeps() {
            Some(RepairPriority::Medium)
        } else {
            Some(RepairPriority::Low)
        };

        let repair_structure =
            select_repair_structure(room_data, system_data.repair_queue, minimum_repair_priority, false).and_then(|id| id.resolve());

        for tower in &my_towers {
            if let Some(creep) = weakest_friendly_creep {
                let _ = tower.heal(creep);
                continue;
            }

            if let Some(structure) = repair_structure.as_ref() {
                if let Some(repairable) = structure.as_repairable() {
                    let _ = tower.repair(repairable);
                }
                continue;
            }
        }

        Ok(MissionResult::Running)
    }
}
