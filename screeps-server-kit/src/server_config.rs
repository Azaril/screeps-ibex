//! Launcher-config preparation: merge the vendored keyless template
//! (`config/server.yml`) with the runtime-only settings from
//! `config/local.yml` and write the result under `target/runtime/` for
//! the container bind-mount (P0.A2; config layout per P0.A9(b) — the
//! external-base `serverConfig:` indirection was dropped, the steamKey
//! comes straight from `config/local.yml`).
//!
//! Schema source (pinned during P0.A2 investigation, 2026-06-09):
//! screepers/screeps-launcher @ main, `launcher/config.go`:
//! - `steamKey` (string, top level) — exported to the backend process
//!   as `STEAM_KEY` env by the launcher (`GetConfig`).
//! - `cli:` `{username, password, host, port}` — the **CLI client**
//!   connect target used by `screeps-launcher cli` (defaults
//!   `127.0.0.1` / `21026` in `NewConfig`). NOT the server bind.
//! - `env.backend.CLI_HOST` / `env.backend.CLI_PORT` — the **CLI
//!   server bind** (defaults `"127.0.0.1"` / `"21026"`). In-container
//!   localhost bind is the default failure mode; the merge forces
//!   `0.0.0.0` + the configured port so the host can reach it.
//! - `env.backend.GAME_HOST` / `GAME_PORT` — game/API bind (defaults
//!   `"0.0.0.0"` / `"21025"`); forced too, so the published ports are
//!   always exactly `eval.ports.*` on both sides of the mapping.
//! - `serverConfig.tickRate` (ms, requires screepsmod-admin-utils,
//!   per upstream README) — set from `eval.tickMs`.
//!
//! SECRETS: the merged [`serde_yaml::Value`] carries the steamKey.
//! It is never logged or Debug-printed; it is serialized only by
//! [`write_runtime_config`] into this crate's `target/runtime/`
//! (gitignored via the repo-global `target` rule; swept by P0.A7).

use crate::config::StackSettings;
use anyhow::{bail, Context, Result};
use secrecy::{ExposeSecret, SecretString};
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

/// The vendored keyless launcher-config template — always the merge
/// base (P0.A9(b)). Committed; MUST stay keyless.
pub const LAUNCHER_TEMPLATE: &str = include_str!("../config/server.yml");

/// Where the merged runtime config is written, relative to this crate.
/// Compile-time crate root is acceptable while the crate lives in-repo
/// and is driven via `cargo run` (revisit at submodule extraction, D-1).
pub fn runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("runtime")
}

/// PURE merge of a base launcher config with the kit-controlled
/// runtime settings. No I/O; unit-tested with literal fixtures.
///
/// Rules (P0.A2; production always passes [`LAUNCHER_TEMPLATE`] as the
/// base since P0.A9(b) — the base parameter stays so the merge is pure
/// and testable against literal bases):
/// - steamKey: a key already present in the base **wins**; otherwise
///   `local_steam_key` (from `config/local.yml`) is inserted; neither
///   present is an error (the screeps backend cannot start without one).
/// - CLI bind: `env.backend.CLI_HOST` forced to `0.0.0.0` and
///   `CLI_PORT` to `cli_port` — always, whatever the base says.
/// - Game bind: `env.backend.GAME_HOST`/`GAME_PORT` forced likewise.
/// - Tick rate: `serverConfig.tickRate` set to `tick_ms`
///   (screepsmod-admin-utils consumes it).
pub fn merge_launcher_config(
    base_yaml: &str,
    local_steam_key: Option<&SecretString>,
    game_port: u16,
    cli_port: u16,
    tick_ms: u64,
) -> Result<Value> {
    let mut root: Value =
        serde_yaml::from_str(base_yaml).context("base launcher config is not valid YAML")?;
    if root.is_null() {
        // An empty base file parses as null; treat as an empty mapping.
        root = Value::Mapping(Mapping::new());
    }
    let Some(map) = root.as_mapping_mut() else {
        bail!("base launcher config is not a YAML mapping");
    };

    // --- steamKey: base wins; eval fills the gap; neither = error ---
    let base_has_key = matches!(map.get("steamKey"), Some(Value::String(s)) if !s.trim().is_empty())
        || matches!(map.get("steamKeyFile"), Some(Value::String(s)) if !s.trim().is_empty());
    if !base_has_key {
        match local_steam_key {
            Some(key) => {
                map.insert(
                    Value::from("steamKey"),
                    Value::from(key.expose_secret().to_string()),
                );
            }
            None => bail!(
                "no Steam key available: set steamKey in screeps-server-kit/config/local.yml \
                 (copy config/local.example.yml; the screeps backend cannot start \
                 without it)"
            ),
        }
    }

    // --- warn early on a base that will not use our mongo/redis ---
    let has_mongo_mod = matches!(map.get("mods"), Some(Value::Sequence(mods))
        if mods.iter().any(|m| m.as_str() == Some("screepsmod-mongo")));
    if !has_mongo_mod {
        tracing::warn!(
            "base launcher config has no screepsmod-mongo in `mods:` — the server \
             will use built-in LokiJS storage instead of the mongo/redis containers"
        );
    }

    // --- env.backend: force the binds (launcher defaults are
    //     CLI_HOST=127.0.0.1, unreachable from the host) ---
    // Launcher env maps are map[string]string (config.go) — values must
    // be YAML strings, hence to_string() on the ports.
    let env = ensure_mapping(map, "env")?;
    let backend = ensure_mapping(env, "backend")?;
    backend.insert(Value::from("GAME_HOST"), Value::from("0.0.0.0"));
    backend.insert(Value::from("GAME_PORT"), Value::from(game_port.to_string()));
    backend.insert(Value::from("CLI_HOST"), Value::from("0.0.0.0"));
    backend.insert(Value::from("CLI_PORT"), Value::from(cli_port.to_string()));

    // --- serverConfig.tickRate (screepsmod-admin-utils) ---
    let server_config = ensure_mapping(map, "serverConfig")?;
    server_config.insert(Value::from("tickRate"), Value::from(tick_ms));

    Ok(root)
}

/// Get `map[key]` as a mutable mapping, inserting an empty one if the
/// key is absent; error if it exists with a non-mapping shape.
fn ensure_mapping<'a>(map: &'a mut Mapping, key: &str) -> Result<&'a mut Mapping> {
    let entry = map
        .entry(Value::from(key))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    entry
        .as_mapping_mut()
        .with_context(|| format!("launcher config key `{key}` is not a mapping"))
}

/// Write the merged config to `target/runtime/config.yml` (gitignored;
/// the ONLY sanctioned on-disk location for the steamKey — P0.A7(b))
/// and return the absolute path for the container bind-mount.
pub fn write_runtime_config(merged: &Value) -> Result<PathBuf> {
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating runtime dir {}", dir.display()))?;
    let path = dir.join("config.yml");
    let yaml = serde_yaml::to_string(merged).context("serializing merged launcher config")?;
    std::fs::write(&path, yaml).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// One-stop: vendored template -> merge with `config/local.yml`
/// settings -> `target/runtime/config.yml`. Returns the runtime config
/// path.
pub fn prepare_runtime_config(stack: &StackSettings) -> Result<PathBuf> {
    let merged = merge_launcher_config(
        LAUNCHER_TEMPLATE,
        stack.steam_key.as_ref(),
        stack.game_port,
        stack.cli_port,
        stack.tick_ms,
    )?;
    write_runtime_config(&merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_KEY: &str = "fake-steam-key-for-tests-001";

    fn fake_key() -> SecretString {
        SecretString::from(FAKE_KEY)
    }

    fn get<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
        let mut cur = root;
        for key in path {
            cur = cur.as_mapping()?.get(*key)?;
        }
        Some(cur)
    }

    /// The committed template must never grow a key. (Belt-and-braces
    /// with the A7 sweep; cheap to pin.)
    #[test]
    fn vendored_template_is_keyless() {
        let root: Value = serde_yaml::from_str(LAUNCHER_TEMPLATE).unwrap();
        assert!(
            get(&root, &["steamKey"]).is_none(),
            "template must stay keyless"
        );
        assert!(get(&root, &["steamKeyFile"]).is_none());
    }

    #[test]
    fn template_plus_eval_steam_key_works() {
        let merged =
            merge_launcher_config(LAUNCHER_TEMPLATE, Some(&fake_key()), 21025, 21026, 100).unwrap();
        assert_eq!(
            get(&merged, &["steamKey"]).and_then(Value::as_str),
            Some(FAKE_KEY)
        );
        // Template content survives the merge.
        let mods = get(&merged, &["mods"])
            .and_then(Value::as_sequence)
            .unwrap();
        assert!(mods.iter().any(|m| m.as_str() == Some("screepsmod-mongo")));
    }

    #[test]
    fn base_steam_key_wins_over_eval_key() {
        let base = "steamKey: base-key-already-here\n";
        let merged = merge_launcher_config(base, Some(&fake_key()), 21025, 21026, 100).unwrap();
        assert_eq!(
            get(&merged, &["steamKey"]).and_then(Value::as_str),
            Some("base-key-already-here"),
            "a key in the base config must win (eval.steamKey is the fallback)"
        );
    }

    #[test]
    fn base_steam_key_file_counts_as_a_key() {
        let base = "steamKeyFile: /screeps/STEAM_KEY\n";
        let merged = merge_launcher_config(base, None, 21025, 21026, 100).unwrap();
        assert!(get(&merged, &["steamKey"]).is_none());
        assert!(get(&merged, &["steamKeyFile"]).is_some());
    }

    #[test]
    fn no_key_anywhere_is_a_clear_error() {
        let err = merge_launcher_config("mods: []\n", None, 21025, 21026, 100).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("config/local.yml"),
            "error must say how to fix it: {msg}"
        );
    }

    /// The launcher's default CLI bind is in-container 127.0.0.1 — the
    /// default failure mode. The merge must force the bind even when the
    /// base explicitly says otherwise.
    #[test]
    fn cli_bind_is_always_forced() {
        let base = r#"
steamKey: base-key
env:
  backend:
    CLI_HOST: 127.0.0.1
    CLI_PORT: "9999"
    SOME_OTHER: keepme
"#;
        let merged = merge_launcher_config(base, None, 21025, 21026, 100).unwrap();
        assert_eq!(
            get(&merged, &["env", "backend", "CLI_HOST"]).and_then(Value::as_str),
            Some("0.0.0.0")
        );
        assert_eq!(
            get(&merged, &["env", "backend", "CLI_PORT"]).and_then(Value::as_str),
            Some("21026"),
            "CLI_PORT must be a YAML string (launcher env maps are map[string]string)"
        );
        // Sibling env keys survive.
        assert_eq!(
            get(&merged, &["env", "backend", "SOME_OTHER"]).and_then(Value::as_str),
            Some("keepme")
        );
    }

    #[test]
    fn game_bind_and_custom_ports_are_forced() {
        let merged =
            merge_launcher_config(LAUNCHER_TEMPLATE, Some(&fake_key()), 31025, 31026, 100).unwrap();
        assert_eq!(
            get(&merged, &["env", "backend", "GAME_HOST"]).and_then(Value::as_str),
            Some("0.0.0.0")
        );
        assert_eq!(
            get(&merged, &["env", "backend", "GAME_PORT"]).and_then(Value::as_str),
            Some("31025")
        );
        assert_eq!(
            get(&merged, &["env", "backend", "CLI_PORT"]).and_then(Value::as_str),
            Some("31026")
        );
    }

    /// A base with no env/serverConfig sections at all (the local
    /// launcher-repo reference config has neither) gains both.
    #[test]
    fn missing_sections_are_created_and_tick_rate_set() {
        let base = "steamKey: base-key\nmods:\n  - screepsmod-mongo\n";
        let merged = merge_launcher_config(base, None, 21025, 21026, 250).unwrap();
        assert_eq!(
            get(&merged, &["env", "backend", "CLI_HOST"]).and_then(Value::as_str),
            Some("0.0.0.0")
        );
        assert_eq!(
            get(&merged, &["serverConfig", "tickRate"]).and_then(Value::as_u64),
            Some(250)
        );
    }

    #[test]
    fn base_tick_rate_is_overridden_but_siblings_survive() {
        let base = r#"
steamKey: base-key
serverConfig:
  tickRate: 1000
  welcomeText: hello
"#;
        let merged = merge_launcher_config(base, None, 21025, 21026, 100).unwrap();
        assert_eq!(
            get(&merged, &["serverConfig", "tickRate"]).and_then(Value::as_u64),
            Some(100)
        );
        assert_eq!(
            get(&merged, &["serverConfig", "welcomeText"]).and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn malformed_base_is_a_clear_error() {
        let err = merge_launcher_config(
            "- just\n- a\n- list\n",
            Some(&fake_key()),
            21025,
            21026,
            100,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a YAML mapping"));
    }
}
