//! Step 5 — module-map assembly and upload via the shared
//! `screeps-rest-api` client (`POST /api/user/code`, the pinned
//! `{branch, modules, _hash}` shape).
//!
//! The uploaded map (3 modules vs deploy.js's 2 — rollup inlined the
//! bindgen JS into `main`; we upload it as its own CJS module, which
//! the engine's `require` handles natively):
//!
//! ```json
//! { "main": "<loader>", "<name>": "<patched glue>", "<name>_bg": {"binary": "<base64>"} }
//! ```
//!
//! Size accounting mirrors deploy.js:121-146 exactly: JS modules count
//! their UTF-8 length, wasm modules their BASE64 length, against the
//! 5 MiB code limit.

use anyhow::{Context, Result};
use screeps_rest_api::{AuthMode, Client, CodeModule, CodeModules, DEFAULT_MIN_DELAY_MS};
use secrecy::SecretString;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

use crate::config::{ServerAuth, ServerConfig};

/// The Screeps code-size limit (deploy.js:124; cargo-screeps warns at
/// the same boundary).
pub const CODE_SIZE_LIMIT_MIB: f64 = 5.0;

/// One uploaded module, as recorded in `manifest.json` (no content —
/// names, sizes, and hashes only; safe for run artifacts).
#[derive(Debug, Clone, Serialize)]
pub struct ModuleInfo {
    pub name: String,
    /// `"js"` or `"wasm"`.
    pub kind: &'static str,
    /// Size as counted against the 5 MiB limit (utf8 for js, base64
    /// for wasm — the deploy.js accounting).
    pub bytes: usize,
    /// SHA-256 of the module content (the base64 string for wasm) —
    /// the parity-diff currency.
    pub sha256: String,
}

/// The assembled upload.
#[derive(Debug)]
pub struct ModuleMap {
    pub modules: CodeModules,
    pub infos: Vec<ModuleInfo>,
    pub used_mib: f64,
    pub used_percent: f64,
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Assemble the 3-module map (pure; pinned by tests).
pub fn assemble(module_name: &str, loader_js: &str, glue_js: &str, wasm_bytes: &[u8]) -> ModuleMap {
    let mut modules = CodeModules::new();
    let mut infos = Vec::new();

    for (name, source) in [
        ("main".to_string(), loader_js),
        (module_name.to_string(), glue_js),
    ] {
        infos.push(ModuleInfo {
            name: name.clone(),
            kind: "js",
            bytes: source.len(),
            sha256: sha256_hex(source),
        });
        modules.insert(name, CodeModule::Source(source.to_string()));
    }

    let binary = CodeModule::from_binary(wasm_bytes);
    let CodeModule::Binary { binary: b64 } = &binary else {
        unreachable!("from_binary returns Binary");
    };
    let wasm_name = format!("{module_name}_bg");
    infos.push(ModuleInfo {
        name: wasm_name.clone(),
        kind: "wasm",
        bytes: b64.len(),
        sha256: sha256_hex(b64),
    });
    modules.insert(wasm_name, binary);

    let used_bytes: usize = infos.iter().map(|i| i.bytes).sum();
    let used_mib = used_bytes as f64 / (1024.0 * 1024.0);
    ModuleMap {
        modules,
        infos,
        used_mib,
        used_percent: 100.0 * used_mib / CODE_SIZE_LIMIT_MIB,
    }
}

/// Write `manifest.json` (names/sizes/hashes — the parity-diff record)
/// next to the dist files.
pub fn write_manifest(dist_dir: &Path, map: &ModuleMap) -> Result<()> {
    #[derive(Serialize)]
    struct Manifest<'a> {
        modules: &'a [ModuleInfo],
        used_mib: f64,
        used_percent_of_limit: f64,
    }
    let path = dist_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(&Manifest {
        modules: &map.infos,
        used_mib: map.used_mib,
        used_percent_of_limit: map.used_percent,
    })?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Build the REST client for a server entry. Courtesy delay: zero for
/// plain-HTTP private servers, the screeps.com-safe default otherwise.
pub fn client_for(server: &ServerConfig) -> Result<Client> {
    let auth = match &server.auth {
        ServerAuth::Token(token) => {
            use secrecy::ExposeSecret;
            AuthMode::Token(SecretString::from(token.expose_secret()))
        }
        ServerAuth::UserPass { username, password } => {
            use secrecy::ExposeSecret;
            AuthMode::UserPass {
                username: username.clone(),
                password: SecretString::from(password.expose_secret()),
            }
        }
    };
    let min_delay = if server.secure {
        Duration::from_millis(DEFAULT_MIN_DELAY_MS)
    } else {
        Duration::ZERO
    };
    Ok(Client::new(server.base_url(), None, auth, min_delay)?)
}

/// Sign in (no-op for token auth) and upload the map to the entry's
/// branch. Secrets stay inside the client (SecretString discipline).
pub async fn upload(server: &ServerConfig, map: &ModuleMap) -> Result<()> {
    let client = client_for(server)?;
    client
        .sign_in()
        .await
        .with_context(|| format!("signing in to {} ({})", server.name, server.base_url()))?;
    client
        .upload_code(&server.branch, &map.modules)
        .await
        .with_context(|| {
            format!(
                "uploading {} modules to {} branch '{}'",
                map.infos.len(),
                server.name,
                server.branch
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE MODULE-MAP PIN: 3 modules — loader as `main`, the glue under
    /// the module name, the wasm as `<name>_bg` `{binary}` — and the
    /// deploy.js size accounting (utf8 for js, base64 for wasm).
    #[test]
    fn assembles_the_three_module_map() {
        let wasm = [0x00u8, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let map = assemble("screeps_ibex", "loader();", "glue();", &wasm);

        let names: Vec<_> = map.modules.keys().cloned().collect();
        assert_eq!(names, ["main", "screeps_ibex", "screeps_ibex_bg"]);
        assert!(matches!(&map.modules["main"], CodeModule::Source(s) if s == "loader();"));
        assert!(
            matches!(&map.modules["screeps_ibex_bg"], CodeModule::Binary { binary } if binary == "AGFzbQEAAAA=")
        );
        // Size accounting: 9 + 7 utf8 + 12 base64 chars.
        let total: usize = map.infos.iter().map(|i| i.bytes).sum();
        assert_eq!(total, 9 + 7 + 12);
        assert!((map.used_percent - 100.0 * map.used_mib / 5.0).abs() < 1e-9);
    }

    /// The wire JSON matches the engine contract (game.js:561-563:
    /// `{binary}` modules become Buffers at require()).
    #[test]
    fn wire_shape_matches_engine_contract() {
        let map = assemble("b", "L", "G", &[0, 97, 115, 109]);
        let wire = serde_json::to_string(&map.modules).unwrap();
        assert_eq!(wire, r#"{"b":"G","b_bg":{"binary":"AGFzbQ=="},"main":"L"}"#);
    }

    #[test]
    fn manifest_records_hashes_not_content() {
        let dir = tempfile::tempdir().unwrap();
        let map = assemble("b", "LOADER-CONTENT", "GLUE-CONTENT", &[1, 2, 3]);
        write_manifest(dir.path(), &map).unwrap();
        let written = std::fs::read_to_string(dir.path().join("manifest.json")).unwrap();
        assert!(written.contains("\"sha256\""));
        assert!(
            !written.contains("LOADER-CONTENT"),
            "content must not leak into the manifest"
        );
    }
}
