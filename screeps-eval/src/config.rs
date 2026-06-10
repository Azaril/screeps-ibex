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

/// Top-level config consumed by the harness and the CLI.
#[derive(Debug)]
pub struct EvalConfig {
    /// API/game endpoint of the private server (host, port, creds).
    pub server: ServerEndpoint,
    /// Steam API key injected into the launcher container as env
    /// (`STEAM_KEY`). Never written into the vendored config template.
    pub steam_key: Option<SecretString>,
    /// Where `.screeps.yaml` was found (diagnostics only).
    pub source_path: Option<PathBuf>,
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
        let server = &server;
        Ok(EvalConfig {
            server: ServerEndpoint {
                host: server.host.clone(),
                port: server.port,
                username: server.username.clone(),
                // SecretString is not Clone by design; re-wrap the exposed value once, here.
                password: SecretString::from(secrecy::ExposeSecret::expose_secret(&server.password)),
                secure: server.secure,
            },
            steam_key: None,
            source_path: None,
        })
    }

    /// `SCREEPS_EVAL_*` environment overrides (highest precedence).
    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("SCREEPS_EVAL_STEAM_KEY") {
            if !v.is_empty() {
                self.steam_key = Some(SecretString::from(v));
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

    /// P0.A7(e): the redaction pin. Debug formatting of the whole
    /// config must never contain password material — `SecretString`
    /// redacts by construction, and this test fails if anyone swaps
    /// it for a plain `String`.
    #[test]
    fn debug_output_redacts_secrets() {
        let mut cfg = EvalConfig::from_yaml_str(FAKE_YAML, "private-server").unwrap();
        cfg.steam_key = Some(SecretString::from("steam-key-material-001"));
        let dump = format!("{:?}", cfg);
        assert!(!dump.contains(FAKE_PW), "password leaked into Debug: {dump}");
        assert!(
            !dump.contains("steam-key-material-001"),
            "steam key leaked into Debug: {dump}"
        );
        // The non-secret fields should still be present/diagnosable.
        assert!(dump.contains("127.0.0.1"));
        assert!(dump.contains("ibex"));
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
        assert!(msg.contains("private-server"), "should list known servers: {msg}");
    }

    #[test]
    fn tick_floor_constant_matches_plan() {
        // Plan D-2: floor 50 ms (operator-established), smoke 100, watch 1000.
        assert_eq!(TICK_MS_FLOOR, 50);
        assert!(TICK_MS_SMOKE >= TICK_MS_FLOOR);
        assert!(TICK_MS_WATCH >= TICK_MS_SMOKE);
    }
}
