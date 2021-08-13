use crate::creep::*;
use crate::entitymappingsystem::*;
use crate::globals::*;
use crate::jobs::data::*;
use crate::jobs::jobsystem::*;
use crate::memorysystem::*;
use crate::missions::data::*;
use crate::missions::missionsystem::*;
use crate::operations::data::*;
use crate::operations::managersystem::*;
use crate::operations::operationsystem::*;
use crate::pathing::costmatrixsystem::*;
use crate::pathing::movementsystem::*;
use crate::room::createroomsystem::*;
use crate::room::data::*;
use crate::room::roomplansystem::*;
use crate::room::updateroomsystem::*;
use crate::room::visibilitysystem::*;
use crate::serialize::*;
use crate::spawnsystem::*;
use crate::statssystem::*;
use crate::transfer::ordersystem::*;
use crate::transfer::transfersystem::*;
use crate::ui::*;
use crate::visualize::*;
use bincode::{DefaultOptions, Deserializer, Serializer};
use log::*;
use screeps::*;
use screeps_rover::*;
use specs::{
    prelude::*,
    saveload::{DeserializeComponents, SerializeComponents},
};
use std::{collections::HashSet, error::Error};

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn serialize_world(world: &World, segments: &[u8]) {
    struct Serialize<'a> {
        segments: &'a [u8],
    }

    #[derive(SystemData)]
    struct SerializeSystemData<'a> {
        memory_arbiter: WriteExpect<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_allocator: Write<'a, SerializeMarkerAllocator>,
        markers: ReadStorage<'a, SerializeMarker>,
        creep_spawnings: ReadStorage<'a, CreepSpawning>,
        creep_owners: ReadStorage<'a, CreepOwner>,
        creep_movement_data: ReadStorage<'a, CreepRoverData>,
        room_data: ReadStorage<'a, RoomData>,
        room_plan_data: ReadStorage<'a, RoomPlanData>,
        job_data: ReadStorage<'a, JobData>,
        operation_data: ReadStorage<'a, OperationData>,
        mission_data: ReadStorage<'a, MissionData>,
    }

    impl<'a, 'b> System<'a> for Serialize<'b> {
        type SystemData = SerializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            let mut serialized_data = Vec::<u8>::with_capacity(1024 * 50);

            let mut serializer = Serializer::new(&mut serialized_data, DefaultOptions::new());

            SerializeComponents::<std::convert::Infallible, SerializeMarker>::serialize(
                &(
                    &data.creep_spawnings,
                    &data.creep_owners,
                    &data.creep_movement_data,
                    &data.room_data,
                    &data.room_plan_data,
                    &data.job_data,
                    &data.operation_data,
                    &data.mission_data,
                ),
                &data.entities,
                &data.markers,
                &mut serializer,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
            .unwrap_or_else(|e| error!("Failed serialization: {}", e));

            let encoded_data = encode_buffer_to_string(&serialized_data).unwrap();

            let mut segments = self.segments.iter();

            for chunk in encoded_data.as_bytes().chunks(MEMORY_SEGMENT_SIZE_LIMIT as usize) {
                if let Some(segment) = segments.next() {
                    //
                    // NOTE: This relies on not using multi-byte characters for encoding. (This is valid from base64 encoding.)
                    //
                    let chunk_str = unsafe { std::str::from_utf8_unchecked(chunk) };

                    //TODO: wiarchbe: Fix conversion to owned string...
                    data.memory_arbiter.set(*segment, chunk_str.to_owned());
                } else {
                    error!(
                        "Not enough segments available to store all state. Segment count: {} - Needed segments: {}",
                        self.segments.len(),
                        encoded_data.len() as f32 / MEMORY_SEGMENT_SIZE_LIMIT as f32
                    );
                }
            }

            for segment in segments {
                data.memory_arbiter.set(*segment, "".to_owned());
            }
        }
    }

    let mut sys = Serialize { segments };

    sys.run_now(&world);
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn deserialize_world(world: &World, segments: &[u8]) {
    struct Deserialize<'a> {
        segments: &'a [u8],
    }

    #[derive(SystemData)]
    struct DeserializeSystemData<'a> {
        memory_arbiter: WriteExpect<'a, MemoryArbiter>,
        entities: Entities<'a>,
        marker_alloc: Write<'a, SerializeMarkerAllocator>,
        markers: WriteStorage<'a, SerializeMarker>,
        creep_spawnings: WriteStorage<'a, CreepSpawning>,
        creep_owners: WriteStorage<'a, CreepOwner>,
        creep_movement_data: WriteStorage<'a, CreepRoverData>,
        room_data: WriteStorage<'a, RoomData>,
        room_plan_data: WriteStorage<'a, RoomPlanData>,
        job_data: WriteStorage<'a, JobData>,
        operation_data: WriteStorage<'a, OperationData>,
        mission_data: WriteStorage<'a, MissionData>,
    }

    impl<'a, 'b> System<'a> for Deserialize<'b> {
        type SystemData = DeserializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            //
            // NOTE: System assumes that segment is available and will panic if data is not accesible.
            //

            use itertools::*;

            let encoded_data = self
                .segments
                .iter()
                .filter_map(|segment| data.memory_arbiter.get(*segment))
                .join("");

            if !encoded_data.is_empty() {
                let decoded_data = decode_buffer_from_string(&encoded_data).unwrap_or_else(|_| Vec::new());

                let mut deserializer = Deserializer::from_slice(&decoded_data, DefaultOptions::new());

                DeserializeComponents::<std::convert::Infallible, SerializeMarker>::deserialize(
                    &mut (
                        &mut data.creep_spawnings,
                        &mut data.creep_owners,
                        &mut data.creep_movement_data,
                        &mut data.room_data,
                        &mut data.room_plan_data,
                        &mut data.job_data,
                        &mut data.operation_data,
                        &mut data.mission_data,
                    ),
                    &data.entities,
                    &mut data.markers,
                    &mut data.marker_alloc,
                    &mut deserializer,
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
                .unwrap_or_else(|e| error!("Failed deserialization: {}", e));
            }
        }
    }

    let mut sys = Deserialize { segments };

    sys.run_now(&world);
}

struct GameEnvironment<'a, 'b, 'c, 'd> {
    world: World,
    pre_pass_dispatcher: Dispatcher<'a, 'b>,
    main_pass_dispatcher: Dispatcher<'c, 'd>,
    loaded: bool,
    tick: Option<u32>,
}

static mut ENVIRONMENT: Option<GameEnvironment> = None;

const COST_MATRIX_SYSTEM_SEGMENT: u8 = 55;

fn create_environment<'a, 'b, 'c, 'd>() -> GameEnvironment<'a, 'b, 'c, 'd> {
    crate::features::js::prepare();

    let mut world = World::new();

    world.insert(MemoryArbiter::new());

    world.insert(SerializeMarkerAllocator::new());
    world.register::<SerializeMarker>();

    let cost_matrix_storage = Box::new(CostMatrixStorageInterface);
    let cost_matrix_system = CostMatrixSystem::new(cost_matrix_storage, COST_MATRIX_SYSTEM_SEGMENT);

    world.insert(cost_matrix_system);

    let movement_data = MovementData::<Entity>::new();

    world.insert(movement_data);

    //
    // Pre-pass update
    //

    let mut pre_pass_dispatcher = DispatcherBuilder::new()
        .with(WaitForSpawnSystem, "wait_for_spawn", &[])
        .with(CleanupCreepsSystem, "cleanup_creeps", &[])
        .with(CreateRoomDataSystem, "create_room_data", &[])
        .with(UpdateRoomDataSystem, "update_room_data", &["create_room_data"])
        .with_barrier()
        .with(EntityMappingSystem, "entity_mapping", &[])
        .build();

    pre_pass_dispatcher.setup(&mut world);

    //
    // Main update
    //

    let mut main_pass_dispatcher = DispatcherBuilder::new()
        .with(OperationManagerSystem, "operations_manager", &[])
        .with(PreRunOperationSystem, "pre_run_operations", &[])
        .with(PreRunMissionSystem, "pre_run_missions", &[])
        .with(PreRunJobSystem, "pre_run_jobs", &[])
        .with_barrier()
        .with(RunOperationSystem, "run_operations", &[])
        .with(RunMissionSystem, "run_missions", &[])
        .with(RunJobSystem, "run_jobs", &[])
        .with(MovementUpdateSystem, "movement", &["run_jobs"])
        .with_barrier()
        .with(VisibilityQueueSystem, "visibility_queue", &[])
        .with(SpawnQueueSystem, "spawn_queue", &[])
        .with(TransferQueueUpdateSystem, "transfer_queue", &[])
        .with(OrderQueueSystem, "order_queue", &[])
        .with_barrier()
        .with(RoomPlanSystem, "room_plan", &[])
        .with_barrier()
        .with(VisualizerSystem, "visualizer", &[])
        .with(StatsSystem, "stats", &[])
        .with_barrier()
        .with(CostMatrixStoreSystem, "cost_matrix_store", &[])
        .with_barrier()
        .with(MemoryArbiterSystem, "memory", &[])
        .build();

    main_pass_dispatcher.setup(&mut world);

    GameEnvironment {
        world,
        pre_pass_dispatcher,
        main_pass_dispatcher,
        loaded: false,
        tick: None,
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick() {
    //
    // Deserialize world state.
    //

    let current_time = game::time();

    const COMPONENT_SEGMENTS: &[u8] = &[50, 51, 52];

    if crate::features::reset::reset_memory() {
        info!("Resetting memory");

        let segments = RawMemory::segments();

        for segment_index in COMPONENT_SEGMENTS.iter() {
            segments.set(*segment_index, "".into());
        }
    }    

    let expected_time = unsafe { ENVIRONMENT.as_ref() }
        .and_then(|e| e.tick)
        .map(|t| t + 1);

    let is_expected_tick = expected_time
        .map(|t| t == current_time)
        .unwrap_or(false);

    if !is_expected_tick {
        info!("Unexpected tick time - may have multiple globals. Actual: {} - Expected: {:?}", current_time, expected_time);
    }

    if crate::features::reset::reset_environment() || !is_expected_tick {
        info!("Resetting environment");

        unsafe { ENVIRONMENT = None };
    }

    crate::features::reset::clear();

    let GameEnvironment {
        world,
        pre_pass_dispatcher,
        main_pass_dispatcher,
        loaded,
        tick,
    } = unsafe { ENVIRONMENT.get_or_insert_with(create_environment) };

    *tick = Some(current_time);

    let is_data_ready = {
        let mut memory_arbiter = world.write_resource::<MemoryArbiter>();

        for segment in COMPONENT_SEGMENTS.iter() {
            memory_arbiter.request(*segment);
        }

        //TODO: Remove this load from here.
        memory_arbiter.request(COST_MATRIX_SYSTEM_SEGMENT);

        COMPONENT_SEGMENTS.iter().all(|segment| memory_arbiter.is_active(*segment))
    };

    if !is_data_ready {
        warn!("Component data is not ready, delaying execution");

        MemoryArbiterSystem.run_now(world);

        return;
    }

    //
    // Add dynamic resources.
    //

    if crate::features::visualize::on() {
        world.insert(Visualizer::new());
        world.insert(UISystem::new());
    } else {
        world.remove::<Visualizer>();
        world.remove::<UISystem>();
    }

    //
    // Load state if not yet loaded.
    //

    if !*loaded {
        deserialize_world(&world, COMPONENT_SEGMENTS);

        *loaded = true;
    }

    //
    // Prepare globals
    //

    let username = game::rooms()
        .values()
        .filter_map(|room| {
            if let Some(controller) = room.controller() {
                if controller.my() {
                    if let Some(owner) = controller.owner() {
                        return Some(owner.username());
                    }
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

    main_pass_dispatcher.dispatch(&world);
    world.maintain();

    //
    // Cleanup memory.
    //

    cleanup_memory().expect("expected Memory.creeps format to be a regular memory object");

    //
    // Serialize world state.
    //

    serialize_world(&world, COMPONENT_SEGMENTS);
}

fn cleanup_memory() -> Result<(), Box<dyn (Error)>> {
    let _alive_creeps: HashSet<String> = game::creeps().keys().collect();

    //TODO: wiarchbe: Re-enable.
    // let screeps_memory = match screeps::memory::root().dict("creeps")? {
    //     Some(v) => v,
    //     None => {
    //         return Ok(());
    //     }
    // };

    // for mem_name in screeps_memory.keys() {
    //     if !alive_creeps.contains(&mem_name) {
    //         debug!("cleaning up creep memory of dead creep {}", mem_name);
    //         screeps_memory.del(&mem_name);
    //     }
    // }

    Ok(())
}
