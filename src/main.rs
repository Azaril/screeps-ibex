#![recursion_limit = "128"]
#![allow(dead_code)]
#![warn(clippy::all)]
#![feature(proc_macro_hygiene)]

#[cfg_attr(feature = "profile", timing)]
mod creep;
#[cfg_attr(feature = "profile", timing)]
mod features;
#[cfg_attr(feature = "profile", timing)]
mod findnearest;
#[cfg_attr(feature = "profile", timing)]
mod globals;
#[cfg_attr(feature = "profile", timing)]
mod jobs;
#[cfg_attr(feature = "profile", timing)]
mod logging;
#[cfg_attr(feature = "profile", timing)]
mod mappingsystem;
#[cfg_attr(feature = "profile", timing)]
mod memorysystem;
#[cfg_attr(feature = "profile", timing)]
mod missions;
#[cfg_attr(feature = "profile", timing)]
mod operations;
#[cfg_attr(feature = "profile", timing)]
mod remoteobjectid;
#[cfg_attr(feature = "profile", timing)]
mod room;
#[cfg_attr(feature = "profile", timing)]
mod serialize;
#[cfg_attr(feature = "profile", timing)]
mod spawnsystem;
#[cfg_attr(feature = "profile", timing)]
mod statssystem;
#[cfg_attr(feature = "profile", timing)]
mod structureidentifier;
#[cfg_attr(feature = "profile", timing)]
mod transfer;
#[cfg_attr(feature = "profile", timing)]
mod ui;
#[cfg_attr(feature = "profile", timing)]
mod visualize;

use log::*;
use screeps::*;
use specs::{
    error::NoError,
    prelude::*,
    saveload::{DeserializeComponents, SerializeComponents},
};
use std::collections::HashSet;
use std::fmt;
#[cfg(feature = "profile")]
use timing_annotate::*;
use stdweb::*;

fn main() {
    stdweb::initialize();

    logging::setup_logging(logging::Info);

    js! {
        var game_loop = @{game_loop};

        module.exports.loop = function() {
            // Provide actual error traces.
            try {
                game_loop();
            } catch (error) {
                // console_error function provided by 'screeps-game-api'
                console_error("caught exception:", error);
                if (error.stack) {
                    console_error("stack trace:", error.stack);
                }
                console_error("resetting VM next tick.");
                // reset the VM since we don't know if everything was cleaned up and don't
                // want an inconsistent state.
                module.exports.loop = wasm_reset;
                //TODO: Halting here seems to cause more problems than it solves.
            }
        }
    }
}

fn serialize_world(world: &World, segment: u32) {
    struct Serialize {
        segment: u32,
    }

    #[derive(SystemData)]
    struct SerializeSystemData<'a> {
        memory_arbiter: Write<'a, memorysystem::MemoryArbiter>,
        entities: Entities<'a>,
        marker_allocator: Write<'a, serialize::SerializeMarkerAllocator>,
        markers: WriteStorage<'a, serialize::SerializeMarker>,
        creep_spawnings: ReadStorage<'a, creep::CreepSpawning>,
        creep_owners: ReadStorage<'a, creep::CreepOwner>,
        room_data: ReadStorage<'a, room::data::RoomData>,
        job_data: ReadStorage<'a, jobs::data::JobData>,
        operation_data: ReadStorage<'a, operations::data::OperationData>,
        mission_data: ReadStorage<'a, missions::data::MissionData>,
    }

    impl<'a> System<'a> for Serialize {
        type SystemData = SerializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            let mut writer = Vec::<u8>::with_capacity(1024 * 100);

            let mut ser = serde_json::ser::Serializer::new(&mut writer);

            SerializeComponents::<NoError, serialize::SerializeMarker>::serialize_recursive(
                &(
                    &data.creep_spawnings,
                    &data.creep_owners,
                    &data.room_data,
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

fn deserialize_world(world: &World, segment: u32) {
    struct Deserialize {
        segment: u32,
    }

    #[derive(SystemData)]
    struct DeserializeSystemData<'a> {
        memory_arbiter: Write<'a, memorysystem::MemoryArbiter>,
        entities: Entities<'a>,
        marker_alloc: Write<'a, serialize::SerializeMarkerAllocator>,
        markers: WriteStorage<'a, serialize::SerializeMarker>,
        creep_spawnings: WriteStorage<'a, creep::CreepSpawning>,
        creep_owners: WriteStorage<'a, creep::CreepOwner>,
        room_data: WriteStorage<'a, room::data::RoomData>,
        job_data: WriteStorage<'a, jobs::data::JobData>,
        operation_data: WriteStorage<'a, operations::data::OperationData>,
        mission_data: WriteStorage<'a, missions::data::MissionData>,
    }

    impl<'a> System<'a> for Deserialize {
        type SystemData = DeserializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            //
            // NOTE: System assumes that segment is available and will panic if data is not accesible.
            //

            let world_data = data.memory_arbiter.get(self.segment).unwrap();

            if !world_data.is_empty() {
                let mut de = serde_json::de::Deserializer::from_str(&world_data);

                DeserializeComponents::<CombinedSerialiationError, serialize::SerializeMarker>::deserialize(
                    &mut (
                        data.creep_spawnings,
                        data.creep_owners,
                        data.room_data,
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
        }
    }

    let mut sys = Deserialize { segment };

    sys.run_now(&world);
}

fn game_loop() {
    #[cfg(feature = "profile")]
    {
        timing::start_trace();
    }
    
    game_loop_internal();

    #[cfg(feature = "profile")]
    {
        let trace = timing::stop_trace();

        if let Some(trace_output) = serde_json::to_string(&trace).ok() {
            info!("{}", trace_output);
        }
    }   
}

fn game_loop_internal() {
    features::js::prepare();

    let mut world = World::new();

    let mut memory_arbiter = memorysystem::MemoryArbiter::default();

    let component_segment = 50;

    memory_arbiter.request(component_segment);

    if !memory_arbiter.is_active(component_segment) {
        world.insert(memory_arbiter);

        let mut memory_system = memorysystem::MemoryArbiterSystem {};

        memory_system.run_now(&world);

        return;
    } else {
        world.insert(memory_arbiter);
    }

    world.insert(serialize::SerializeMarkerAllocator::new());
    world.register::<serialize::SerializeMarker>();

    if features::visualize::on() {
        world.insert(visualize::Visualizer::new());
        world.insert(ui::UISystem::new());
    }

    //
    // Pre-pass update
    //

    let mut pre_pass_dispatcher = DispatcherBuilder::new()
        .with(creep::WaitForSpawnSystem, "wait_for_spawn", &[])
        .with(creep::CleanupCreepsSystem, "cleanup_creeps", &[])
        .with(room::createroomsystem::CreateRoomDataSystem, "create_room_data", &[])
        .with(
            room::updateroomsystem::UpdateRoomDataSystem,
            "update_room_data",
            &["create_room_data"],
        )
        .with(mappingsystem::MappingSystem, "mapping", &["create_room_data"])
        .build();

    pre_pass_dispatcher.setup(&mut world);

    //
    // Main update
    //

    let mut main_dispatcher = DispatcherBuilder::new()
        .with(operations::managersystem::OperationManagerSystem, "operations_manager", &[])
        .with(operations::operationsystem::PreRunOperationSystem, "pre_run_operations", &[])
        .with(missions::missionsystem::PreRunMissionSystem, "pre_run_missions", &[])
        .with(jobs::jobsystem::PreRunJobSystem, "pre_run_jobs", &[])
        .with_barrier()
        .with(operations::operationsystem::RunOperationSystem, "run_operations", &[])
        .with(missions::missionsystem::RunMissionSystem, "run_missions", &[])
        .with(jobs::jobsystem::RunJobSystem, "run_jobs", &[])
        .with_barrier()
        .with(room::visibilitysystem::VisibilityQueueSystem, "visibility_queue", &[])
        .with(spawnsystem::SpawnQueueSystem, "spawn_queue", &[])
        .with(transfer::transfersystem::TransferQueueSystem, "transfer_queue", &[])
        .with(transfer::ordersystem::OrderQueueSystem, "order_queue", &[])
        .with_barrier()
        .with(visualize::VisualizerSystem, "visualizer", &[])
        .with(statssystem::StatsSystem, "stats", &[])
        .with(memorysystem::MemoryArbiterSystem, "memory", &[])
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
        globals::user::set_name(&username);
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
