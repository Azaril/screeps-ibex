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
use std::cell::RefCell;
use std::collections::HashSet;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn serialize_world(world: &World, segments: &[u32]) {
    struct Serialize<'a> {
        segments: &'a [u32],
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

            let encoded_data = match encode_buffer_to_string(&serialized_data) {
                Ok(s) => s,
                Err(e) => {
                    error!("Encode failed: {}", e);
                    return;
                }
            };

            let mut segments = self.segments.iter();

            for chunk in encoded_data.as_bytes().chunks(1024 * 50) {
                if let Some(segment) = segments.next() {
                    //
                    // NOTE: This relies on not using multi-byte characters for encoding. (This is valid from base64 encoding.)
                    //
                    let chunk_str = unsafe { std::str::from_utf8_unchecked(chunk) };

                    data.memory_arbiter.set(*segment, chunk_str);
                } else {
                    error!(
                        "Not enough segments available to store all state. Segment count: {} - Needed segments: {}",
                        self.segments.len(),
                        encoded_data.len() as f32 / (1024.0 * 50.0)
                    );
                }
            }

            for segment in segments {
                data.memory_arbiter.set(*segment, "");
            }
        }
    }

    let mut sys = Serialize { segments };

    sys.run_now(world);
}

/// Loads world state from RawMemory segments. On deserialization failure we log and continue;
/// the only supported recovery is a full reset (environment and optionally memory via reset flags).
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
fn deserialize_world(world: &World, segments: &[u32]) {
    struct Deserialize<'a> {
        segments: &'a [u32],
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

    sys.run_now(world);
}

struct GameEnvironment<'a, 'b, 'c, 'd> {
    world: World,
    pre_pass_dispatcher: Dispatcher<'a, 'b>,
    main_pass_dispatcher: Dispatcher<'c, 'd>,
    loaded: bool,
    tick: Option<u32>,
}

thread_local! {
    static ENVIRONMENT: RefCell<Option<GameEnvironment<'static, 'static, 'static, 'static>>> = const { RefCell::new(None) };
}

const COST_MATRIX_SYSTEM_SEGMENT: u32 = 55;

fn create_environment<'a, 'b, 'c, 'd>() -> GameEnvironment<'a, 'b, 'c, 'd> {
    info!("Initializing game environment");

    crate::features::prepare();

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
    // Handle reset flags (before feature cache or environment are touched).
    //

    let reset = crate::features::load_reset();

    //
    // Deserialize world state.
    //

    let current_time = game::time();

    const COMPONENT_SEGMENTS: &[u32] = &[50, 51, 52];

    let should_reset = reset.environment
        || ENVIRONMENT.with(|e| {
            e.borrow()
                .as_ref()
                .and_then(|env| env.tick)
                .map(|t| t + 1 != current_time)
                .unwrap_or(false)
        });

    if should_reset {
        info!("Resetting environment");
        ENVIRONMENT.with(|e| *e.borrow_mut() = None);
    }

    if reset.memory {
        info!("Resetting memory");

        for segment in COMPONENT_SEGMENTS.iter() {
            raw_memory::segments().set(*segment as u8, String::new());
        }
    }

    crate::features::clear_reset();

    //
    // Load feature flags from Memory into per-tick cache
    // (after resets, so the cache reflects any prepare() defaults).
    //

    crate::features::load();
    let features = crate::features::features();

    ENVIRONMENT.with(|env_cell| {
        let mut env_ref = env_cell.borrow_mut();
        let env = env_ref.get_or_insert_with(create_environment);

        let is_data_ready = {
            let mut memory_arbiter = env.world.write_resource::<MemoryArbiter>();

            for segment in COMPONENT_SEGMENTS.iter() {
                memory_arbiter.request(*segment);
            }

            //TODO: Remove this load from here.
            memory_arbiter.request(COST_MATRIX_SYSTEM_SEGMENT);

            COMPONENT_SEGMENTS.iter().all(|segment| memory_arbiter.is_active(*segment))
        };

        if !is_data_ready {
            info!("Component data is not ready, delaying execution");

            MemoryArbiterSystem.run_now(&env.world);

            return;
        }

        //
        // Add dynamic resources.
        //

        if features.visualize.on {
            env.world.insert(Visualizer::new());
            env.world.insert(UISystem::new());
        } else {
            env.world.remove::<Visualizer>();
            env.world.remove::<UISystem>();
        }

        if !env.loaded {
            info!("Deserializing world state to environment");

            deserialize_world(&env.world, COMPONENT_SEGMENTS);

            env.loaded = true;
        }

        env.tick = Some(current_time);

        //
        // Prepare globals
        //

        let username = game::rooms().values()
            .filter_map(|room| {
                if let Some(controller) = room.controller() {
                    if controller.my() {
                        return controller.owner().map(|o| o.username());
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

        env.pre_pass_dispatcher.dispatch(&env.world);
        env.world.maintain();

        env.main_pass_dispatcher.dispatch(&env.world);
        env.world.maintain();

        //
        // Cleanup memory.
        //

        if let Err(e) = cleanup_memory() {
            warn!("cleanup_memory: {}", e);
        }

        //
        // Serialize world state.
        //

        serialize_world(&env.world, COMPONENT_SEGMENTS);
    });
}

fn cleanup_memory() -> Result<(), Box<dyn ::std::error::Error>> {
    let alive_creeps: HashSet<String> = screeps::game::creeps()
        .keys()
        .map(|k| k.to_string())
        .collect();

    let screeps_memory = match crate::memory_helper::dict("creeps") {
        Some(v) => v,
        None => {
            return Ok(());
        }
    };

    for mem_name in crate::memory_helper::keys(&screeps_memory) {
        if !alive_creeps.contains(&mem_name) {
            debug!("cleaning up creep memory of dead creep {}", mem_name);
            crate::memory_helper::del(&screeps_memory, &mem_name);
        }
    }

    Ok(())
}
