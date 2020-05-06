use super::data::*;
use super::operationsystem::*;
use crate::missions::claim::*;
use crate::missions::data::*;
use crate::missions::remotebuild::*;
use crate::ownership::*;
use crate::room::data::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use crate::room::gather::*;
use crate::room::roomplansystem::*;

#[derive(Clone, ConvertSaveload)]
pub struct ClaimOperation {
    owner: EntityOption<OperationOrMissionEntity>,
    claim_missions: EntityVec<Entity>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ClaimOperation {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ClaimOperation::new(owner);

        builder.with(OperationData::Claim(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>) -> ClaimOperation {
        ClaimOperation {
            owner: owner.into(),
            claim_missions: EntityVec::new(),
        }
    }

    fn gather_candidate_room_data(gather_system_data: &GatherSystemData, room_name: RoomName) -> Option<CandidateRoomData> {
        let search_room_entity = gather_system_data.mapping.get_room(&room_name)?;
        let search_room_data = gather_system_data.room_data.get(search_room_entity)?;
        
        let static_visibility_data = search_room_data.get_static_visibility_data()?;
        let dynamic_visibility_data = search_room_data.get_dynamic_visibility_data()?;

        let has_controller = static_visibility_data.controller().is_some();
        let has_sources = !static_visibility_data.sources().is_empty();

        let visibility_timeout = if has_sources {
            5000
        } else {
            10000
        };

        if !dynamic_visibility_data.updated_within(visibility_timeout) {
            return None;
        }

        let can_claim = dynamic_visibility_data.owner().neutral() && (dynamic_visibility_data.reservation().mine() || dynamic_visibility_data.reservation().neutral());
        let hostile = dynamic_visibility_data.owner().hostile() || dynamic_visibility_data.source_keeper();

        let viable = has_controller && has_sources && can_claim;
        let can_expand = !hostile;

        let candidate_room_data = CandidateRoomData::new(search_room_entity, viable, can_expand);

        Some(candidate_room_data)
    }

    fn spawn_remote_build(
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData
    ) {
        //
        // Ensure remote builders occur.
        //

        for (entity, room_data) in (&*system_data.entities, &*system_data.room_data).join() {
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
                                let mission_data = system_data.mission_data;

                                let has_remote_build_mission = room_data.get_missions().iter().any(|mission_entity| {
                                    match mission_data.get(*mission_entity) {
                                        Some(MissionData::RemoteBuild(_)) => true,
                                        _ => false,
                                    }
                                });

                                //
                                // Spawn a new mission to fill the remote build role if missing.
                                //

                                if !has_remote_build_mission {
                                    info!("Starting remote build for room. Room: {}", room_data.name);

                                    let room_entity = entity;

                                    let construction_site_pos = spawn_construction_site.pos();
                                    let mut nearest_spawn = None;

                                    //TODO: Replace this hack that finds the nearest room. (Need state machine to drive operation. Blocked on specs vec serialization.)
                                    for (other_entity, other_room_data) in (&*system_data.entities, &*system_data.room_data).join() {
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
                                        let owner_entity = runtime_data.entity;
                                        let home_room_data_entity = nearest_spawn_room_entity;

                                        system_data.updater.exec_mut(move |world| {
                                            let mission_entity = RemoteBuildMission::build(
                                                world.create_entity(),
                                                Some(OperationOrMissionEntity::Operation(owner_entity)),
                                                room_entity,
                                                home_room_data_entity,
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
                }
            }
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
#[allow(clippy::cognitive_complexity)]
impl Operation for ClaimOperation {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: OperationOrMissionEntity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn child_complete(&mut self, child: Entity) {
        self.claim_missions.retain(|e| *e != child);
    }

    fn describe(&mut self, _system_data: &mut OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Claim".to_string(), None);
        })
    }

    fn pre_run_operation(&mut self, _system_data: &mut OperationExecutionSystemData, _runtime_data: &mut OperationExecutionRuntimeData) {
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        
        Self::spawn_remote_build(system_data, runtime_data);

        //
        // Trigger new claim missions.
        //

        let mut currently_owned_rooms = 0;

        for (_, room_data) in (&*system_data.entities, &*system_data.room_data).join() {
            if let Some(dynamic_visibility_data) = room_data.get_dynamic_visibility_data() {
                if dynamic_visibility_data.visible() && dynamic_visibility_data.owner().mine() {
                    currently_owned_rooms += 1;
                }
            }
        }

        //
        // Only allow as many missions in progress as would reach GCL/CPU cap.
        //

        //TODO: Need better dynamic estimation of room cost.
        const ESTIMATED_ROOM_CPU_COST: f64 = 10.0;
        let cpu_limit = game::cpu::limit();

        let current_gcl = game::gcl::level();
        let maximum_rooms = ((cpu_limit / ESTIMATED_ROOM_CPU_COST) as u32).min(current_gcl);

        if currently_owned_rooms + self.claim_missions.len() >= maximum_rooms as usize {
            return Ok(OperationResult::Running);
        }

        if game::time() % 50 != 25 {
            return Ok(OperationResult::Running);
        }

        let gather_system_data = GatherSystemData {
            entities: system_data.entities,
            mapping: system_data.mapping,
            room_data: system_data.room_data,
        };
        
        let gathered_data = gather_candidate_rooms(&gather_system_data, 2, Self::gather_candidate_room_data);

        for unknown_room in gathered_data.unknown_rooms().iter() {
            system_data
                .visibility
                .request(VisibilityRequest::new(unknown_room.room_name(), VISIBILITY_PRIORITY_MEDIUM));
        }

        for candidate_room in gathered_data.candidate_rooms().iter() {
            let room_data = system_data.room_data.get_mut(candidate_room.room_data_entity()).unwrap();

            //
            // Skip rooms that don't have sufficient sources. (If it is not known yet if they do, at least scout.)
            //

            if let Some(static_visibility_data) = room_data.get_static_visibility_data() {
                if static_visibility_data.sources().len() < 2 {
                    continue;
                }
            }

            //
            // Ensure a plan exists for the room or request one if it doesn't.
            //

            if system_data.room_plan_data.get(candidate_room.room_data_entity()).is_none() {
                system_data.room_plan_queue.request(RoomPlanRequest::new(room_data.name, 0.5));

                continue;
            }

            let mission_data = system_data.mission_data;

            //TODO: wiarchbe: Use trait instead of match.
            let has_claim_mission =
                room_data
                    .get_missions()
                    .iter()
                    .any(|mission_entity| match mission_data.get(*mission_entity) {
                        Some(MissionData::Claim(_)) => true,
                        _ => false,
                    });

            //
            // Spawn a new mission to fill the claim role if missing.
            //

            if !has_claim_mission {
                info!("Starting claim for room. Room: {}", room_data.name);

                let mission_entity = ClaimMission::build(
                    system_data.updater.create_entity(system_data.entities),
                    Some(OperationOrMissionEntity::Operation(runtime_data.entity)),
                    candidate_room.room_data_entity(),
                    candidate_room.home_room_data_entity(),
                )
                .build();

                room_data.add_mission(mission_entity);

                self.claim_missions.push(mission_entity);

                if currently_owned_rooms + self.claim_missions.len() >= maximum_rooms as usize {
                    break
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
