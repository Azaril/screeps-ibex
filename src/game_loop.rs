use log::*;
use screeps::*;
use specs::{
    error::NoError,
    prelude::*,
    saveload::{DeserializeComponents, SerializeComponents},
};
use std::collections::HashSet;
use std::fmt;
use crate::memorysystem::*;
use crate::serialize::*;
use crate::creep::*;
use crate::room::data::*;
use crate::jobs::data::*;
use crate::operations::data::*;
use crate::missions::data::*;
use crate::ui::*;
use crate::visualize::*;
use crate::spawnsystem::*;
use crate::statssystem::*;
use crate::entitymappingsystem::*;
use crate::jobs::jobsystem::*;
use crate::missions::missionsystem::*;
use crate::transfer::transfersystem::*;
use crate::globals::*;
use crate::room::createroomsystem::*;
use crate::room::updateroomsystem::*;
use crate::operations::managersystem::*;
use crate::operations::operationsystem::*;
use crate::room::visibilitysystem::*;
use crate::transfer::ordersystem::*;
use crate::room::roomplansystem::*;
use crate::pathing::movementsystem::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn serialize_world(world: &World, segment: u32) {
    struct Serialize {
        segment: u32,
    }

    #[derive(SystemData)]
    struct SerializeSystemData<'a> {
        memory_arbiter: Write<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_allocator: Write<'a, SerializeMarkerAllocator>,
        markers: WriteStorage<'a, SerializeMarker>,
        creep_spawnings: ReadStorage<'a, CreepSpawning>,
        creep_owners: ReadStorage<'a, CreepOwner>,
        room_data: ReadStorage<'a, RoomData>,
        room_plan_data: ReadStorage<'a, RoomPlanData>,
        job_data: ReadStorage<'a, JobData>,
        operation_data: ReadStorage<'a, OperationData>,
        mission_data: ReadStorage<'a, MissionData>,
    }

    impl<'a> System<'a> for Serialize {
        type SystemData = SerializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            let mut writer = Vec::<u8>::with_capacity(1024 * 100);

            let mut ser = serde_json::ser::Serializer::new(&mut writer);

            SerializeComponents::<NoError, SerializeMarker>::serialize_recursive(
                &(
                    &data.creep_spawnings,
                    &data.creep_owners,
                    &data.room_data,
                    &data.room_plan_data,
                    &data.job_data,
                    &data.operation_data,
                    &data.mission_data,
                ),
                &data.entities,
                &mut data.markers,
                &mut data.marker_allocator,
                &mut ser,
            )
            .unwrap_or_else(|e| error!("Error: {}", e));

            let world_data = unsafe { std::str::from_utf8_unchecked(&writer) };

            data.memory_arbiter.set(self.segment, world_data);
        }
    }

    let mut sys = Serialize { segment };

    sys.run_now(&world);
}

#[derive(Debug)]
enum CombinedSerialiationError {
    SerdeJson(serde_json::error::Error),
}

impl fmt::Display for CombinedSerialiationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CombinedSerialiationError::SerdeJson(ref e) => write!(f, "{}", e),
        }
    }
}

impl From<serde_json::error::Error> for CombinedSerialiationError {
    fn from(x: serde_json::error::Error) -> Self {
        CombinedSerialiationError::SerdeJson(x)
    }
}

impl From<NoError> for CombinedSerialiationError {
    fn from(e: NoError) -> Self {
        match e {}
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn deserialize_world(world: &World, segment: u32) {
    struct Deserialize {
        segment: u32,
    }

    #[derive(SystemData)]
    struct DeserializeSystemData<'a> {
        memory_arbiter: Write<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_alloc: Write<'a, SerializeMarkerAllocator>,
        markers: WriteStorage<'a, SerializeMarker>,
        creep_spawnings: WriteStorage<'a, CreepSpawning>,
        creep_owners: WriteStorage<'a, CreepOwner>,
        room_data: WriteStorage<'a, RoomData>,
        room_plan_data: WriteStorage<'a, RoomPlanData>,
        job_data: WriteStorage<'a, JobData>,
        operation_data: WriteStorage<'a, OperationData>,
        mission_data: WriteStorage<'a, MissionData>,
    }
    
    impl<'a> System<'a> for Deserialize {
        type SystemData = DeserializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            //
            // NOTE: System assumes that segment is available and will panic if data is not accesible.
            //

            let world_data = data.memory_arbiter.get(self.segment);

            if let Some(world_data) = world_data {
                if !world_data.is_empty() {
                    let mut de = serde_json::de::Deserializer::from_str(&world_data);

                    DeserializeComponents::<CombinedSerialiationError, SerializeMarker>::deserialize(
                        &mut (
                            data.creep_spawnings,
                            data.creep_owners,
                            data.room_data,
                            data.room_plan_data,
                            data.job_data,
                            data.operation_data,
                            data.mission_data,
                        ),
                        &data.entities,
                        &mut data.markers,
                        &mut data.marker_alloc,
                        &mut de,
                    )
                    .unwrap_or_else(|e| error!("Error: {}", e));
                }
            } else {
                panic!("Failed to get world data from segment that was expected to be active.");
            }
        }
    }

    let mut sys = Deserialize { segment };

    sys.run_now(&world);
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick() {
    crate::features::js::prepare();

    let mut world = World::new();

    let mut memory_arbiter = MemoryArbiter::default();

    let component_segment = 50;

    memory_arbiter.request(component_segment);

    if !memory_arbiter.is_active(component_segment) {
        world.insert(memory_arbiter);

        let mut memory_system = MemoryArbiterSystem {};

        memory_system.run_now(&world);

        return;
    } else {
        world.insert(memory_arbiter);
    }

    world.insert(SerializeMarkerAllocator::new());
    world.register::<SerializeMarker>();

    if crate::features::visualize::on() {
        world.insert(Visualizer::new());
        world.insert(UISystem::new());
    }

    //
    // Pre-pass update
    //

    let mut pre_pass_dispatcher = DispatcherBuilder::new()
        .with(WaitForSpawnSystem, "wait_for_spawn", &[])
        .with(CleanupCreepsSystem, "cleanup_creeps", &[])
        .with(CreateRoomDataSystem, "create_room_data", &[])
        .with(UpdateRoomDataSystem, "update_room_data", &["create_room_data"])
        .with(EntityMappingSystem, "entity_mapping", &["create_room_data"])
        .build();

    pre_pass_dispatcher.setup(&mut world);

    //
    // Main update
    //

    let mut main_dispatcher = DispatcherBuilder::new()
        .with(OperationManagerSystem, "operations_manager", &[])
        .with(PreRunOperationSystem, "pre_run_operations", &[])
        .with(PreRunMissionSystem, "pre_run_missions", &[])
        .with(PreRunJobSystem, "pre_run_jobs", &[])
        .with_barrier()
        .with(RunOperationSystem, "run_operations", &[])
        .with(RunMissionSystem, "run_missions", &[])
        .with(RunJobSystem, "run_jobs", &[])
        .with(MovementSystem, "movement", &["run_jobs"])
        .with_barrier()
        .with(VisibilityQueueSystem, "visibility_queue", &[])
        .with(SpawnQueueSystem, "spawn_queue", &[])
        .with(TransferQueueSystem, "transfer_queue", &[])
        .with(OrderQueueSystem, "order_queue", &[])
        .with_barrier()
        .with(RoomPlanSystem, "room_plan", &[])
        .with_barrier()
        .with(VisualizerSystem, "visualizer", &[])
        .with(StatsSystem, "stats", &[])
        .with(MemoryArbiterSystem, "memory", &[])
        .build();

    main_dispatcher.setup(&mut world);

    //
    // Deserialize world state.
    //

    deserialize_world(&world, component_segment);

    //
    // Prepare globals
    //

    let username = game::rooms::values()
        .iter()
        .filter_map(|room| {
            if let Some(controller) = room.controller() {
                if controller.my() {
                    return controller.owner_name();
                }
            }
            None
        })
        .next();

    if let Some(username) = username {
        user::set_name(&username);
    }

    //
    // Execution
    //

    pre_pass_dispatcher.dispatch(&world);
    world.maintain();

    main_dispatcher.dispatch(&world);
    world.maintain();

    //
    // Cleanup memory.
    //

    cleanup_memory().expect("expected Memory.creeps format to be a regular memory object");

    //
    // Serialize world state.
    //

    serialize_world(&world, component_segment);
}

fn cleanup_memory() -> Result<(), Box<dyn (::std::error::Error)>> {
    let alive_creeps: HashSet<String> = screeps::game::creeps::keys().into_iter().collect();

    let screeps_memory = match screeps::memory::root().dict("creeps")? {
        Some(v) => v,
        None => {
            warn!("not cleaning game creep memory: no Memory.creeps dict");
            return Ok(());
        }
    };

    for mem_name in screeps_memory.keys() {
        if !alive_creeps.contains(&mem_name) {
            debug!("cleaning up creep memory of dead creep {}", mem_name);
            screeps_memory.del(&mem_name);
        }
    }

    Ok(())
}