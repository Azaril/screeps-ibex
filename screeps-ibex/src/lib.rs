#![recursion_limit = "256"]
#![allow(dead_code)]
#![warn(clippy::all)]
#![feature(const_fn)]

#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

mod componentaccess;
mod constants;
mod creep;
mod entitymappingsystem;
mod features;
mod findnearest;
mod game_loop;
mod globals;
mod jobs;
mod logging;
mod memorysystem;
mod missions;
mod operations;
mod pathing;
mod remoteobjectid;
mod room;
mod serialize;
mod spawnsystem;
mod statssystem;
mod store;
mod structureidentifier;
mod transfer;
mod ui;
mod visualize;

use std::panic;

use log::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn setup() {
    logging::setup_logging(logging::Info);

    panic::set_hook(Box::new(console_error_panic_hook::hook));
}

#[wasm_bindgen]
pub fn tick() {
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
