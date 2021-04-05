"use strict";

function console_error(...args) {
    console.log(...args);
    Game.notify(args.join(' '));
}

let wasm_module = null;
let initialized = false;

function wasm_reset() {
    //wasm_module = null;
    initialized = false;

    module.exports.loop = wasm_initialize;
}

function wasm_initialize() {
    try {    
        if (Game.cpu.bucket < 500 || Game.cpu.getUsed() > 100) {
            return;
        }

        if (wasm_module == null) {
            console.log("Reset!");

            wasm_module = require("screeps-starter-rust");
        }

        if (Game.cpu.bucket < 500 || Game.cpu.getUsed() > 100) {
            return;
        }    

        if (!initialized) {
            wasm_module.initialize_instance();

            wasm_module.setup();

            initialized = true;
        }
        
        if (Game.cpu.bucket < 500 || Game.cpu.getUsed() > 100) {
            return;
        }

        module.exports.tick();
    } catch(error) {
        console_error("Caught exception:", error);
        if (error.stack) {
            console_error("Stack trace:", error.stack);
        }
        console_error("Resetting VM next tick.");
        
        module.exports.loop = wasm_reset;
    }
}

module.exports.loop = wasm_initialize;