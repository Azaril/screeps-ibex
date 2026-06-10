//! Configuration loading for the eval harness.
//!
//! Sources, in precedence order (highest wins):
//! 1. `SCREEPS_EVAL_*` environment variables
//! 2. The `.screeps.yaml` unified config (discovered by walking up from
//!    the current directory; same file the deploy tooling reads)
//!
//! SECRETS POLICY (Phase 0 plan P0.A7): every credential lives in a
//! [`SecretString`] from the moment it is parsed — `Debug`/`Display`
//! redact by construction, so secrets cannot reach logs or `runs/`
//! artifacts through formatting. The one remaining leak path is code
//! that calls `expose_secret()` and embeds the value in a payload
//! (e.g. the server-CLI `setPassword(...)` command) — those call sites
//! must mask before any echo/transcript; see `phase-0.md` P0.A7(c).

use anyhow::{bail, Context, Result};
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Name of the eval server entry in `.screeps.yaml` (`servers:` map).
pub const DEFAULT_SERVER_NAME: &str = "private-server";

/// Tick-rate floor in milliseconds. Operator-established: at/below
/// 50 ms the server and UI start failing to keep up (plan D-2).
pub const TICK_MS_FLOOR: u64 = 50;
/// Default tick rate for unattended smoke/baseline runs.
pub const TICK_MS_SMOKE: u64 = 100;
/// Default tick rate when a human is watching.
pub const TICK_MS_WATCH: u64 = 1000;

/// Default published game/API port (screeps-launcher `env.backend.GAME_PORT`).
pub const DEFAULT_GAME_PORT: u16 = 21025;
/// Default published server-CLI port (screeps-launcher `env.backend.CLI_PORT`).
pub const DEFAULT_CLI_PORT: u16 = 21026;

/// Top-level config consumed by the harness and the CLI.
#[derive(Debug)]
pub struct EvalConfig {
    /// API/game endpoint of the private server (host, port, creds).
    pub server: ServerEndpoint,
    /// The optional `eval:` section of `.screeps.yaml` (server-stack
    /// settings). Absent section = all defaults.
    pub eval: EvalSettings,
    /// Where `.screeps.yaml` was found (diagnostics only).
    pub source_path: Option<PathBuf>,
}

/// The `eval:` section of `.screeps.yaml` — file-driven server-stack
/// configuration (P0.A2; no host env vars required, operator directive).
#[derive(Debug)]
pub struct EvalSettings {
    /// Path to an existing screeps-launcher `config.yml` used as the
    /// merge base **verbatim, including its steamKey** (e.g. a local
    /// clone of the launcher repo). When unset, the vendored keyless
    /// template (`screeps-eval/server/config.yml`) is the base and
    /// `steam_key` must be provided instead.
    pub server_config: Option<PathBuf>,
    /// Steam Web API key, used ONLY when the base config lacks one.
    /// Lives in the merged runtime config under `target/runtime/`
    /// (gitignored) — never in the vendored template, never in logs.
    pub steam_key: Option<SecretString>,
    /// Published game/API port (host side and in-container, forced via
    /// `env.backend.GAME_PORT` at merge time). Default 21025.
    pub game_port: u16,
    /// Published server-CLI port (forced via `env.backend.CLI_PORT`).
    /// Default 21026.
    pub cli_port: u16,
    /// Tick duration in ms written to `serverConfig.tickRate`
    /// (screepsmod-admin-utils). Default 100; clamped to the 50 ms
    /// floor (plan D-2).
    pub tick_ms: u64,
}

impl Default for EvalSettings {
    fn default() -> Self {
        EvalSettings {
            server_config: None,
            steam_key: None,
            game_port: DEFAULT_GAME_PORT,
            cli_port: DEFAULT_CLI_PORT,
            tick_ms: TICK_MS_SMOKE,
        }
    }
}

/// One server entry resolved from `.screeps.yaml` + env overrides.
#[derive(Debug)]
pub struct ServerEndpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretString,
    pub secure: bool,
}

impl ServerEndpoint {
    pub fn http_base(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }
}

// ---- raw .screeps.yaml shape (only the fields we consume) ----

#[derive(Deserialize)]
struct RawUnifiedConfig {
    /// Entries stay lazily-typed: other entries (e.g. `mmo`, which uses
    /// token auth and has no `username`) must not break parsing of the
    /// one entry we actually select.
    servers: HashMap<String, serde_yaml::Value>,
    /// Optional `eval:` section (lazily-typed so an absent or foreign-
    /// shaped section gives a targeted error, not a whole-file failure).
    eval: Option<serde_yaml::Value>,
}

/// Raw `eval:` section shape — key names match `.example-screeps.yaml`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawEval {
    server_config: Option<PathBuf>,
    steam_key: Option<SecretString>,
    #[serde(default)]
    ports: RawEvalPorts,
    tick_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawEvalPorts {
    game: Option<u16>,
    cli: Option<u16>,
}

#[derive(Deserialize)]
struct RawServer {
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    username: String,
    password: SecretString,
    #[serde(default)]
    secure: bool,
}

fn default_port() -> u16 {
    21025
}

impl From<RawEval> for EvalSettings {
    fn from(raw: RawEval) -> Self {
        let defaults = EvalSettings::default();
        let tick_ms = raw.tick_ms.unwrap_or(defaults.tick_ms);
        EvalSettings {
            server_config: raw.server_config,
            steam_key: raw.steam_key,
            game_port: raw.ports.game.unwrap_or(defaults.game_port),
            cli_port: raw.ports.cli.unwrap_or(defaults.cli_port),
            // Clamp to the operator-established floor (plan D-2) rather
            // than erroring: the config is shared/long-lived, and a
            // too-low value is a tuning mistake, not a corruption.
            tick_ms: tick_ms.max(TICK_MS_FLOOR),
        }
    }
}

/// Walk up from `start` looking for `.screeps.yaml`.
pub fn discover_config_file(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(".screeps.yaml");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

impl EvalConfig {
    /// Load from an explicit path, or discover by walking up from the
    /// current directory. `server_name` selects the `servers:` entry.
    pub fn load(explicit: Option<&Path>, server_name: &str) -> Result<Self> {
        let path = match explicit {
            Some(p) => p.to_path_buf(),
            None => {
                let cwd = std::env::current_dir().context("cannot determine current directory")?;
                discover_config_file(&cwd).context(
                    ".screeps.yaml not found (walked up from current directory); \
                     pass --config or create one from .example-screeps.yaml",
                )?
            }
        };
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut cfg = Self::from_yaml_str(&raw, server_name)
            .with_context(|| format!("parsing {}", path.display()))?;
        cfg.source_path = Some(path);
        cfg.apply_env_overrides();
        Ok(cfg)
    }

    /// Parse from YAML content (separated from I/O for testability —
    /// tests construct configs from literals, never from the real,
    /// secret-bearing `.screeps.yaml`).
    pub fn from_yaml_str(yaml: &str, server_name: &str) -> Result<Self> {
        let raw: RawUnifiedConfig = serde_yaml::from_str(yaml)?;
        let Some(value) = raw.servers.get(server_name) else {
            let known: Vec<_> = raw.servers.keys().cloned().collect();
            bail!("server '{server_name}' not in .screeps.yaml (known: {known:?})");
        };
        let server: RawServer = serde_yaml::from_value(value.clone())
            .with_context(|| format!("server entry '{server_name}' has an unexpected shape"))?;
        let eval = match raw.eval {
            // Lazy: absent section = defaults (the section is optional).
            None => EvalSettings::default(),
            Some(value) => {
                let raw_eval: RawEval = serde_yaml::from_value(value)
                    .context("the `eval:` section has an unexpected shape")?;
                EvalSettings::from(raw_eval)
            }
        };
        let server = &server;
        Ok(EvalConfig {
            server: ServerEndpoint {
                host: server.host.clone(),
                port: server.port,
                username: server.username.clone(),
                // SecretString is not Clone by design; re-wrap the exposed value once, here.
                password: SecretString::from(secrecy::ExposeSecret::expose_secret(
                    &server.password,
                )),
                secure: server.secure,
            },
            eval,
            source_path: None,
        })
    }

    /// `SCREEPS_EVAL_*` environment overrides (highest precedence).
    /// Optional code path only — the documented mechanism is file-driven
    /// via `.screeps.yaml` (operator directive, P0.A7(a)).
    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("SCREEPS_EVAL_STEAM_KEY") {
            if !v.is_empty() {
                self.eval.steam_key = Some(SecretString::from(v));
            }
        }
        if let Ok(v) = std::env::var("SCREEPS_EVAL_HOST") {
            if !v.is_empty() {
                self.server.host = v;
            }
        }
        if let Ok(v) = std::env::var("SCREEPS_EVAL_PORT") {
            if let Ok(p) = v.parse() {
                self.server.port = p;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PW: &str = "super-secret-test-pw-7391";
    const FAKE_YAML: &str = r#"
servers:
  private-server:
    host: 127.0.0.1
    port: 21025
    username: ibex
    password: super-secret-test-pw-7391
"#;

    /// Like FAKE_YAML but with an `eval:` section carrying a steamKey —
    /// exercises the serde path the real file would take.
    const FAKE_YAML_WITH_EVAL: &str = r#"
servers:
  private-server:
    host: 127.0.0.1
    port: 21025
    username: ibex
    password: super-secret-test-pw-7391
eval:
  serverConfig: C:\some\launcher\config.yml
  steamKey: steam-key-material-001
  ports:
    game: 31025
    cli: 31026
  tickMs: 250
"#;

    /// P0.A7(e): the redaction pin. Debug formatting of the whole
    /// config must never contain password material — `SecretString`
    /// redacts by construction, and this test fails if anyone swaps
    /// it for a plain `String`. Covers `eval.steamKey` (P0.A2).
    #[test]
    fn debug_output_redacts_secrets() {
        let cfg = EvalConfig::from_yaml_str(FAKE_YAML_WITH_EVAL, "private-server").unwrap();
        assert!(
            cfg.eval.steam_key.is_some(),
            "fixture must exercise eval.steamKey"
        );
        let dump = format!("{:?}", cfg);
        assert!(
            !dump.contains(FAKE_PW),
            "password leaked into Debug: {dump}"
        );
        assert!(
            !dump.contains("steam-key-material-001"),
            "steam key leaked into Debug: {dump}"
        );
        // The non-secret fields should still be present/diagnosable.
        assert!(dump.contains("127.0.0.1"));
        assert!(dump.contains("ibex"));
    }

    #[test]
    fn eval_section_parses_fully() {
        let cfg = EvalConfig::from_yaml_str(FAKE_YAML_WITH_EVAL, "private-server").unwrap();
        assert_eq!(
            cfg.eval.server_config.as_deref(),
            Some(std::path::Path::new(r"C:\some\launcher\config.yml"))
        );
        assert!(cfg.eval.steam_key.is_some());
        assert_eq!(cfg.eval.game_port, 31025);
        assert_eq!(cfg.eval.cli_port, 31026);
        assert_eq!(cfg.eval.tick_ms, 250);
    }

    #[test]
    fn absent_eval_section_yields_defaults() {
        let cfg = EvalConfig::from_yaml_str(FAKE_YAML, "private-server").unwrap();
        assert!(cfg.eval.server_config.is_none());
        assert!(cfg.eval.steam_key.is_none());
        assert_eq!(cfg.eval.game_port, DEFAULT_GAME_PORT);
        assert_eq!(cfg.eval.cli_port, DEFAULT_CLI_PORT);
        assert_eq!(cfg.eval.tick_ms, TICK_MS_SMOKE);
    }

    #[test]
    fn partial_eval_section_fills_defaults_and_clamps_tick_floor() {
        let yaml = r#"
servers:
  private-server:
    host: 127.0.0.1
    username: ibex
    password: super-secret-test-pw-7391
eval:
  tickMs: 10
"#;
        let cfg = EvalConfig::from_yaml_str(yaml, "private-server").unwrap();
        assert_eq!(cfg.eval.game_port, DEFAULT_GAME_PORT);
        assert_eq!(cfg.eval.cli_port, DEFAULT_CLI_PORT);
        // 10 ms is below the operator-established floor: clamped, not error.
        assert_eq!(cfg.eval.tick_ms, TICK_MS_FLOOR);
    }

    #[test]
    fn misspelled_eval_key_is_a_clear_error() {
        let yaml = r#"
servers:
  private-server:
    host: 127.0.0.1
    username: ibex
    password: super-secret-test-pw-7391
eval:
  steamkey: oops-wrong-case
"#;
        let err = EvalConfig::from_yaml_str(yaml, "private-server").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("eval"),
            "error should point at the eval section: {msg}"
        );
    }

    #[test]
    fn parses_unified_config_and_defaults() {
        let cfg = EvalConfig::from_yaml_str(FAKE_YAML, "private-server").unwrap();
        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.port, 21025);
        assert_eq!(cfg.server.username, "ibex");
        assert!(!cfg.server.secure);
        assert_eq!(cfg.server.http_base(), "http://127.0.0.1:21025");
    }

    /// Regression: the real .screeps.yaml has an `mmo` entry with a
    /// different shape (token auth, no username). Unrelated entries
    /// must never break parsing of the selected one.
    #[test]
    fn foreign_server_shapes_are_ignored() {
        let yaml = r#"
servers:
  mmo:
    host: screeps.com
    secure: true
    token: not-a-real-token
  private-server:
    host: 127.0.0.1
    port: 21025
    username: ibex
    password: super-secret-test-pw-7391
"#;
        let cfg = EvalConfig::from_yaml_str(yaml, "private-server").unwrap();
        assert_eq!(cfg.server.host, "127.0.0.1");
        // ...and selecting the odd-shaped entry gives a clear error, not a panic.
        let err = EvalConfig::from_yaml_str(yaml, "mmo").unwrap_err();
        assert!(format!("{err:#}").contains("unexpected shape"));
    }

    #[test]
    fn unknown_server_name_is_a_clear_error() {
        let err = EvalConfig::from_yaml_str(FAKE_YAML, "mmo").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("'mmo'"), "unhelpful error: {msg}");
        assert!(
            msg.contains("private-server"),
            "should list known servers: {msg}"
        );
    }

    #[test]
    #[allow(clippy::assertions_on_constants)] // deliberate: this is a pin test
    fn tick_floor_constant_matches_plan() {
        // Plan D-2: floor 50 ms (operator-established), smoke 100, watch 1000.
        assert_eq!(TICK_MS_FLOOR, 50);
        assert!(TICK_MS_SMOKE >= TICK_MS_FLOOR);
        assert!(TICK_MS_WATCH >= TICK_MS_SMOKE);
    }
}
