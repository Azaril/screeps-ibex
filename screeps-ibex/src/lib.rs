#![recursion_limit = "256"]
#![allow(dead_code)]
#![warn(clippy::all)]

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: talc::TalckWasm = unsafe { talc::TalckWasm::new_global() };

mod constants;
mod creep;
mod entitymappingsystem;
mod features;
mod findnearest;
mod game_loop;
mod globals;
mod jobs;
mod logging;
mod memory_helper;
mod memorysystem;
mod missions;
mod operations;
mod panic;
mod pathing;
mod remoteobjectid;
mod room;
mod serialize;
mod spawnsystem;
mod stats_history;
mod statssystem;
mod store;
mod structureidentifier;
mod transfer;
mod ui;
mod visualization;
mod visualize;

use log::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = setup)]
pub fn setup() {
    logging::setup_logging(logging::Info);
    panic::setup_panic_hook();
}

#[wasm_bindgen(js_name = game_loop)]
pub fn game_loop_export() {
    main_loop();
}

fn main_loop() {
    #[cfg(feature = "profile")]
    {
        screeps_timing::start_trace(Box::new(|| (screeps::game::cpu::get_used() * 1000.0) as u64));
    }

    game_loop::tick();

    #[cfg(feature = "profile")]
    {
        let trace = screeps_timing::stop_trace();

        let used_cpu = screeps::game::cpu::get_used();

        if used_cpu >= 18.0 {
            warn!("Long tick: {}", used_cpu);

            if let Some(trace_output) = serde_json::to_string(&trace).ok() {
                info!("{}", trace_output);
            }
        }
    }
}
