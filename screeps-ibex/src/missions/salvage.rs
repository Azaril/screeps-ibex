use super::data::*;
use super::dismantle::*;
use super::missionsystem::*;
use super::raid::*;
use super::utility::*;
use crate::jobs::utility::dismantle::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use log::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Per-room salvage coordinator: strips a militarily dead room of its
/// resources by sequencing the existing [`RaidMission`] (loot stores) and
/// [`DismantleMission`] (wreck structures, recover energy) children.
///
/// Created exclusively by `SalvageOperation` after the EV/strategic gate.
/// The mission owns the in-room lifecycle: it keeps intel fresh, aborts
/// loudly if the room re-arms or enters safe mode, and completes once
/// nothing lootable or dismantlable (within the hit-pool horizon) remains.
/// Controller takeover is deliberately NOT this mission's job — once the
/// owner's controller decays to neutral, the mining-outpost pipeline takes
/// the room over through its normal candidate flow.
#[derive(ConvertSaveload)]
pub struct SalvageMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    raid_mission: EntityOption<Entity>,
    dismantle_mission: EntityOption<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl SalvageMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = SalvageMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::Salvage(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> SalvageMission {
        SalvageMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            raid_mission: None.into(),
            dismantle_mission: None.into(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for SalvageMission {
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

    fn get_children(&self) -> Vec<Entity> {
        [*self.raid_mission, *self.dismantle_mission].iter().filter_map(|e| *e).collect()
    }

    fn child_complete(&mut self, child: Entity) {
        if self.raid_mission.map(|e| e == child).unwrap_or(false) {
            self.raid_mission.take();
        }

        if self.dismantle_mission.map(|e| e == child).unwrap_or(false) {
            self.dismantle_mission.take();
        }
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        let mut parts = Vec::new();
        if self.raid_mission.is_some() {
            parts.push("raid");
        }
        if self.dismantle_mission.is_some() {
            parts.push("dismantle");
        }
        if parts.is_empty() {
            "Salvage".to_string()
        } else {
            format!("Salvage - {}", parts.join(", "))
        }
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!(
            "Salvage - raid: {} dismantle: {}",
            self.raid_mission.is_some(),
            self.dismantle_mission.is_some()
        ))
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        self.home_room_datas
            .retain(|entity| system_data.room_data.get(*entity).map(is_valid_home_room).unwrap_or(false));

        if self.home_room_datas.is_empty() {
            return Err("No home rooms for salvage mission".to_owned());
        }

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        let derelict_features = system_data.features.derelict;

        // Phase 1: gates and work detection against an immutable room borrow.
        let (room_name, needs_raiding, needs_dismantling) = {
            let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
            let dynamic_visibility_data = room_data.get_dynamic_visibility_data().ok_or("Expected dynamic visibility data")?;

            if !derelict_features.on {
                return Err("Derelict-room handling disabled - aborting salvage".to_string());
            }

            if dynamic_visibility_data.updated_within(1000) {
                if dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly() {
                    // Claimed - colony/outpost machinery owns the room now.
                    return Ok(MissionResult::Success);
                }

                if dynamic_visibility_data.militarily_active() {
                    return Err("Salvage target re-armed (spawn/tower/combat creeps) - aborting".to_string());
                }
            } else if !dynamic_visibility_data.updated_within(derelict_features.action_max_age) {
                return Err("Salvage intel too stale - aborting".to_string());
            }

            if dynamic_visibility_data.safe_mode_active() {
                // Safe mode blocks withdraw/dismantle for us; abort and let the
                // operation re-admit once it has expired.
                return Err("Salvage target under safe mode - aborting".to_string());
            }

            // Work detection needs live structure data; keep eyes on the room.
            let work = room_data.get_structures().map(|structures| {
                let sources = room_data
                    .get_static_visibility_data()
                    .map(|s| s.sources().as_slice())
                    .unwrap_or(&[]);

                let needs_raiding = structures.all().iter().any(|s| is_salvage_loot_target(s, sources));
                let needs_dismantling =
                    DismantleMission::requires_dismantling(structures.all(), sources, derelict_features.max_structure_hits);

                (needs_raiding, needs_dismantling)
            });

            match work {
                Some((needs_raiding, needs_dismantling)) => (room_data.name, needs_raiding, needs_dismantling),
                None => {
                    system_data.visibility.request(VisibilityRequest::new(
                        room_data.name,
                        VISIBILITY_PRIORITY_MEDIUM,
                        VisibilityRequestFlags::ALL,
                    ));

                    return Ok(MissionResult::Running);
                }
            }
        };

        // Phase 2: child mission upkeep (needs mutable room access).
        if let Some(mut raid_mission) = self
            .raid_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<RaidMission>()
        {
            raid_mission.set_home_rooms(&self.home_room_datas);
        } else if self.raid_mission.is_none() && needs_raiding {
            info!("Starting raid for salvage of room {}", room_name);

            let raid_entity = RaidMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                self.room_data,
                &self.home_room_datas,
            )
            .build();

            let room_data = system_data.room_data.get_mut(self.room_data).ok_or("Expected room data")?;
            room_data.add_mission(raid_entity);

            self.raid_mission = Some(raid_entity).into();
        }

        if let Some(mut dismantle_mission) = self
            .dismantle_mission
            .and_then(|e| system_data.missions.get(e))
            .as_mission_type_mut::<DismantleMission>()
        {
            dismantle_mission.set_home_rooms(&self.home_room_datas);
        } else if self.dismantle_mission.is_none() && needs_dismantling {
            info!("Starting dismantle for salvage of room {}", room_name);

            let dismantle_entity = DismantleMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                self.room_data,
                &self.home_room_datas,
                false,
            )
            .build();

            let room_data = system_data.room_data.get_mut(self.room_data).ok_or("Expected room data")?;
            room_data.add_mission(dismantle_entity);

            self.dismantle_mission = Some(dismantle_entity).into();
        }

        if !needs_raiding && !needs_dismantling && self.raid_mission.is_none() && self.dismantle_mission.is_none() {
            info!("Salvage of room {} complete - nothing lootable or dismantlable remains", room_name);

            return Ok(MissionResult::Success);
        }

        Ok(MissionResult::Running)
    }
}
