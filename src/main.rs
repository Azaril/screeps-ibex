#![recursion_limit = "128"]
#![allow(dead_code)]
#![warn(clippy::all)]

#![cfg_attr(feature = "profile", feature(proc_macro_hygiene))]
#![cfg_attr(feature = "profile", feature(custom_inner_attributes))]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

mod creep;
mod features;
mod findnearest;
mod globals;
mod jobs;
mod logging;
mod entitymappingsystem;
mod memorysystem;
mod missions;
mod operations;
mod remoteobjectid;
mod room;
mod serialize;
mod spawnsystem;
mod statssystem;
mod structureidentifier;
mod transfer;
mod ui;
mod visualize;
mod game_loop;

use log::*;
use stdweb::*;

fn main() {
    stdweb::initialize();

    logging::setup_logging(logging::Info);

    js! {
        var main_loop = @{main_loop};

        module.exports.loop = function() {
            // Provide actual error traces.
            try {
                main_loop();
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

fn main_loop() {
    #[cfg(feature = "profile")]
    {
        screeps_timing::start_trace(|| screeps::game::cpu::get_used());
    }
    
    game_loop::tick();

    #[cfg(feature = "profile")]
    {
        let trace = screeps_timing::stop_trace();

        if let Some(trace_output) = serde_json::to_string(&trace).ok() {
            info!("{}", trace_output);
        }
    }   
}