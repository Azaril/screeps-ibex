use super::data::*;
use crate::componentaccess::*;
use crate::creep::*;
use crate::jobs::data::*;
use crate::operations::data::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use crate::room::visibilitysystem::*;
use crate::spawnsystem::*;
use crate::transfer::ordersystem::*;
use crate::transfer::transfersystem::*;
use crate::ui::*;
use crate::visualize::*;
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
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
    transfer_queue: Write<'a, TransferQueue>,
    order_queue: Write<'a, OrderQueue>,
    visibility: Write<'a, VisibilityQueue>,
}

pub struct MissionExecutionSystemData<'a, 'b, 'c: 'b> {
    pub updater: &'b Read<'a, LazyUpdate>,
    pub room_data: &'b mut WriteStorage<'a, RoomData>,
    pub room_plan_data: &'b ReadStorage<'a, RoomPlanData>,
    pub entities: &'b Entities<'a>,
    pub creep_owner: &'b ReadStorage<'a, CreepOwner>,
    pub creep_spawning: &'b ReadStorage<'a, CreepSpawning>,
    pub job_data: &'b WriteStorage<'a, JobData>,
    pub missions: &'b (dyn ComponentAccess<MissionData> + 'c),
    pub mission_requests: &'b mut MissionRequests,
    pub spawn_queue: &'b mut SpawnQueue,
    pub room_plan_queue: &'b mut RoomPlanQueue,
    pub visualizer: Option<&'b mut Visualizer>,
    pub ui: Option<&'b mut UISystem>,
    pub transfer_queue: &'b mut TransferQueue,
    pub order_queue: &'b mut OrderQueue,
    pub visibility: &'b mut Write<'a, VisibilityQueue>,
}

pub struct MissionRequests {
    abort: Vec<Entity>,
}

impl MissionRequests {
    fn new() -> MissionRequests {
        MissionRequests { abort: Vec::new() }
    }

    pub fn abort(&mut self, mission: Entity) {
        self.abort.push(mission);
    }

    fn process(system_data: &mut MissionExecutionSystemData) {
        while let Some(mission_entity) = system_data.mission_requests.abort.pop() {
            if let Some(mission_data) = system_data.missions.get(mission_entity) {
                let mut mission = mission_data.as_mission_mut();

                let owner = mission.get_owner().clone();
                let children = mission.get_children();
                let room = mission.get_room();

                mission.complete(system_data, mission_entity);

                Self::queue_cleanup_mission(&system_data.updater, mission_entity, owner, children, room);
            }
        }
    }

    fn queue_cleanup_mission(updater: &LazyUpdate, mission_entity: Entity, owner: Option<Entity>, children: Vec<Entity>, room: Entity) {
        updater.exec_mut(move |world| {
            if world.entities().is_alive(mission_entity) {

                //
                // Remove mission from room.
                //

                if let Some(room_data) = world.write_storage::<RoomData>().get_mut(room) {
                    room_data.remove_mission(mission_entity);
                }

                //
                // Notify children of termination.
                //

                for child_entity in children {
                    if let Some(operation_data) = world.write_storage::<OperationData>().get_mut(child_entity) {
                        operation_data.as_operation().owner_complete(mission_entity);
                    }

                    if let Some(mission_data) = world.write_storage::<MissionData>().get_mut(child_entity) {
                        mission_data.as_mission_mut().owner_complete(mission_entity);
                    }
                }

                //
                // Notify owner of termination.
                //

                if let Some(owner) = owner {
                    if let Some(operation_data) = world.write_storage::<OperationData>().get_mut(owner) {
                        operation_data.as_operation().child_complete(mission_entity);
                    }

                    if let Some(mission_data) = world.write_storage::<MissionData>().get_mut(owner) {
                        mission_data.as_mission_mut().child_complete(mission_entity);
                    }
                }

                if let Err(err) = world.delete_entity(mission_entity) {
                    warn!("Trying to clean up mission entity that no longer exists. Error: {}", err);
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

    fn describe(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) {
        let description = self.describe_state(system_data, mission_entity);

        if let Some(room_data) = system_data.room_data.get(self.get_room()) {
            if let Some(ui) = system_data.ui.as_deref_mut() {
                if let Some(visualizer) = system_data.visualizer.as_deref_mut() {
                    ui.with_room(room_data.name, visualizer, move |room_ui| {
                        room_ui.missions().add_text(description, None);
                    })
                }
            }
        }
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String;

    fn pre_run_mission(&mut self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String>;

    fn complete(&mut self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) {}
}

pub struct PreRunMissionSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for PreRunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut mission_requests = MissionRequests::new();

        for (entity, mission_data) in (&data.entities, &mut data.missions.restrict_mut()).join() {
            let mut system_data = MissionExecutionSystemData {
                updater: &data.updater,
                entities: &data.entities,
                room_data: &mut data.room_data,
                room_plan_data: &data.room_plan_data,
                creep_owner: &data.creep_owner,
                creep_spawning: &data.creep_spawning,
                job_data: &data.job_data,
                missions: &mission_data,
                mission_requests: &mut mission_requests,
                spawn_queue: &mut data.spawn_queue,
                room_plan_queue: &mut data.room_plan_queue,
                visualizer: data.visualizer.as_deref_mut(),
                ui: data.ui.as_deref_mut(),
                transfer_queue: &mut data.transfer_queue,
                order_queue: &mut data.order_queue,
                visibility: &mut data.visibility,
            };

            {
                let mut mission = mission_data.get_unchecked().as_mission_mut();

                let cleanup_mission = match mission.pre_run_mission(&mut system_data, entity) {
                    Ok(()) => false,
                    Err(error) => {
                        info!("Mission pre-run failed, cleaning up. Error: {}", error);

                        true
                    }
                };

                if cleanup_mission {
                    system_data.mission_requests.abort(entity);
                } else {
                    mission.describe(&mut system_data, entity);
                }
            }

            MissionRequests::process(&mut system_data);
        }
    }
}

pub struct RunMissionSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RunMissionSystem {
    type SystemData = MissionSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let mut mission_requests = MissionRequests::new();

        for (entity, mission_data) in (&data.entities, &mut data.missions.restrict_mut()).join() {
            let mut system_data = MissionExecutionSystemData {
                updater: &data.updater,
                entities: &data.entities,
                room_data: &mut data.room_data,
                room_plan_data: &data.room_plan_data,
                creep_owner: &data.creep_owner,
                creep_spawning: &data.creep_spawning,
                job_data: &data.job_data,
                missions: &mission_data,
                mission_requests: &mut mission_requests,
                spawn_queue: &mut data.spawn_queue,
                room_plan_queue: &mut data.room_plan_queue,
                visualizer: data.visualizer.as_deref_mut(),
                ui: data.ui.as_deref_mut(),
                transfer_queue: &mut data.transfer_queue,
                order_queue: &mut data.order_queue,
                visibility: &mut data.visibility,
            };

            {
                let mut mission = mission_data.get_unchecked().as_mission_mut();

                let cleanup_mission = match mission.run_mission(&mut system_data, entity) {
                    Ok(MissionResult::Running) => false,
                    Ok(MissionResult::Success) => {
                        info!("Mission complete, cleaning up.");
                        true
                    }
                    Err(error) => {
                        info!("Mission run failed, cleaning up. Error: {}", error);
                        true
                    }
                };

                if cleanup_mission {
                    system_data.mission_requests.abort(entity);
                }
            }

            MissionRequests::process(&mut system_data);
        }
    }
}
