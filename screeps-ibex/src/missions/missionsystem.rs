use super::data::*;
use super::localsupply::structure_data::SupplyStructureCache;
use crate::cleanup::*;
use crate::creep::*;
use crate::entitymappingsystem::EntityMappingData;
use crate::jobs::data::*;
use crate::military::boostqueue::*;
use crate::military::economy::*;
use crate::military::squad::SquadContext;
use crate::repairqueue::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use crate::room::visibilitysystem::*;
use crate::spawnsystem::*;
use crate::transfer::ordersystem::*;
use crate::transfer::transfersystem::*;
use crate::visualization::SummaryContent;
use log::*;
use specs::prelude::*;

#[derive(SystemData)]
pub struct MissionSystemData<'a> {
    updater: Read<'a, LazyUpdate>,
    missions: WriteStorage<'a, MissionData>,
    room_data: WriteStorage<'a, RoomData>,
    room_plan_data: ReadStorage<'a, RoomPlanData>,
    room_plan_queue: Write<'a, RoomPlanQueue>,
    entities: Entities<'a>,
    spawn_queue: Write<'a, SpawnQueue>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    creep_spawning: ReadStorage<'a, CreepSpawning>,
    job_data: WriteStorage<'a, JobData>,
    transfer_queue: Write<'a, TransferQueue>,
    order_queue: Write<'a, OrderQueue>,
    visibility: Write<'a, VisibilityQueue>,
    boost_queue: Write<'a, BoostQueue>,
    repair_queue: Write<'a, RepairQueue>,
    supply_structure_cache: Write<'a, SupplyStructureCache>,
    cleanup_queue: Write<'a, EntityCleanupQueue>,
    economy: Write<'a, EconomySnapshot>,
    route_cache: Write<'a, RoomRouteCache>,
    squad_contexts: WriteStorage<'a, SquadContext>,
    mapping: Read<'a, EntityMappingData>,
}

pub struct MissionExecutionSystemData<'a, 'b> {
    pub updater: &'b Read<'a, LazyUpdate>,
    pub room_data: &'b mut WriteStorage<'a, RoomData>,
    pub room_plan_data: &'b ReadStorage<'a, RoomPlanData>,
    pub entities: &'b Entities<'a>,
    pub creep_owner: &'b ReadStorage<'a, CreepOwner>,
    pub creep_spawning: &'b ReadStorage<'a, CreepSpawning>,
    pub job_data: &'b WriteStorage<'a, JobData>,
    pub missions: &'b WriteStorage<'a, MissionData>,
    pub cleanup_queue: &'b mut EntityCleanupQueue,
    pub spawn_queue: &'b mut SpawnQueue,
    pub room_plan_queue: &'b mut RoomPlanQueue,
    pub transfer_queue: &'b mut TransferQueue,
    pub order_queue: &'b mut OrderQueue,
    pub visibility: &'b mut Write<'a, VisibilityQueue>,
    pub boost_queue: &'b mut BoostQueue,
    pub repair_queue: &'b mut RepairQueue,
    pub supply_structure_cache: &'b mut SupplyStructureCache,
    pub economy: &'b mut EconomySnapshot,
    pub route_cache: &'b mut RoomRouteCache,
    pub squad_contexts: &'b mut WriteStorage<'a, SquadContext>,
    pub mapping: &'b Read<'a, EntityMappingData>,
}

/// Queue a mission for cleanup via the `EntityCleanupQueue`.
///
/// Extracts context from the live mission component and pushes a
/// `MissionCleanup` entry. If the entity has no `MissionData` yet
/// (created via `LazyUpdate` this tick), queues a deferred cleanup
/// via `LazyUpdate::exec_mut` as a fallback.
fn queue_mission_abort(system_data: &mut MissionExecutionSystemData, mission_entity: Entity) {
    if let Some(mission_data) = system_data.missions.get(mission_entity) {
        let mission = mission_data.as_mission();
        let owner = *mission.get_owner();
        let children = mission.get_children();
        let room = mission.get_room();
        drop(mission);

        system_data.cleanup_queue.delete_mission(MissionCleanup {
            entity: mission_entity,
            owner,
            children,
            room,
        });
    } else {
        // Entity exists but has no MissionData component yet (likely created
        // via LazyUpdate this tick). Queue deferred cleanup so it is deleted
        // once its components are applied during world.maintain().
        warn!(
            "Mission abort requested for entity {:?} but it has no MissionData \
             (component not yet applied). Queuing deferred cleanup.",
            mission_entity
        );
        let entity = mission_entity;
        system_data.updater.exec_mut(move |world| {
            if world.entities().is_alive(entity) {
                // Remove from any room that lists it.
                for room_data in (&mut world.write_storage::<RoomData>()).join() {
                    room_data.remove_mission(entity);
                }

                if let Err(err) = world.delete_entity(entity) {
                    warn!("Deferred cleanup of orphan mission entity {:?} failed: {}", entity, err);
                } else {
                    info!("Deferred cleanup: deleted orphan mission entity {:?}", entity);
                }
            }
        });
    }
}

pub enum MissionResult {
    Running,
    Success,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub trait Mission {
    fn get_owner(&self) -> &Option<Entity>;

    fn owner_complete(&mut self, owner: Entity);

    fn get_room(&self) -> Entity;

    fn get_children(&self) -> Vec<Entity> {
        Vec::new()
    }

    fn child_complete(&mut self, _child: Entity) {}

    /// Remove any internal entity references that fail the validity check.
    ///
    /// Called by `repair_entity_integrity` before serialization to prevent
    /// `ConvertSaveload` panics on dangling entities. Default is a no-op.
    fn repair_entity_refs(&mut self, _is_valid: &dyn Fn(Entity) -> bool) {}

    /// Called by `EntityCleanupSystem` when a creep entity dies.
    /// Missions that track creeps should override this to remove the
    /// entity from their tracking lists.
    fn remove_creep(&mut self, _entity: Entity) {}

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String;

    /// Produce a structured summary for the visualization overlay.
    /// Reads only `self`; no system data required. Override in concrete missions for richer detail.
    fn summarize(&self) -> SummaryContent {
        SummaryContent::Text("Mission".to_string())
    }

    fn pre_run_mission(&mut self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String>;
}

pub struct PreRunMissionSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mission_entities: Vec<Entity> = (&data.entities, &data.missions).join().map(|(e, _)| e).collect();

        for entity in mission_entities {
            let mut system_data = MissionExecutionSystemData {
                updater: &data.updater,
                entities: &data.entities,
                room_data: &mut data.room_data,
                room_plan_data: &data.room_plan_data,
                creep_owner: &data.creep_owner,
                creep_spawning: &data.creep_spawning,
                job_data: &data.job_data,
                missions: &data.missions,
                cleanup_queue: &mut data.cleanup_queue,
                spawn_queue: &mut data.spawn_queue,
                room_plan_queue: &mut data.room_plan_queue,
                transfer_queue: &mut data.transfer_queue,
                order_queue: &mut data.order_queue,
                visibility: &mut data.visibility,
                boost_queue: &mut data.boost_queue,
                repair_queue: &mut data.repair_queue,
                supply_structure_cache: &mut data.supply_structure_cache,
                economy: &mut data.economy,
                route_cache: &mut data.route_cache,
                squad_contexts: &mut data.squad_contexts,
                mapping: &data.mapping,
            };

            if let Some(mission_data) = data.missions.get(entity) {
                let mut mission = mission_data.as_mission_mut();

                let cleanup_mission = match mission.pre_run_mission(&mut system_data, entity) {
                    Ok(()) => false,
                    Err(error) => {
                        info!("Mission pre-run failed, cleaning up. Error: {}", error);

                        true
                    }
                };

                if cleanup_mission {
                    drop(mission);
                    queue_mission_abort(&mut system_data, entity);
                }
            }
        }
    }
}

pub struct RunMissionSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mission_entities: Vec<Entity> = (&data.entities, &data.missions).join().map(|(e, _)| e).collect();

        for entity in mission_entities {
            let mut system_data = MissionExecutionSystemData {
                updater: &data.updater,
                entities: &data.entities,
                room_data: &mut data.room_data,
                room_plan_data: &data.room_plan_data,
                creep_owner: &data.creep_owner,
                creep_spawning: &data.creep_spawning,
                job_data: &data.job_data,
                missions: &data.missions,
                cleanup_queue: &mut data.cleanup_queue,
                spawn_queue: &mut data.spawn_queue,
                room_plan_queue: &mut data.room_plan_queue,
                transfer_queue: &mut data.transfer_queue,
                order_queue: &mut data.order_queue,
                visibility: &mut data.visibility,
                boost_queue: &mut data.boost_queue,
                repair_queue: &mut data.repair_queue,
                supply_structure_cache: &mut data.supply_structure_cache,
                economy: &mut data.economy,
                route_cache: &mut data.route_cache,
                squad_contexts: &mut data.squad_contexts,
                mapping: &data.mapping,
            };

            if let Some(mission_data) = data.missions.get(entity) {
                let mut mission = mission_data.as_mission_mut();

                let cleanup_mission = match mission.run_mission(&mut system_data, entity) {
                    Ok(MissionResult::Running) => false,
                    Ok(MissionResult::Success) => true,
                    Err(error) => {
                        info!("Mission run failed, cleaning up. Error: {}", error);
                        true
                    }
                };

                if cleanup_mission {
                    drop(mission);
                    queue_mission_abort(&mut system_data, entity);
                }
            }
        }
    }
}
