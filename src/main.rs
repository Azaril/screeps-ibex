#![recursion_limit = "128"]
#![allow(dead_code)]
#![warn(clippy::all)]

extern crate fern;
#[macro_use]
extern crate log;
extern crate screeps;
#[macro_use]
extern crate stdweb;

extern crate serde;

extern crate specs;
extern crate specs_derive;

extern crate itertools;

extern crate crossbeam_queue;

#[macro_use]
mod timing;
mod creep;
mod findnearest;
mod jobs;
mod logging;
mod missions;
mod operations;
mod remoteobjectid;
mod room;
mod serialize;
mod spawnsystem;
mod structureidentifier;
mod mappingsystem;

use std::fmt;

use std::collections::HashSet;

#[allow(unused_imports)]
use screeps::*;
#[allow(unused_imports)]
use specs::{
    error::NoError,
    prelude::*,
    saveload::{
        DeserializeComponents, MarkedBuilder, SerializeComponents, SimpleMarker,
        SimpleMarkerAllocator,
    },
};

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
                module.exports.loop = wasm_initialize;
            }
        }
    }
}

fn serialize_world(world: &World, cb: fn(&str)) {
    scope_timing!("serialize_world");

    struct Serialize {
        writer: Vec<u8>,
    }

    #[derive(SystemData)]
    struct SerializeSystemData<'a> {
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
            let mut ser = serde_json::ser::Serializer::new(&mut self.writer);

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
        }
    }

    let mut sys = Serialize {
        writer: Vec::<u8>::with_capacity(1024 * 16),
    };

    sys.run_now(&world);

    let data = unsafe { std::str::from_utf8_unchecked(&sys.writer) };

    cb(&data);
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

fn deserialize_world(world: &World, data: &str) {
    scope_timing!("deserialize_world");

    struct Deserialize<'a> {
        raw_data: &'a str,
    }

    #[derive(SystemData)]
    struct DeserializeSystemData<'a> {
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

    impl<'a> System<'a> for Deserialize<'a> {
        type SystemData = DeserializeSystemData<'a>;

        fn run(&mut self, mut data: Self::SystemData) {
            let mut de = serde_json::de::Deserializer::from_str(self.raw_data);

            DeserializeComponents::<CombinedSerialiationError, serialize::SerializeMarker>::deserialize(
                &mut (data.creep_spawnings, data.creep_owners, data.room_data, data.job_data, data.operation_data, data.mission_data),
                &data.entities,
                &mut data.markers,
                &mut data.marker_alloc,
                &mut de
            )
            .unwrap_or_else(|e| eprintln!("Error: {}", e));
        }
    }

    let mut sys = Deserialize { raw_data: data };

    sys.run_now(&world);
}

fn game_loop() {
    scope_timing!("Main tick");

    info!("Tick start - CPU: {}", screeps::game::cpu::get_used());

    let mut world = World::new();

    world.insert(serialize::SerializeMarkerAllocator::new());
    world.register::<serialize::SerializeMarker>();

    //
    // Pre-pass update
    //

    let mut pre_pass_dispatcher = DispatcherBuilder::new()
        .with(creep::WaitForSpawnSystem, "wait_for_spawn", &[])
        .with(creep::CleanupCreepsSystem, "cleanup_creeps", &[])
        .with(room::createroomsystem::CreateRoomDataSystem, "create_room_data", &[])
        .with(room::updateroomsystem::UpdateRoomDataSystem, "update_room_data", &[])
        .with(mappingsystem::MappingSystem, "mapping", &[])
        .build();

    pre_pass_dispatcher.setup(&mut world);

    //
    // Main update
    //

    let mut main_dispatcher = DispatcherBuilder::new()
        .with(
            operations::managersystem::OperationManagerSystem,
            "operations_manager",
            &[],
        )
        .with(
            operations::operationsystem::OperationSystem,
            "operations",
            &[],
        )
        .with(missions::missionsystem::MissionSystem, "missions", &[])
        .with(jobs::jobsystem::JobSystem, "jobs", &[])
        .with(room::visibilitysystem::VisibilityQueueSystem, "visibility", &[])
        .with(spawnsystem::SpawnQueueSystem, "spawn_queue", &[])
        .build();

    main_dispatcher.setup(&mut world);

    //
    // Deserialize world state.
    //

    //TODO: Use a memory segment and raw memory to avoid extra string serialize.
    if let Ok(entry) = memory::root().string("native") {
        if let Some(data) = entry {
            deserialize_world(&world, &data);
        }
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

    serialize_world(&world, |data| {
        //TODO: Use a memory segment and raw memory to avoid extra string serialize.
        memory::root().set("native", data);
    });

    info!("Tick end - CPU: {}", screeps::game::cpu::get_used());
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
