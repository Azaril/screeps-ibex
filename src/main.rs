#![recursion_limit = "128"]
extern crate fern;
#[macro_use]
extern crate log;
extern crate screeps;
#[macro_use]
extern crate stdweb;

//#[macro_use]
extern crate serde;
extern crate specs;

mod logging;
mod creep;
mod jobs;

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
    struct Serialize {
        writer: Vec<u8>
    }

    impl<'a> System<'a> for Serialize {
        type SystemData = (
            Entities<'a>,
            ReadStorage<'a, SimpleMarker<creep::CreepMarker>>,
            ReadStorage<'a, creep::CreepSpawning>,
            ReadStorage<'a, creep::CreepOwner>,
            ReadStorage<'a, jobs::data::JobData>
        );

        fn run(&mut self, (entities, markers, spawnings, owners, jobs): Self::SystemData) {
            let mut ser = serde_json::ser::Serializer::new(&mut self.writer);

            SerializeComponents::<NoError, SimpleMarker<creep::CreepMarker>>::serialize(
                &(&spawnings, &owners, &jobs),
                &entities,
                &markers,
                &mut ser,
            ).unwrap_or_else(|e| error!("Error: {}", e));
        }
    }

    let mut sys = Serialize{ writer: Vec::<u8>::with_capacity(2048) };

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
    struct Deserialize<'a> {
        data: &'a str
    }

    impl<'a> System<'a> for Deserialize<'a> {
        type SystemData = (
            Entities<'a>,
            Write<'a, SimpleMarkerAllocator<creep::CreepMarker>>,
            WriteStorage<'a, SimpleMarker<creep::CreepMarker>>,
            WriteStorage<'a, creep::CreepSpawning>,
            WriteStorage<'a, creep::CreepOwner>,
            WriteStorage<'a, jobs::data::JobData>
        );

        fn run(&mut self, (entities, mut alloc, mut markers, spawnings, owners, jobs): Self::SystemData) {
            let mut de = serde_json::de::Deserializer::from_str(self.data);

            DeserializeComponents::<CombinedSerialiationError, SimpleMarker<creep::CreepMarker>>::deserialize(
                &mut (spawnings, owners, jobs),
                &entities,
                &mut markers,
                &mut alloc,
                &mut de
            )
            .unwrap_or_else(|e| eprintln!("Error: {}", e));
        }
    }

    let mut sys = Deserialize{ data: data };

    sys.run_now(&world);
}

fn game_loop() {
    info!("loop starting! CPU: {}", screeps::game::cpu::get_used());

    let mut world = World::new();

    world.register::<SimpleMarker<creep::CreepMarker>>();
    world.insert(SimpleMarkerAllocator::<creep::CreepMarker>::new());

    //TODO: wiarchbe: Is there a better way to not have to pre-register these? (Maybe create dispatcher systems earlier?)
    world.register::<creep::CreepSpawning>();
    world.register::<creep::CreepOwner>();
    world.register::<jobs::data::JobData>();

    //
    // Deserialize world state.
    //

    {
        if let Ok(entry) = memory::root().string("native") {
            if let Some(data) = entry {
                deserialize_world(&world, &data);
            }
        }
    }

    //
    // Pre-pass
    //

    {
        let mut dispatcher = DispatcherBuilder::new()
            .with(creep::WaitForSpawnSystem, "wait_for_spawn", &[])
            .build();

        dispatcher.setup(&mut world);

        dispatcher.dispatch(&world);

        world.maintain();
    }

    //
    // Main update
    //

    {
        let mut dispatcher = DispatcherBuilder::new()
            .with(jobs::system::JobSystem, "jobs", &[])
            .build();

        dispatcher.setup(&mut world);

        dispatcher.dispatch(&world);

        world.maintain();
    } 

    debug!("running spawns");

    for spawn in screeps::game::spawns::values() {
        debug!("running spawn {}", spawn.name());
        let body = [Part::Move, Part::Move, Part::Carry, Part::Work];

        if spawn.energy() >= body.iter().map(|p| p.cost()).sum() {
            let time = screeps::game::time();
            let mut additional = 0;
            let (res, name) = loop {
                let name = format!("{}-{}", time, additional);
                let res = spawn.spawn_creep(&body, &name);

                if res == ReturnCode::NameExists {
                    additional += 1;
                } else {
                    break (res, name);
                }
            };

            if res != ReturnCode::Ok {
                warn!("couldn't spawn: {:?}", res);
            } else {
                creep::create_spawning_creep_entity(&mut world, &name);
            }
        }
    }

    /*
    for creep in screeps::game::creeps::values() {
        let name = creep.name();

        debug!("running creep {}", name);
        
        if creep.spawning() {
            continue;
        }

        let source = &creep.room().find(find::SOURCES)[0];

        let entity = ::creep::create_creep_entity(&mut world, &creep);
    }
    */

    let time = screeps::game::time();

    if time % 32 == 3 {
        info!("Running memory cleanup");

        cleanup_memory().expect("expected Memory.creeps format to be a regular memory object");
    }

    //
    // Serialize world state.
    //

    serialize_world(&world, |data| {
        memory::root().set("native", data);
    });

    info!("done! cpu: {}", screeps::game::cpu::get_used())
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
