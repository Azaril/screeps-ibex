"use strict";

let wasm_module = null;
let initialized = false;

function wasm_reset() {
    wasm_module = null;
    initialized = false;
}

module.exports.loop = function() {
    //TODO: Fix this so the panic hook can log normally?
    console.error = function(...args) {
        console.log(...args);
    }

    try {    
        if (wasm_module == null) {
            //if (Game.cpu.bucket < 500 || Game.cpu.getUsed() > 100) {
                //return;
            //}

            console.log("Reset!");

            wasm_module = require("screeps-ibex");
        }

        if (wasm_module != null && !initialized) {
            //if (Game.cpu.bucket < 500 || Game.cpu.getUsed() > 100) {
                //return;
            //}

            console.log("Initializing!");

            wasm_module.initialize_instance();

            wasm_module.setup();

            initialized = true;

            //if (Game.cpu.bucket < 500 || Game.cpu.getUsed() > 100) {
                //return;
            //}
        }
        
        if (initialized) {
            wasm_module.tick();
        }
    } catch(error) {
        console.error("Caught exception:", error);
        if (error.stack) {
            console.error("Stack trace:", error.stack);
        }

        console.error("Resetting VM!");
        
        wasm_reset();
    }
};