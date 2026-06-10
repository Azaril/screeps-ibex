//! The `POST /api/user/code` module map — code upload (for the future
//! rust-native deploy tool, P0.A11; pinned now per P0.A12 so the shape
//! lives with the rest of the endpoint set).
//!
//! ## Pinned request shape
//!
//! Body: `{branch, modules, _hash}` —
//! `node_modules/screeps-api/dist/ScreepsAPI.js:894-897` (`code.set`:
//! `if (!_hash) _hash = Date.now()` then
//! `POST /api/user/code {branch, modules, _hash}`), the exact client
//! `js_tools/deploy.js:156-158` drives (`api.code.set(branch,
//! code.modules)`).
//!
//! `modules` is a map of module name -> module content, where content is
//! EITHER a plain JavaScript source string OR `{binary: <base64>}` for a
//! binary (wasm) module — pinned from `js_tools/deploy.js:121-146`
//! (`load_built_code`: `.wasm` files become
//! `modules[name] = {binary: <base64 data>}`, everything else
//! `modules[name] = <utf8 source>`; the `main` module is the entry
//! point). Response: `{ok: 1}`.
//!
//! Official-server rate limits for this endpoint
//! (ScreepsAPI.js:1421/:1433): GET 60/hour, POST 240/day.

use base64::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One code module: plain JavaScript source, or a binary (wasm) module
/// uploaded as base64 under a `binary` key. `untagged`: a JSON string
/// is [`Source`](CodeModule::Source), an object is
/// [`Binary`](CodeModule::Binary) — exactly the wire shape deploy.js
/// produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CodeModule {
    /// Plain JavaScript source (uploaded verbatim).
    Source(String),
    /// Binary module — base64-encoded content (deploy.js:128:
    /// `fs.readFileSync(..., {encoding: 'base64'})`).
    Binary { binary: String },
}

impl CodeModule {
    /// Wrap raw binary content (e.g. a compiled wasm module) as a
    /// base64 binary module.
    pub fn from_binary(bytes: &[u8]) -> Self {
        CodeModule::Binary {
            // Standard base64 (RFC 4648, with padding) — what Node's
            // Buffer.toString('base64') produces (deploy.js:128). Via the
            // `base64` crate per the workspace convention (P0.A12.1) —
            // never hand-roll well-solved encodings.
            binary: BASE64_STANDARD.encode(bytes),
        }
    }
}

/// The `modules` map of a code upload: module name (no extension) ->
/// content. `BTreeMap` for a deterministic wire order.
pub type CodeModules = BTreeMap<String, CodeModule>;

#[cfg(test)]
mod tests {
    use super::*;

    /// THE WIRE-SHAPE PIN: a module map serializes to exactly what
    /// deploy.js's `load_built_code` hands `api.code.set` — JS modules
    /// as plain strings, wasm modules as `{"binary": <base64>}`.
    #[test]
    fn module_map_serializes_to_the_deploy_js_shape() {
        let mut modules = CodeModules::new();
        modules.insert(
            "main".to_owned(),
            CodeModule::Source("module.exports.loop = function() {}".to_owned()),
        );
        modules.insert(
            "screeps_ibex".to_owned(),
            CodeModule::Binary {
                binary: "AGFzbQEAAAA=".to_owned(), // base64 of a wasm header
            },
        );
        assert_eq!(
            serde_json::to_string(&modules).unwrap(),
            r#"{"main":"module.exports.loop = function() {}","screeps_ibex":{"binary":"AGFzbQEAAAA="}}"#
        );
    }

    /// RECORDED SHAPE: `GET /api/user/code` returns the same module map
    /// (`{ok, branch, modules}` — Endpoints.md:214; round-trips through
    /// the untagged enum).
    #[test]
    fn module_map_deserializes_both_variants() {
        let fixture = r#"{"main":"console.log(1)","bot":{"binary":"AGFzbQ=="}}"#;
        let modules: CodeModules = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            modules["main"],
            CodeModule::Source("console.log(1)".to_owned())
        );
        assert_eq!(
            modules["bot"],
            CodeModule::Binary {
                binary: "AGFzbQ==".to_owned()
            }
        );
    }

    /// The MODULE SHAPE pin (not encoding internals — the `base64` crate
    /// owns those per P0.A12.1): a wasm magic header wraps to the exact
    /// `{binary: <base64>}` Node/deploy.js produces ("AGFzbQEAAAA=").
    #[test]
    fn binary_module_matches_node_buffer_encoding() {
        assert_eq!(
            CodeModule::from_binary(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]),
            CodeModule::Binary {
                binary: "AGFzbQEAAAA=".to_owned()
            }
        );
    }
}
