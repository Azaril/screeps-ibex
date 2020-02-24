use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

use super::data::*;
use super::operationsystem::*;
use crate::missions::claim::*;
use crate::missions::data::*;
use crate::missions::remotebuild::*;
use crate::missions::scout::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;

#[derive(Clone, ConvertSaveload)]
pub struct ClaimOperation {
    claim_missions: EntityVec,
}

impl ClaimOperation {
    pub fn build<B>(builder: B) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ClaimOperation::new();

        builder
            .with(OperationData::Claim(operation))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new() -> ClaimOperation {
        ClaimOperation {
            claim_missions: EntityVec::new(),
        }
    }
}

#[allow(clippy::cognitive_complexity)]
impl Operation for ClaimOperation {
    fn describe(&mut self, _system_data: &OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Claim".to_string(), None);
        })
    }

    fn pre_run_operation(&mut self, system_data: &OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {
        //
        // Cleanup missions that no longer exist.
        //

        self.claim_missions.0.retain(|entity| system_data.entities.is_alive(*entity));
    }

    fn run_operation(
        &mut self,
        system_data: &OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        scope_timing!("ClaimOperation");

        let current_gcl = game::gcl::level();
        let current_claim_missions = self.claim_missions.0.len();
        let mut currently_owned_rooms = 0;

        for (_, room_data) in (system_data.entities, system_data.room_data).join() {
            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.owner().mine() {
                    currently_owned_rooms += 1;
                }
            }
        }

        //
        // Only allow as many missions in progress as would reach GCL cap.
        //

        //TODO: Only 'add' rooms if GCL limit is not reached.

        if currently_owned_rooms + current_claim_missions >= (current_gcl as usize) {
            return Ok(OperationResult::Running);
        }

        //TODO: Do this in a single pass and use closest room to be the home room.

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
                    for _ in 0..2 {
                        candidate_rooms = candidate_rooms
                            .into_iter()
                            .flat_map(|room_name| {
                                game::map::describe_exits(room_name)
                                    .values()
                                    .cloned()
                                    .chain(std::iter::once(room_name))
                                    .collect::<Vec<RoomName>>()
                            })
                            .filter(|room_name| {
                                if let Some(search_room_entity) = system_data.mapping.rooms.get(&room_name) {
                                    if let Some(search_room_data) = system_data.room_data.get(*search_room_entity) {
                                        if let Some(search_room_visibility_data) = search_room_data.get_dynamic_visibility_data() {
                                            if search_room_visibility_data.updated_within(5000)
                                                && (search_room_visibility_data.owner().hostile()
                                                    || search_room_visibility_data.source_keeper())
                                            {
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
                        if let Some(offset_room_entity) = system_data.mapping.rooms.get(&offset_room_name) {
                            desired_missions.push((*offset_room_entity, entity));
                        } else {
                            runtime_data
                                .visibility
                                .request(VisibilityRequest::new(offset_room_name, VISIBILITY_PRIORITY_MEDIUM));
                        }
                    }
                }
            }
        }

        for (room_data_entity, home_room_data_entity) in desired_missions {
            let room_data = system_data.room_data.get(room_data_entity).unwrap();

            //
            // Skip rooms that don't have sufficient sources. (If it is not known yet if they do, at least scout.)
            //

            if let Some(static_visibility_data) = room_data.get_static_visibility_data() {
                if static_visibility_data.sources().len() < 2 {
                    continue;
                }
            }

            let dynamic_visibility_data = room_data.get_dynamic_visibility_data();

            //
            // Spawn scout missions for claim rooms that have not had visibility updated in a long time.
            //

            if dynamic_visibility_data.as_ref().map(|v| !v.updated_within(1000)).unwrap_or(true) {
                //TODO: wiarchbe: Use trait instead of match.
                let has_scout_mission =
                    room_data
                        .missions
                        .0
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

                    let room_entity = room_data_entity;
                    let home_room_entity = home_room_data_entity;

                    system_data.updater.exec_mut(move |world| {
                        let mission_entity = ScoutMission::build(world.create_entity(), room_entity, home_room_entity).build();

                        let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();

                        if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                            room_data.missions.0.push(mission_entity);
                        }
                    });
                }
            }

            //
            // Spawn claim missions for rooms that are not owned and have recent visibility.
            //

            let can_claim = dynamic_visibility_data
                .as_ref()
                .map(|v| {
                    v.updated_within(1000)
                        && v.owner().neutral()
                        && (v.reservation().neutral() || v.reservation().mine())
                        && !v.source_keeper()
                })
                .unwrap_or(false);

            if can_claim {
                //TODO: Check path finding and accessibility to room.

                //TODO: wiarchbe: Use trait instead of match.
                let has_claim_mission =
                    room_data
                        .missions
                        .0
                        .iter()
                        .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                            Some(MissionData::Claim(_)) => true,
                            _ => false,
                        });

                //
                // Spawn a new mission to fill the claim role if missing.
                //

                if !has_claim_mission {
                    info!("Starting claim for room. Room: {}", room_data.name);

                    let operation_entity = *runtime_data.entity;
                    let room_entity = room_data_entity;
                    let home_room_entity = home_room_data_entity;

                    system_data.updater.exec_mut(move |world| {
                        let mission_entity = ClaimMission::build(world.create_entity(), room_entity, home_room_entity).build();

                        let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();

                        if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                            room_data.missions.0.push(mission_entity);
                        }

                        let operation_data_storage = &mut world.write_storage::<::operations::data::OperationData>();

                        if let Some(OperationData::Claim(operation_data)) = operation_data_storage.get_mut(operation_entity) {
                            operation_data.claim_missions.0.push(mission_entity);
                        }
                    });
                }
            }
        }

        for (entity, room_data) in (system_data.entities, system_data.room_data).join() {
            //TODO: The construction operation will trigger construction sites - this is brittle to rely on.

            //
            // Spawn remote build for rooms that are owned and have a spawn construction site.
            //

            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
                    if let Some(room) = game::rooms::get(room_data.name) {
                        let spawns = room.find(find::MY_SPAWNS);

                        if spawns.is_empty() {
                            let construction_sites = room.find(find::CONSTRUCTION_SITES);

                            let spawn_construction_site = construction_sites
                                .into_iter()
                                .find(|construction_site| construction_site.structure_type() == StructureType::Spawn);

                            if let Some(spawn_construction_site) = spawn_construction_site {
                                //TODO: wiarchbe: Use trait instead of match.
                                let has_remote_build_mission =
                                    room_data
                                        .missions
                                        .0
                                        .iter()
                                        .any(|mission_entity| match system_data.mission_data.get(*mission_entity) {
                                            Some(MissionData::RemoteBuild(_)) => true,
                                            _ => false,
                                        });

                                //
                                // Spawn a new mission to fill the remote build role if missing.
                                //

                                if !has_remote_build_mission {
                                    info!("Starting remote build for room. Room: {}", room_data.name);

                                    let room_entity = entity;

                                    let construction_site_pos = spawn_construction_site.pos();
                                    let mut nearest_spawn = None;

                                    //TOODO: Replace this hack that finds the nearest room. (Need state machine to drive operation. Blocked on specs vec serialization.)
                                    for (other_entity, other_room_data) in (system_data.entities, system_data.room_data).join() {
                                        if let Some(other_dynamic_visibility_data) = other_room_data.get_dynamic_visibility_data() {
                                            if other_dynamic_visibility_data.visible() && other_dynamic_visibility_data.owner().mine() {
                                                if let Some(other_room) = game::rooms::get(other_room_data.name) {
                                                    let spawns = other_room.find(find::MY_SPAWNS);

                                                    for spawn in spawns {
                                                        let distance = construction_site_pos.get_range_to(&spawn.pos());
                                                        if let Some((nearest_distance, _)) = nearest_spawn {
                                                            if distance < nearest_distance {
                                                                nearest_spawn = Some((distance, other_entity));
                                                            }
                                                        } else {
                                                            nearest_spawn = Some((distance, other_entity));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if let Some((_, nearest_spawn_room_entity)) = nearest_spawn {
                                        let home_room_data_entity = nearest_spawn_room_entity;

                                        system_data.updater.exec_mut(move |world| {
                                            let mission_entity =
                                                RemoteBuildMission::build(world.create_entity(), room_entity, home_room_data_entity)
                                                    .build();

                                            let room_data_storage = &mut world.write_storage::<::room::data::RoomData>();

                                            if let Some(room_data) = room_data_storage.get_mut(room_entity) {
                                                room_data.missions.0.push(mission_entity);
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
