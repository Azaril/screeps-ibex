"use strict";

function console_error(...args) {
    console.log(...args);
    Game.notify(args.join(' '));
}

let wasm_module = null;
let initialized = false;

function wasm_reset() {
    wasm_module = null;
    initialized = false;
}

module.exports.loop = function() {
    try {    
        // GIANT HACK TO FIX TIME
        Game._time = Game.time;
        Game.time = function() { return Game._time; };

        if (Game.cpu.bucket < 500 || Game.cpu.getUsed() > 100) {
            return;
        }

        if (wasm_module == null) {
            console.log("Reset!");

            wasm_module = require("screeps-ibex");
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

        wasm_module.tick();
    } catch(error) {
        console_error("Caught exception:", error);
        if (error.stack) {
            console_error("Stack trace:", error.stack);
        }
        console_error("Resetting VM next tick.");
        
        wasm_reset();
    }
};