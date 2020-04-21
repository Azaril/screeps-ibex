use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::remotemine::*;
use crate::missions::reserve::*;
use crate::missions::scout::*;
use crate::ownership::*;
use crate::room::data::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use std::collections::HashMap;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, ConvertSaveload)]
pub struct RemoteMineOperation {
    owner: EntityOption<OperationOrMissionEntity>,
}

struct CandidateRoomData {
    room_data_entity: Entity,
    viable: bool,
    can_expand: bool
}

struct CandidateRoom {
    room_data_entity: Entity,
    home_room_data_entity: Entity
}

struct UnknownRoom {
    room_name: RoomName,
    home_room_data_entity: Entity,
}

struct GatherRoomData {
    candidate_rooms: Vec<CandidateRoom>,
    unknown_rooms: Vec<UnknownRoom>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RemoteMineOperation {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = RemoteMineOperation::new(owner);

        builder.with(OperationData::RemoteMine(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>) -> RemoteMineOperation {
        RemoteMineOperation { owner: owner.into() }
    }

    fn gather_candidate_room_data(system_data: &OperationExecutionSystemData, room_name: RoomName) -> Option<CandidateRoomData> {
        let search_room_entity = system_data.mapping.rooms.get(&room_name)?;
        let search_room_data = system_data.room_data.get(*search_room_entity)?;
        
        let static_visibility_data = search_room_data.get_static_visibility_data()?;
        let dynamic_visibility_data = search_room_data.get_dynamic_visibility_data()?;

        //TODO: Look at update time?

        let has_sources = !static_visibility_data.sources().is_empty();

        let can_reserve = dynamic_visibility_data.owner().neutral() && dynamic_visibility_data.reservation().neutral();
        let hostile = dynamic_visibility_data.owner().hostile() || dynamic_visibility_data.source_keeper();

        let candidate_room_data = CandidateRoomData {
            room_data_entity: *search_room_entity,
            viable: has_sources && can_reserve,
            can_expand: !hostile
        };

        Some(candidate_room_data)
    }

    fn gather_candidate_rooms(system_data: &OperationExecutionSystemData, max_distance: u32) -> GatherRoomData {
        let mut unknown_rooms = HashMap::new();

        struct VisitedRoomData {
            room_data_entity: Entity,
            home_room_data_entity: Entity,
            distance: u32,
            viable: bool,
            can_expand: bool
        };

        let mut visited_rooms: HashMap<RoomName, VisitedRoomData> = HashMap::new();
        let mut expansion_rooms: HashMap<RoomName, Entity> = HashMap::new();

        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                let seed_room = room.controller()
                    .map(|controller| controller.my() && controller.level() >= 2)
                    .unwrap_or(false);

                if seed_room {
                    let visited_room = VisitedRoomData {
                        room_data_entity: entity,
                        home_room_data_entity: entity,
                        distance: 0,
                        viable: false,
                        can_expand: true
                    };

                    if visited_room.can_expand {
                        let room_exits = game::map::describe_exits(room_data.name);

                        //let source_room_status = game::map::get_room_status(*source_room_name);

                        for expansion_room in room_exits.values() {
                            //TODO: Make room status public...
                            /*
                            let expansion_room_status = game::map::get_room_status(*expansion_room);

                            let can_expand = match expansion_room_status.status {
                                game::map::RoomStatus::Normal => source_room_status.status == game::map::RoomStatus::Normal,
                                game::map::RoomStatus::Closed => false,
                                game::map::RoomStatus::Novice => source_room_status.status == game::map::RoomStatus::Novice,
                                game::map::RoomStatus::Respawn => source_room_status.status == game::map::RoomStatus::Respawn,
                            };
                            */

                            let can_expand = true;

                            if can_expand {
                                expansion_rooms.insert(*expansion_room, entity);
                            }
                        }
                    }  
                    
                    visited_rooms.insert(room_data.name, visited_room);
                }
            }
        }

        let mut distance = 1;

        while !expansion_rooms.is_empty() && distance <= max_distance {
            let next_rooms: HashMap<RoomName, Entity> = std::mem::replace(&mut expansion_rooms, HashMap::new());

            for (source_room_name, home_room_entity) in next_rooms.iter() {
                if !visited_rooms.contains_key(source_room_name) {
                    let candiate_room_data = Self::gather_candidate_room_data(system_data, *source_room_name);

                    if let Some(candidate_room_data) = candiate_room_data {
                        let visited_room = VisitedRoomData {
                            room_data_entity: candidate_room_data.room_data_entity,
                            home_room_data_entity: *home_room_entity,
                            distance,
                            viable: candidate_room_data.viable,
                            can_expand: candidate_room_data.can_expand
                        };

                        if visited_room.can_expand {
                            let room_exits = game::map::describe_exits(*source_room_name);

                            //let source_room_status = game::map::get_room_status(*source_room_name);
    
                            for expansion_room in room_exits.values() {
                                //TODO: Make room status public...
                                /*
                                let expansion_room_status = game::map::get_room_status(*expansion_room);

                                let can_expand = match expansion_room_status.status {
                                    game::map::RoomStatus::Normal => source_room_status.status == game::map::RoomStatus::Normal,
                                    game::map::RoomStatus::Closed => false,
                                    game::map::RoomStatus::Novice => source_room_status.status == game::map::RoomStatus::Novice,
                                    game::map::RoomStatus::Respawn => source_room_status.status == game::map::RoomStatus::Respawn,
                                };
                                */

                                let can_expand = true;

                                if can_expand {
                                    expansion_rooms.insert(*expansion_room, *home_room_entity);
                                }
                            }
                        }
                        
                        visited_rooms.insert(*source_room_name, visited_room);
                    } else {
                        unknown_rooms.insert(*source_room_name, *home_room_entity);
                    }
                }
            }

            distance += 1;
        }

        let candidate_rooms = visited_rooms
            .values()
            .filter(|v| v.viable)
            .map(|v| CandidateRoom { room_data_entity: v.room_data_entity, home_room_data_entity: v.home_room_data_entity })
            .collect();

        let returned_unknown_rooms = unknown_rooms
            .into_iter()
            .map(|(room_name, home_room_data_entity)| UnknownRoom { room_name, home_room_data_entity })
            .collect();

        GatherRoomData {
            candidate_rooms,
            unknown_rooms: returned_unknown_rooms
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for RemoteMineOperation {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn describe(&mut self, _system_data: &OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Remote Mine".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        if !crate::features::remote_mine::harvest() {
            return Ok(OperationResult::Running);
        }

        let gathered_data = Self::gather_candidate_rooms(system_data, 1);

        for unknown_room in gathered_data.unknown_rooms.iter() {
            runtime_data
                .visibility
                .request(VisibilityRequest::new(unknown_room.room_name, VISIBILITY_PRIORITY_MEDIUM));
        }  
        
        //TODO: Move this to visibility system.
        for unknown_room in gathered_data.unknown_rooms.iter() {
            if let Some(room_entity) = system_data.mapping.rooms.get(&unknown_room.room_name) {
                if let Some(room_data) = system_data.room_data.get(*room_entity) {
                    let dynamic_visibility_data = room_data.get_dynamic_visibility_data();

                    //
                    // Spawn scout missions for remote mine rooms that have not had visibility updated in a long time.
                    //

                    if dynamic_visibility_data.as_ref().map(|v| !v.updated_within(1000)).unwrap_or(true) {
                        //TODO: wiarchbe: Use trait instead of match.
                        let has_scout_mission =
                            room_data
                                .get_missions()
                                .iter()
                                .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                                    Some(MissionData::Scout(_)) => true,
                                    _ => false,
                                });

                        //
                        // Spawn a new mission to fill the scout role if missing.
                        //

                        if !has_scout_mission {
                            info!("Starting scout for room. Room: {}", room_data.name);

                            let owner_entity = *runtime_data.entity;
                            let room_entity = *room_entity;
                            let home_room_entity = unknown_room.home_room_data_entity;

                            system_data.updater.exec_mut(move |world| {
                                let mission_entity = ScoutMission::build(
                                    world.create_entity(),
                                    Some(OperationOrMissionEntity::Operation(owner_entity)),
                                    room_entity,
                                    home_room_entity,
                                )
                                .build();

                                let room_data_storage = &mut world.write_storage::<RoomData>();

                                if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                    room_data.add_mission(mission_entity);
                                }
                            });
                        }
                    }
                }
            }
        }

        for candidate_room in gathered_data.candidate_rooms.iter() {
            let room_data = system_data.room_data.get(candidate_room.room_data_entity).unwrap();
            let dynamic_visibility_data = room_data.get_dynamic_visibility_data();

            //
            // Spawn remote mine missions for rooms that are not hostile and have recent visibility.
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
                let has_remote_mine_mission =
                    room_data
                        .get_missions()
                        .iter()
                        .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                            Some(MissionData::RemoteMine(_)) => true,
                            _ => false,
                        });

                //
                // Spawn a new mission to fill the remote mine role if missing.
                //

                if !has_remote_mine_mission {
                    info!("Starting remote mine for room. Room: {}", room_data.name);

                    let owner_entity = *runtime_data.entity;
                    let room_entity = candidate_room.room_data_entity;
                    let home_room_entity = candidate_room.home_room_data_entity;

                    system_data.updater.exec_mut(move |world| {
                        let mission_entity = RemoteMineMission::build(
                            world.create_entity(),
                            Some(OperationOrMissionEntity::Operation(owner_entity)),
                            room_entity,
                            home_room_entity,
                        )
                        .build();

                        let room_data_storage = &mut world.write_storage::<RoomData>();

                        if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                            room_data.add_mission(mission_entity);
                        }
                    });
                }

                //TODO: wiarchbe: Use trait instead of match.
                let has_reservation_mission =
                    room_data
                        .get_missions()
                        .iter()
                        .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                            Some(MissionData::Reserve(_)) => true,
                            _ => false,
                        });

                //
                // Spawn a new mission to fill the reservation role if missing.
                //

                if !has_reservation_mission && crate::features::remote_mine::reserve() {
                    info!("Starting reservation for room. Room: {}", room_data.name);

                    let owner_entity = *runtime_data.entity;
                    let room_entity = candidate_room.room_data_entity;
                    let home_room_entity = candidate_room.home_room_data_entity;

                    system_data.updater.exec_mut(move |world| {
                        let mission_entity = ReserveMission::build(
                            world.create_entity(),
                            Some(OperationOrMissionEntity::Operation(owner_entity)),
                            room_entity,
                            home_room_entity,
                        )
                        .build();

                        let room_data_storage = &mut world.write_storage::<RoomData>();

                        if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                            room_data.add_mission(mission_entity);
                        }
                    });
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
