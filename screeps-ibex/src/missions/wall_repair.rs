use super::data::*;
use super::missionsystem::*;
use crate::jobs::utility::repair::RepairPriority;
use crate::remoteobjectid::*;
use crate::repairqueue::RepairRequest;
use crate::serialize::*;
use crate::structureidentifier::RemoteStructureIdentifier;
use crate::transfer::transfersystem::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Minimum wall/rampart hits to consider "safe" during a siege.
/// Structures below this threshold get priority repair.
const EMERGENCY_WALL_HITS: u32 = 100_000;

/// Moderate wall/rampart hits threshold. Structures below this get medium priority.
const MODERATE_WALL_HITS: u32 = 1_000_000;

/// Ticks with no hostiles before the mission completes (avoids restart flip-flop).
const IDLE_TICKS_BEFORE_COMPLETE: u32 = 100;

/// Mission to prioritize wall and rampart repair during siege.
///
/// When hostiles are present and attacking walls/ramparts, this mission
/// ensures towers focus on repair and that the transfer system prioritizes
/// energy delivery to towers. It also tracks the weakest wall/rampart
/// sections and logs warnings.
///
/// Uses an idle/active state: when hostiles leave, the mission stays running
/// (idle) for IDLE_TICKS_BEFORE_COMPLETE before completing, so we don't
/// restart the mission every other tick if hostiles flicker.
#[derive(ConvertSaveload)]
pub struct WallRepairMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    /// Tick when we last ran the wall scan.
    last_scan_tick: u32,
    /// Ticks since we last saw hostiles. When >= IDLE_TICKS_BEFORE_COMPLETE we complete.
    ticks_since_hostiles: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl WallRepairMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = WallRepairMission {
            owner: owner.into(),
            room_data,
            last_scan_tick: 0,
            ticks_since_hostiles: 0,
        };

        builder
            .with(MissionData::WallRepair(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for WallRepairMission {
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
        "WallRepair".to_string()
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text("WallRepair".to_string())
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let room_data_entity = self.room_data;

        // Register a high-priority transfer generator for tower energy during siege.
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

                // Only boost tower energy priority if hostiles are present.
                let hostiles = creeps.hostile();
                if hostiles.is_empty() {
                    return Ok(());
                }

                // Check if any wall/rampart is under emergency threshold.
                let has_emergency = structures.ramparts().iter().any(|r| r.my() && r.hits() < EMERGENCY_WALL_HITS)
                    || structures.walls().iter().any(|w| w.hits() < EMERGENCY_WALL_HITS);

                if !has_emergency {
                    return Ok(());
                }

                // Request high-priority energy for all towers that aren't full.
                for tower in structures.towers() {
                    if !tower.my() {
                        continue;
                    }
                    let free_cap = tower.store().get_free_capacity(Some(ResourceType::Energy));
                    if free_cap > 0 {
                        let request = TransferDepositRequest::new(
                            TransferTarget::Tower(tower.remote_id()),
                            Some(ResourceType::Energy),
                            TransferPriority::High,
                            free_cap as u32,
                            TransferType::Haul,
                        );
                        transfer.request_deposit(request);
                    }
                }

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let current_tick = game::time();

        // Scan every 20 ticks to save CPU.
        if current_tick.saturating_sub(self.last_scan_tick) < 20 {
            return Ok(MissionResult::Running);
        }
        let elapsed = current_tick.saturating_sub(self.last_scan_tick);
        self.last_scan_tick = current_tick;

        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;

        let structures = match room_data.get_structures() {
            Some(s) => s,
            None => return Ok(MissionResult::Running),
        };

        let has_hostiles = room_data
            .get_creeps()
            .map(|c| !c.hostile().is_empty())
            .unwrap_or(false);

        if has_hostiles {
            self.ticks_since_hostiles = 0;
        } else {
            self.ticks_since_hostiles = self.ticks_since_hostiles.saturating_add(elapsed);
            if self.ticks_since_hostiles >= IDLE_TICKS_BEFORE_COMPLETE {
                return Ok(MissionResult::Success);
            }
        }

        let room_name = room_data.name;

        // Analyze wall and rampart health and populate the repair queue.
        let mut weakest_rampart_hits: u32 = u32::MAX;
        let mut weakest_wall_hits: u32 = u32::MAX;
        let mut emergency_count: u32 = 0;
        let mut moderate_count: u32 = 0;

        for rampart in structures.ramparts() {
            if !rampart.my() {
                continue;
            }
            let hits = rampart.hits();
            let hits_max = rampart.hits_max();
            weakest_rampart_hits = weakest_rampart_hits.min(hits);

            let priority = if hits < EMERGENCY_WALL_HITS {
                emergency_count += 1;
                RepairPriority::Critical
            } else if hits < MODERATE_WALL_HITS {
                moderate_count += 1;
                RepairPriority::High
            } else if hits < hits_max {
                RepairPriority::Medium
            } else {
                continue;
            };

            system_data.repair_queue.request_repair(RepairRequest {
                structure_id: RemoteStructureIdentifier::new(&StructureObject::from(rampart.clone())),
                priority,
                current_hits: hits,
                max_hits: hits_max,
                room: room_name,
            });
        }

        for wall in structures.walls() {
            let hits = wall.hits();
            let hits_max = wall.hits_max();
            weakest_wall_hits = weakest_wall_hits.min(hits);

            let priority = if hits < EMERGENCY_WALL_HITS {
                emergency_count += 1;
                RepairPriority::Critical
            } else if hits < MODERATE_WALL_HITS {
                moderate_count += 1;
                RepairPriority::High
            } else if hits < hits_max {
                RepairPriority::Medium
            } else {
                continue;
            };

            system_data.repair_queue.request_repair(RepairRequest {
                structure_id: RemoteStructureIdentifier::new(&StructureObject::from(wall.clone())),
                priority,
                current_hits: hits,
                max_hits: hits_max,
                room: room_name,
            });
        }

        if emergency_count > 0 {
            let has_hostiles = room_data.get_creeps().map(|c| !c.hostile().is_empty()).unwrap_or(false);

            if has_hostiles {
                warn!(
                    "[WallRepair] Room {} has {} structures below emergency threshold ({} hits)",
                    room_name, emergency_count, EMERGENCY_WALL_HITS
                );
            } else {
                debug!(
                    "[WallRepair] Room {} has {} structures below emergency threshold ({} hits) - no hostiles, likely building up",
                    room_name, emergency_count, EMERGENCY_WALL_HITS
                );
            }
        }

        if moderate_count > 0 {
            debug!(
                "[WallRepair] Room {} has {} structures below moderate threshold ({} hits)",
                room_name, moderate_count, MODERATE_WALL_HITS
            );
        }

        // Log weakest points.
        if weakest_rampart_hits < u32::MAX {
            debug!("[WallRepair] Room {} weakest rampart: {} hits", room_name, weakest_rampart_hits);
        }

        if weakest_wall_hits < u32::MAX {
            debug!("[WallRepair] Room {} weakest wall: {} hits", room_name, weakest_wall_hits);
        }

        Ok(MissionResult::Running)
    }
}
