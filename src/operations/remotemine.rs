use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

use super::data::*;
use super::operationsystem::*;
use crate::missions::data::*;
use crate::missions::remotemine::*;
use crate::missions::scout::*;
use crate::missions::reserve::*;
use crate::room::visibilitysystem::*;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct RemoteMineOperation {}

impl RemoteMineOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = RemoteMineOperation::new();

        builder
            .with(OperationData::RemoteMine(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> RemoteMineOperation {
        RemoteMineOperation {}
    }
}

impl Operation for RemoteMineOperation {
    fn run_operation<'a>(
        &mut self,
        system_data: &'a OperationExecutionSystemData,
    ) -> OperationResult {
        scope_timing!("RemoteMineOperation");

        let mut desired_missions = vec![];

        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(room) = game::rooms::get(room_data.name) {
                let controller = room.controller();

                let (my_room, room_level) = controller
                    .map(|controller| (controller.my(), controller.level()))
                    .unwrap_or((false, 0));

                if my_room && room_level >= 2 {
                    let mut candidate_rooms = vec![room_data.name];

                    //TODO: Configure how far to expand room search.
                    for _ in 0..1 {
                        candidate_rooms = candidate_rooms
                            .into_iter()
                            .flat_map(|room_name| {
                                game::map::describe_exits(room_name)
                                    .values()
                                    .cloned()
                                    .collect::<Vec<RoomName>>()
                            })
                            .filter(|room_name| {
                                if let Some(search_room_entity) = system_data.mapping.rooms.get(&room_name) {
                                    if let Some(search_room_data) = system_data.room_data.get(*search_room_entity) {
                                        if let Some(search_room_data) = search_room_data.get_dynamic_visibility_data() {
                                            if search_room_data.updated_within(5000) && (search_room_data.hostile_owner() || search_room_data.source_keeper()) {
                                                return false;
                                            }
                                        }
                                    }
                                }
                                true
                            })
                            .unique()
                            .collect();
                    }

                    for offset_room_name in candidate_rooms {
                        if let Some(offset_room_entity) =
                            system_data.mapping.rooms.get(&offset_room_name)
                        {
                            desired_missions.push((*offset_room_entity, entity));
                        } else {
                            system_data.visibility.request(VisibilityRequest::new(
                                offset_room_name,
                                VISIBILITY_PRIORITY_MEDIUM,
                            ));
                        }
                    }
                }
            }
        }

        for (room_data_entity, home_room_data_entity) in desired_missions {
            let room_data = system_data.room_data.get(room_data_entity).unwrap();

            //
            // Skip rooms that are known to have no sources. (If it is not known yet if they do, at least scout.)
            //

            if let Some(static_visibility_data) = room_data.get_static_visibility_data() {
                if static_visibility_data.sources().is_empty() {
                    continue;
                }
            }

            let dynamic_visibility_data = room_data.get_dynamic_visibility_data();

            //
            // Spawn scout missions for remote mine rooms that have not had visibility updated in a long time.
            //

            if dynamic_visibility_data
                .as_ref()
                .map(|v| !v.updated_within(5000))
                .unwrap_or(true)
            {
                //TODO: wiarchbe: Use trait instead of match.
                let has_scout_mission = room_data.missions.0.iter().any(|mission_entity| {
                    match system_data.mission_data.get(*mission_entity) {
                        Some(MissionData::Scout(_)) => true,
                        _ => false,
                    }
                });

                //
                // Spawn a new mission to fill the scout role if missing.
                //

                if !has_scout_mission {
                    info!("Starting scout for room. Room: {}", room_data.name);

                    let room_entity = room_data_entity;
                    let home_room_entity = home_room_data_entity;

                    system_data.updater.exec_mut(move |world| {
                        let mission_entity = ScoutMission::build(
                            world.create_entity(),
                            room_entity,
                            home_room_entity,
                        )
                        .build();

                        let room_data_storage =
                            &mut world.write_storage::<::room::data::RoomData>();

                        if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                            room_data.missions.0.push(mission_entity);
                        }
                    });
                }
            }

            //
            // Spawn remote mine missions for rooms that are not hostile and have recent visibility.
            //

            if let Some(dynamic_visibility_data) = dynamic_visibility_data {
                if !dynamic_visibility_data.updated_within(1000) {
                    continue;
                }

                if dynamic_visibility_data.owner().is_some() || dynamic_visibility_data.source_keeper() {
                    continue;
                }

                if !dynamic_visibility_data.my()
                    && (dynamic_visibility_data.friendly_owner() || dynamic_visibility_data.hostile_owner())
                {
                    continue;
                }

                //TODO: Check path finding and accessibility to room.

                //TODO: wiarchbe: Use trait instead of match.
                let has_remote_mine_mission = room_data.missions.0.iter().any(|mission_entity| {
                    match system_data.mission_data.get(*mission_entity) {
                        Some(MissionData::RemoteMine(_)) => true,
                        _ => false,
                    }
                });

                //
                // Spawn a new mission to fill the remote mine role if missing.
                //

                if !has_remote_mine_mission {
                    info!("Starting remote mine for room. Room: {}", room_data.name);

                    let room_entity = room_data_entity;
                    let home_room_entity = home_room_data_entity;

                    system_data.updater.exec_mut(move |world| {
                        let mission_entity = RemoteMineMission::build(
                            world.create_entity(),
                            room_entity,
                            home_room_entity,
                        )
                        .build();

                        let room_data_storage =
                            &mut world.write_storage::<::room::data::RoomData>();

                        if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                            room_data.missions.0.push(mission_entity);
                        }
                    });
                }

                //TODO: wiarchbe: Use trait instead of match.
                let has_reservation_mission = room_data.missions.0.iter().any(|mission_entity| {
                    match system_data.mission_data.get(*mission_entity) {
                        Some(MissionData::Reserve(_)) => true,
                        _ => false,
                    }
                });

                //
                // Spawn a new mission to fill the reservation role if missing.
                //

                if !has_reservation_mission {
                    info!("Starting reservation for room. Room: {}", room_data.name);

                    let room_entity = room_data_entity;
                    let home_room_entity = home_room_data_entity;

                    system_data.updater.exec_mut(move |world| {
                        let mission_entity = ReserveMission::build(
                            world.create_entity(),
                            room_entity,
                            home_room_entity,
                        )
                        .build();

                        let room_data_storage =
                            &mut world.write_storage::<::room::data::RoomData>();

                        if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                            room_data.missions.0.push(mission_entity);
                        }
                    });
                }
            }
        }

        OperationResult::Running
    }
}
