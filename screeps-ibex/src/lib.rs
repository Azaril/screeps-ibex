#![recursion_limit = "256"]
#![allow(dead_code)]
#![warn(clippy::all)]

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: talc::TalckWasm = unsafe { talc::TalckWasm::new_global() };

mod cleanup;
// The JS-free tactical seam + pure combat decisions (ADR 0006 §B.2 / S17) live in their own member
// crate `screeps-combat-decision`, so the host-side sim (`screeps-combat-agent`) depends on that
// tiny crate instead of the whole bot. Re-exported as `crate::combat` so the live adapters in
// `jobs::squad_combat` / `missions::attack_mission` (the only `game::*` users) keep their paths.
pub use screeps_combat_decision as combat;
mod constants;
mod cpugovernor;
mod creep;
mod entitymappingsystem;
mod expansion;
mod features;
mod findnearest;
mod game_loop;
mod gameview;
mod identity;
mod intents;
mod jobs;
mod logging;
mod machine_tick;
mod memory_helper;
mod memorysystem;
mod metrics;
mod military;
mod missions;
mod operations;
mod panic;
mod pathing;
mod remoteobjectid;
mod repairqueue;
mod room;
mod segments;
mod serialize;
mod spawnsystem;
mod stats_history;
mod statssystem;
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
