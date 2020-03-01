"use strict";
let wasm_module = null;
let stdweb_vars = null;
let wasm_instance = null;
let initialized_stdweb_vars = false;

function wasm_initialize() {
    if (Game.cpu.bucket < 500) {
        return;
    }
    if (wasm_module == null) {
        let wasm_bytes = wasm_fetch_module_bytes();
        wasm_module = new WebAssembly.Module(wasm_bytes);
    }
    // The biggest CPU users will be the call to `new WebAssembly.Module` and
    // `new WebAssembly.Instance`, so having two checks will be useful.
    if (Game.cpu.bucket < 500) {
        return;
    }
    
    if (stdweb_vars == null) {
        stdweb_vars = wasm_create_stdweb_vars();
    }

    if (Game.cpu.bucket < 500) {
        return;
    }

    if (wasm_instance == null) {
        wasm_instance = new WebAssembly.Instance(wasm_module, stdweb_vars.imports);
    }
    
    if (Game.cpu.bucket < 500) {
        return;
    }

    if (!initialized_stdweb_vars) {
        stdweb_vars.initialize(wasm_instance);

        initialized_stdweb_vars = true;
    }
    
    if (Game.cpu.bucket < 500) {
        return;
    }

    module.exports.loop();
}

module.exports.loop = wasm_initialize;