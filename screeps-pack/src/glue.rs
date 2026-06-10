//! Step 3 — the JS glue: patch the `--target nodejs` wasm-bindgen
//! output for the Screeps isolate, and render the loader (`main`)
//! module from the embedded template.
//!
//! ## The patch (anchored + version-gated, NOT a loose regex)
//!
//! cargo-screeps' single-regex patcher rotted when bindgen output
//! drifted (investigation §2a); the antidote is trunk's pattern
//! (`wasm_bindgen.rs:92-118`): anchor on the EXACT emitted shape and
//! hard-error on anything unexpected, naming the version. The anchored
//! tail of wasm-bindgen 0.2.108 `--target nodejs` output (verified
//! against live output, 2026-06-10):
//!
//! ```js
//! const wasmPath = `${__dirname}/<name>_bg.wasm`;
//! const wasmBytes = require('fs').readFileSync(wasmPath);
//! const wasmModule = new WebAssembly.Module(wasmBytes);
//! const wasm = new WebAssembly.Instance(wasmModule, __wbg_get_imports()).exports;
//! ```
//!
//! (optionally followed by `wasm.__wbindgen_start();`). The isolate has
//! no `fs`/`path`/`__dirname`, and synchronous load-at-require would
//! also defeat the loader's multi-tick staging — the tail becomes a
//! deferred `exports.__instantiate(compiledModule)` under loader
//! control (the cargo-screeps `initialize_instance` idea, split so the
//! loader keeps ibex's per-tick require→compile→instantiate staging).
//!
//! ## The polyfill
//!
//! Bindgen references bare `TextDecoder`/`TextEncoder` globals — absent
//! in the isolate. The vendored CC0 FastestSmallestTextEncoderDecoder
//! (`templates/text-encoder-decoder.min.js` — the SAME polyfill both
//! the deploy.js bundle and cargo-screeps used) is prepended to the
//! glue module; its IIFE attaches the globals before any bindgen code
//! evaluates.

use anyhow::{bail, Result};

/// The vendored TextEncoder/TextDecoder polyfill
/// (<https://github.com/anonyco/FastestSmallestTextEncoderDecoder>, CC0).
pub const TEXT_POLYFILL: &str = include_str!("../templates/text-encoder-decoder.min.js");

/// The loader template (genericized js_src/main.js — bucket gate,
/// staged init, console.error shim, wasm-bindgen#3130 halt trap).
pub const LOADER_TEMPLATE: &str = include_str!("../templates/loader.js");

/// Bindgen versions whose `--target nodejs` output the anchored patch
/// is verified against. Other versions hard-error rather than risk
/// silent corruption (risk #1) — extend after inspecting the output.
pub const VERIFIED_BINDGEN_OUTPUT: &str = "0.2.108";

/// Patch the nodejs-target bindgen JS for the Screeps isolate:
/// polyfill prepended, synchronous wasm load replaced by a deferred
/// `exports.__instantiate`. `version` is diagnostic (error messages).
pub fn patch_bindgen_js(js: &str, module_name: &str, version: &str) -> Result<String> {
    let anchor = format!(
        "const wasmPath = `${{__dirname}}/{module_name}_bg.wasm`;\n\
         const wasmBytes = require('fs').readFileSync(wasmPath);\n\
         const wasmModule = new WebAssembly.Module(wasmBytes);\n\
         const wasm = new WebAssembly.Instance(wasmModule, __wbg_get_imports()).exports;"
    );
    let Some(pos) = js.find(&anchor) else {
        bail!(
            "wasm-bindgen {version} emitted JS whose wasm-load tail does not match the \
             anchored shape this tool patches (verified against: {VERIFIED_BINDGEN_OUTPUT}). \
             Refusing to guess — inspect the new output and extend the patcher \
             (screeps-pack/src/glue.rs)."
        );
    };
    if js[pos + anchor.len()..].contains(&anchor[..anchor.find('\n').unwrap_or(0)]) {
        bail!(
            "wasm-bindgen {version} output contains the wasm-load tail twice — refusing to patch"
        );
    }

    // Whatever follows the anchor must be only whitespace, optionally
    // with the start-function call — anything else is unknown new code.
    let remainder = &js[pos + anchor.len()..];
    let start_call = "wasm.__wbindgen_start();";
    let has_start = remainder.contains(start_call);
    let leftover = remainder.replace(start_call, "");
    if !leftover.trim().is_empty() {
        bail!(
            "wasm-bindgen {version} emitted unexpected code after the wasm-load tail \
             (verified against: {VERIFIED_BINDGEN_OUTPUT}):\n{}",
            leftover.trim()
        );
    }

    let replacement = format!(
        "// screeps-pack: deferred instantiation — the loader module stages\n\
         // require(bytes) -> compile -> __instantiate across ticks.\n\
         let wasm = null;\n\
         exports.__instantiate = function(compiledModule) {{\n\
         \x20   wasm = new WebAssembly.Instance(compiledModule, __wbg_get_imports()).exports;\n\
         {}\
         \x20   return wasm;\n\
         }};\n",
        if has_start {
            "\x20   wasm.__wbindgen_start();\n"
        } else {
            ""
        }
    );

    let patched = format!(
        "// screeps-pack: vendored TextEncoder/TextDecoder polyfill (CC0,\n\
         // github.com/anonyco/FastestSmallestTextEncoderDecoder) — the isolate\n\
         // has no util module and no global TextDecoder.\n\
         {TEXT_POLYFILL}\n\
         {}{replacement}",
        &js[..pos]
    );

    // Belt and braces against partial patches / future output drift:
    // nothing Node-only may survive into the isolate.
    for forbidden in [
        "__dirname",
        "require('fs')",
        "require(\"fs\")",
        "require('path')",
        "require(\"path\")",
    ] {
        if patched.contains(forbidden) {
            bail!(
                "patched glue still contains `{forbidden}` — wasm-bindgen {version} \
                 output drifted (verified against: {VERIFIED_BINDGEN_OUTPUT})"
            );
        }
    }
    Ok(patched)
}

/// Render the loader (`main`) module for a bot.
pub fn render_loader(module_name: &str, bucket_boot_threshold: u32) -> Result<String> {
    let rendered = LOADER_TEMPLATE
        .replace("__SCREEPS_PACK_BOT_MODULE__", module_name)
        .replace("__SCREEPS_PACK_WASM_MODULE__", &format!("{module_name}_bg"))
        .replace(
            "__SCREEPS_PACK_BUCKET_THRESHOLD__",
            &bucket_boot_threshold.to_string(),
        );
    if rendered.contains("__SCREEPS_PACK_") {
        bail!("loader template contains an unknown placeholder — template/render drifted");
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature of the REAL 0.2.108 nodejs output shape (head
    /// trimmed; the tail is verbatim from live output, 2026-06-10).
    fn fixture(tail_extra: &str) -> String {
        format!(
            "let cachedTextDecoder = new TextDecoder('utf-8', {{ ignoreBOM: true, fatal: true }});\n\
             cachedTextDecoder.decode();\n\
             function __wbg_get_imports() {{\n    return {{}};\n}}\n\
             exports.game_loop = function() {{ wasm.game_loop(); }};\n\
             \n\
             const wasmPath = `${{__dirname}}/screeps_ibex_bg.wasm`;\n\
             const wasmBytes = require('fs').readFileSync(wasmPath);\n\
             const wasmModule = new WebAssembly.Module(wasmBytes);\n\
             const wasm = new WebAssembly.Instance(wasmModule, __wbg_get_imports()).exports;\n\
             {tail_extra}"
        )
    }

    /// The happy path: tail replaced by deferred __instantiate, the
    /// polyfill prepended, no Node-isms survive.
    #[test]
    fn patches_the_verified_tail_shape() {
        let patched = patch_bindgen_js(&fixture(""), "screeps_ibex", "0.2.108").unwrap();
        assert!(patched.contains("exports.__instantiate = function(compiledModule)"));
        assert!(patched.contains("new WebAssembly.Instance(compiledModule, __wbg_get_imports())"));
        assert!(patched.contains("let wasm = null;"));
        // The polyfill comes FIRST (globals must exist before bindgen
        // module code evaluates).
        let polyfill_pos = patched
            .find("EncoderDecoderTogether")
            .or_else(|| patched.find("TextDecoder="))
            .unwrap_or(usize::MAX);
        let decoder_use = patched.find("new TextDecoder('utf-8'").unwrap();
        assert!(
            polyfill_pos < decoder_use,
            "polyfill must precede bindgen code"
        );
        for forbidden in ["__dirname", "require('fs')", "readFileSync"] {
            assert!(!patched.contains(forbidden), "leftover: {forbidden}");
        }
        // The original wasm const is gone; the deferred binding remains.
        assert!(!patched.contains("const wasm ="));
    }

    /// Crates with a start section get the start call inside
    /// __instantiate (preserved semantics, deferred timing).
    #[test]
    fn start_function_call_is_preserved_in_instantiate() {
        let patched = patch_bindgen_js(
            &fixture("wasm.__wbindgen_start();\n"),
            "screeps_ibex",
            "0.2.108",
        )
        .unwrap();
        let inst = patched.split("exports.__instantiate").nth(1).unwrap();
        assert!(inst.contains("wasm.__wbindgen_start();"));
    }

    /// Unknown output shapes hard-error naming the version — never
    /// silent corruption (risk #1).
    #[test]
    fn unknown_tail_is_a_hard_error_naming_the_version() {
        let err = patch_bindgen_js(
            "const wasm = totallyNewLoaderShape();",
            "screeps_ibex",
            "0.2.999",
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("0.2.999"));
        assert!(msg.contains(VERIFIED_BINDGEN_OUTPUT));
    }

    /// Unexpected trailing code after the anchor is rejected too.
    #[test]
    fn unexpected_trailing_code_is_rejected() {
        let err =
            patch_bindgen_js(&fixture("somethingNew();\n"), "screeps_ibex", "0.2.108").unwrap_err();
        assert!(format!("{err:#}").contains("somethingNew"));
    }

    /// The wrong module name must not match (the anchor embeds it).
    #[test]
    fn anchor_is_module_name_specific() {
        assert!(patch_bindgen_js(&fixture(""), "other_bot", "0.2.108").is_err());
    }

    /// Loader render: placeholders fully substituted; the load-bearing
    /// behaviors (bucket gate, staged init, halt trap, loading-complete
    /// line) are present verbatim.
    #[test]
    fn loader_renders_faithfully() {
        let loader = render_loader("screeps_ibex", 1500).unwrap();
        assert!(!loader.contains("__SCREEPS_PACK_"));
        assert!(loader.contains(r#"const bot = require("screeps_ibex");"#));
        assert!(loader.contains(r#"const WASM_MODULE_NAME = "screeps_ibex_bg";"#));
        assert!(loader.contains("const BUCKET_BOOT_THRESHOLD = 1500;"));
        // The #3130 halt trap and its running flag.
        assert!(loader.contains("Game.cpu.halt();"));
        assert!(loader.contains("let running = false;"));
        // Staged multi-tick init, exactly the main.js shape.
        assert!(loader.contains("if (!wasm_bytes) wasm_bytes = require(WASM_MODULE_NAME);"));
        assert!(
            loader.contains("if (!wasm_module) wasm_module = new WebAssembly.Module(wasm_bytes);")
        );
        assert!(
            loader.contains("if (!wasm_instance) wasm_instance = bot.__instantiate(wasm_module);")
        );
        // Bucket gate + the smoke-visible boot lines.
        assert!(loader.contains("Game.cpu.bucket < BUCKET_BOOT_THRESHOLD"));
        assert!(loader.contains("loading complete, CPU used"));
        // One-time setup then the per-tick loop swap.
        assert!(loader.contains("bot.setup();"));
        assert!(loader.contains("module.exports.loop = loaded_loop;"));
    }

    #[test]
    fn loader_bucket_threshold_is_configurable() {
        let loader = render_loader("my_bot", 500).unwrap();
        assert!(loader.contains("const BUCKET_BOOT_THRESHOLD = 500;"));
        assert!(loader.contains(r#"require("my_bot")"#));
        assert!(loader.contains(r#""my_bot_bg""#));
    }
}
