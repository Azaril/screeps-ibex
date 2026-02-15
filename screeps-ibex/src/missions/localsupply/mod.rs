pub mod body_helpers;
pub mod mineral_mining;
pub mod room_transfer;
pub mod source_mining;
pub mod structure_data;

use self::mineral_mining::*;
use self::room_transfer::*;
use self::source_mining::*;
use self::structure_data::*;
use super::data::*;
use super::missionsystem::*;
use crate::remoteobjectid::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use screeps::*;
use screeps_cache::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

/// Thin coordinator that creates and manages child missions for a room's
/// local supply: one `SourceMiningMission` per source, one
/// `MineralMiningMission` per mineral/extractor pair, and one
/// `RoomTransferMission` for transfer queue registration and link transfers.
pub struct LocalSupplyMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
    home_room_datas: EntityVec<Entity>,
    source_mining_missions: EntityVec<Entity>,
    mineral_mining_missions: EntityVec<Entity>,
    transfer_mission: EntityOption<Entity>,
    allow_spawning: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(bound = "MA: Marker")]
pub struct LocalSupplyMissionSaveloadData<MA>
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    owner: <EntityOption<Entity> as ConvertSaveload<MA>>::Data,
    room_data: <Entity as ConvertSaveload<MA>>::Data,
    home_room_datas: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    source_mining_missions: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    mineral_mining_missions: <EntityVec<Entity> as ConvertSaveload<MA>>::Data,
    transfer_mission: <EntityOption<Entity> as ConvertSaveload<MA>>::Data,
    allow_spawning: <bool as ConvertSaveload<MA>>::Data,
}

impl<MA> ConvertSaveload<MA> for LocalSupplyMission
where
    MA: Marker + Serialize,
    for<'deser> MA: Deserialize<'deser>,
{
    type Data = LocalSupplyMissionSaveloadData<MA>;
    #[allow(deprecated)]
    type Error = NoError;

    fn convert_into<F>(&self, mut ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<MA>,
    {
        Ok(LocalSupplyMissionSaveloadData {
            owner: ConvertSaveload::convert_into(&self.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_into(&self.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_into(&self.home_room_datas, &mut ids)?,
            source_mining_missions: ConvertSaveload::convert_into(&self.source_mining_missions, &mut ids)?,
            mineral_mining_missions: ConvertSaveload::convert_into(&self.mineral_mining_missions, &mut ids)?,
            transfer_mission: ConvertSaveload::convert_into(&self.transfer_mission, &mut ids)?,
            allow_spawning: ConvertSaveload::convert_into(&self.allow_spawning, &mut ids)?,
        })
    }

    fn convert_from<F>(data: Self::Data, mut ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(MA) -> Option<Entity>,
    {
        Ok(LocalSupplyMission {
            owner: ConvertSaveload::convert_from(data.owner, &mut ids)?,
            room_data: ConvertSaveload::convert_from(data.room_data, &mut ids)?,
            home_room_datas: ConvertSaveload::convert_from(data.home_room_datas, &mut ids)?,
            source_mining_missions: ConvertSaveload::convert_from(data.source_mining_missions, &mut ids)?,
            mineral_mining_missions: ConvertSaveload::convert_from(data.mineral_mining_missions, &mut ids)?,
            transfer_mission: ConvertSaveload::convert_from(data.transfer_mission, &mut ids)?,
            allow_spawning: ConvertSaveload::convert_from(data.allow_spawning, &mut ids)?,
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LocalSupplyMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LocalSupplyMission::new(owner, room_data, home_room_datas);

        builder
            .with(MissionData::LocalSupply(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity, home_room_datas: &[Entity]) -> LocalSupplyMission {
        LocalSupplyMission {
            owner: owner.into(),
            room_data,
            home_room_datas: home_room_datas.to_owned().into(),
            source_mining_missions: EntityVec::new(),
            mineral_mining_missions: EntityVec::new(),
            transfer_mission: None.into(),
            allow_spawning: true,
        }
    }

    pub fn set_home_rooms(&mut self, home_room_datas: &[Entity]) {
        if self.home_room_datas.as_slice() != home_room_datas {
            self.home_room_datas = home_room_datas.to_owned().into();
        }
    }

    pub fn allow_spawning(&mut self, allow: bool) {
        self.allow_spawning = allow;
    }

    /// Ensure child missions exist for all sources, minerals, and transfers.
    fn ensure_children(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        // Collect the data we need from room_data, then drop the borrow so we
        // can later get mutable access to add missions.
        let (room_name, sources, mineral_extractor_pairs) = {
            let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
            let room_name = room_data.name;

            let has_visibility = room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false);

            // Refresh structure data if stale.
            {
                let structure_data_rc = system_data.supply_structure_cache.get_room(room_name);
                let mut sd = structure_data_rc.maybe_access(
                    |d| game::time() - d.last_updated >= 10 && has_visibility,
                    || create_structure_data(room_data),
                );
                let _ = sd.get();
            }

            let static_visibility_data = match room_data.get_static_visibility_data() {
                Some(svd) => svd,
                None => {
                    system_data.visibility.request(VisibilityRequest::new(
                        room_data.name,
                        VISIBILITY_PRIORITY_CRITICAL,
                        VisibilityRequestFlags::ALL,
                    ));
                    return Ok(());
                }
            };

            let sources: Vec<RemoteObjectId<Source>> = static_visibility_data.sources().clone();

            let mineral_extractor_pairs: Vec<MineralExtractorPair> = {
                let structure_data_rc = system_data.supply_structure_cache.get_room(room_name);
                let sd_borrow = structure_data_rc.borrow();
                sd_borrow
                    .as_ref()
                    .map(|sd| sd.mineral_extractors_to_containers.keys().cloned().collect())
                    .unwrap_or_default()
            };

            (room_name, sources, mineral_extractor_pairs)
        };
        // room_data borrow is now dropped.

        // Ensure one SourceMiningMission per source.
        for source_id in sources.iter() {
            let already_exists = self.source_mining_missions.iter().any(|&mission_e| {
                system_data
                    .missions
                    .get(mission_e)
                    .as_mission_type::<SourceMiningMission>()
                    .map(|m| *m.source() == *source_id)
                    .unwrap_or(false)
            });

            if !already_exists {
                let child_entity = SourceMiningMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(mission_entity),
                    self.room_data,
                    &self.home_room_datas,
                    *source_id,
                    room_name,
                )
                .build();

                if let Some(room_data_mut) = system_data.room_data.get_mut(self.room_data) {
                    room_data_mut.add_mission(child_entity);
                }
                self.source_mining_missions.push(child_entity);
            }
        }

        // Ensure one MineralMiningMission per mineral/extractor pair.
        for (mineral_id, extractor_id) in mineral_extractor_pairs {
            let already_exists = self.mineral_mining_missions.iter().any(|&mission_e| {
                system_data
                    .missions
                    .get(mission_e)
                    .as_mission_type::<MineralMiningMission>()
                    .map(|m| *m.mineral() == mineral_id && *m.extractor() == extractor_id)
                    .unwrap_or(false)
            });

            if !already_exists {
                let child_entity = MineralMiningMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(mission_entity),
                    self.room_data,
                    &self.home_room_datas,
                    mineral_id,
                    extractor_id,
                    room_name,
                )
                .build();

                if let Some(room_data_mut) = system_data.room_data.get_mut(self.room_data) {
                    room_data_mut.add_mission(child_entity);
                }
                self.mineral_mining_missions.push(child_entity);
            }
        }

        // Ensure one RoomTransferMission.
        if self.transfer_mission.is_none() {
            let child_entity = RoomTransferMission::build(
                system_data.updater.create_entity(system_data.entities),
                Some(mission_entity),
                self.room_data,
                room_name,
            )
            .build();

            if let Some(room_data_mut) = system_data.room_data.get_mut(self.room_data) {
                room_data_mut.add_mission(child_entity);
            }
            self.transfer_mission = Some(child_entity).into();
        }

        Ok(())
    }

    /// Push `home_room_datas` and `allow_spawning` down to child missions.
    fn update_children(&self, system_data: &mut MissionExecutionSystemData) {
        for &child_entity in self.source_mining_missions.iter() {
            if let Some(mut child) = system_data.missions.get(child_entity).as_mission_type_mut::<SourceMiningMission>() {
                child.set_home_rooms(&self.home_room_datas);
                child.allow_spawning(self.allow_spawning);
            }
        }

        for &child_entity in self.mineral_mining_missions.iter() {
            if let Some(mut child) = system_data.missions.get(child_entity).as_mission_type_mut::<MineralMiningMission>() {
                child.set_home_rooms(&self.home_room_datas);
                child.allow_spawning(self.allow_spawning);
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for LocalSupplyMission {
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

    fn get_children(&self) -> Vec<Entity> {
        let mut children = Vec::new();
        children.extend(self.source_mining_missions.iter());
        children.extend(self.mineral_mining_missions.iter());
        if let Some(e) = *self.transfer_mission {
            children.push(e);
        }
        children
    }

    fn child_complete(&mut self, child: Entity) {
        self.source_mining_missions.retain(|&e| e != child);
        self.mineral_mining_missions.retain(|&e| e != child);
        if self.transfer_mission.map(|e| e == child).unwrap_or(false) {
            self.transfer_mission.take();
        }
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        format!(
            "Local Supply - Sources: {} Minerals: {}",
            self.source_mining_missions.len(),
            self.mineral_mining_missions.len()
        )
    }

    fn summarize(&self) -> crate::visualization::SummaryContent {
        use crate::visualization::SummaryContent;
        SummaryContent::Lines {
            header: "Local Supply".to_string(),
            items: vec![
                format!("Source missions: {}", self.source_mining_missions.len()),
                format!("Mineral missions: {}", self.mineral_mining_missions.len()),
                format!("Transfer: {}", if self.transfer_mission.is_some() { "active" } else { "none" }),
            ],
        }
    }

    fn pre_run_mission(&mut self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        self.ensure_children(system_data, mission_entity)?;
        self.update_children(system_data);

        Ok(MissionResult::Running)
    }
}
