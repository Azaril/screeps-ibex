use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::miningoutpost::*;
use crate::room::gather::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct MiningOutpostOperation {
    owner: EntityOption<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl MiningOutpostOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = MiningOutpostOperation::new(owner);

        builder.with(OperationData::MiningOutpost(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> MiningOutpostOperation {
        MiningOutpostOperation { owner: owner.into() }
    }

    fn gather_candidate_room_data(gather_system_data: &GatherSystemData, room_name: RoomName) -> Option<CandidateRoomData> {
        let search_room_entity = gather_system_data.mapping.get_room(&room_name)?;
        let search_room_data = gather_system_data.room_data.get(search_room_entity)?;

        let static_visibility_data = search_room_data.get_static_visibility_data()?;
        let dynamic_visibility_data = search_room_data.get_dynamic_visibility_data()?;

        let has_sources = !static_visibility_data.sources().is_empty();

        let visibility_timeout = if has_sources { 3000 } else { 10000 };

        if !dynamic_visibility_data.updated_within(visibility_timeout) {
            return None;
        }

        let can_reserve = dynamic_visibility_data.owner().neutral()
            && (dynamic_visibility_data.reservation().neutral() || dynamic_visibility_data.reservation().mine());
        let hostile = dynamic_visibility_data.owner().hostile() || dynamic_visibility_data.source_keeper();

        let viable = has_sources && can_reserve;
        let can_expand = !hostile;

        let candidate_room_data = CandidateRoomData::new(search_room_entity, viable, can_expand);

        Some(candidate_room_data)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for MiningOutpostOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn describe(&mut self, _system_data: &mut OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Remote Mine".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        if game::time() % 50 != 25 {
            return Ok(OperationResult::Running);
        }

        let gather_system_data = GatherSystemData {
            entities: system_data.entities,
            mapping: system_data.mapping,
            room_data: system_data.room_data,
        };

        let gathered_data = gather_candidate_rooms(&gather_system_data, 1, Self::gather_candidate_room_data);

        for unknown_room in gathered_data.unknown_rooms().iter() {
            system_data
                .visibility
                .request(VisibilityRequest::new(unknown_room.room_name(), VISIBILITY_PRIORITY_MEDIUM));
        }

        for candidate_room in gathered_data.candidate_rooms().iter() {
            let room_data_storage = &mut *system_data.room_data;
            let room_data = room_data_storage.get_mut(candidate_room.room_data_entity()).unwrap();
            let dynamic_visibility_data = room_data.get_dynamic_visibility_data();

            //
            // Spawn mining outpost missions for rooms that are not hostile and have recent visibility.
            //

            if let Some(dynamic_visibility_data) = dynamic_visibility_data {
                if !dynamic_visibility_data.updated_within(1000)
                    || !dynamic_visibility_data.owner().neutral()
                    || dynamic_visibility_data.reservation().friendly()
                    || dynamic_visibility_data.reservation().hostile()
                    || dynamic_visibility_data.source_keeper()
                {
                    continue;
                }

                //TODO: Check path finding and accessibility to room.

                //TODO: wiarchbe: Use trait instead of match.
                let mission_data = system_data.mission_data;

                let has_mining_outpost_mission =
                    room_data
                        .get_missions()
                        .iter()
                        .any(|mission_entity| match mission_data.get(*mission_entity) {
                            Some(MissionData::MiningOutpost(_)) => true,
                            _ => false,
                        });

                //
                // Spawn a new mission to fill the mining outpost role if missing.
                //

                if !has_mining_outpost_mission {
                    info!("Starting mining outpost mission for room. Room: {}", room_data.name);

                    let mission_entity = MiningOutpostMission::build(
                        system_data.updater.create_entity(system_data.entities),
                        Some(runtime_data.entity),
                        candidate_room.room_data_entity(),
                        candidate_room.home_room_data_entity(),
                    )
                    .build();

                    room_data.add_mission(mission_entity);
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
