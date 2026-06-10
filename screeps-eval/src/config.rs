//! Configuration loading for the eval harness — file-driven, FIXED paths
//! (P0.A9; no environment variables, no directory walking).
//!
//! Two files, two concerns:
//! - **`../.screeps.yaml`** (repo root, gitignored) — server CREDENTIALS
//!   only (`servers:` entries; the same unified file deploy.js and
//!   screeps-prospector read). `--config` overrides this path and is the
//!   only override. Any other top-level section (`configs:`, a leftover
//!   `eval:`) is ignored.
//! - **`config/local.yml`** (crate-local, gitignored via this crate's
//!   `.gitignore`) — eval-stack settings: `steamKey`, `ports`, `tickMs`,
//!   `spawn`, `bots` (P0.A10), `image` (P0.A9(d)). The committed
//!   `config/local.example.yml` documents every key. Absent file = all
//!   defaults (commands that need the steamKey fail with a pointer to
//!   the example).
//!
//! WHY `CARGO_MANIFEST_DIR` (not the invocation cwd) anchors the fixed
//! paths: `server_config::runtime_dir()` already anchors crate-relative
//! artifacts there, the crate is always driven via `cargo run` while it
//! lives in-repo (decision D-1) so the compile-time path is valid, and
//! config resolution must not silently change (or vanish) when a command
//! runs from an unexpected directory — the operator convention is still
//! "cd screeps-eval first" (the child `.cargo/config.toml` target pin is
//! cwd-discovered), but a cwd mistake now fails at the build-target
//! level, never by reading the wrong config. Revisit at extraction,
//! together with `runtime_dir()`.
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

/// Name of the default bot entry in `.screeps.yaml` (`servers:` map) —
/// also the default `bots:` list (P0.A10 compat).
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

/// Default launcher image — pulled from the registry (P0.A2 image
/// policy; override or build locally via the `image:` block, P0.A9(d)).
pub const DEFAULT_LAUNCHER_IMAGE: &str = "screepers/screeps-launcher:latest";

/// The crate root at compile time — the anchor for every fixed path
/// (see the module docs for why cwd is not used).
pub fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Fixed credentials path: the repo-root `.screeps.yaml`
/// (`<crate>/../.screeps.yaml`). `--config` is the only override.
pub fn default_creds_path() -> PathBuf {
    crate_dir().join("..").join(".screeps.yaml")
}

/// Fixed local-settings path: `<crate>/config/local.yml` (gitignored;
/// documented by the committed `config/local.example.yml`).
pub fn local_config_path() -> PathBuf {
    crate_dir().join("config").join("local.yml")
}

/// Top-level config consumed by the harness and the CLI.
#[derive(Debug)]
pub struct EvalConfig {
    /// The `servers:` entry selected by `--server-name` — the identity
    /// server-level commands (`run`, `smoke`, default `deploy`) act as.
    pub server: ServerEndpoint,
    /// One resolved endpoint per `bots:` entry (P0.A10) — `bootstrap`
    /// registers and places a spawn for each.
    pub bots: Vec<BotEndpoint>,
    /// Eval-stack settings from `config/local.yml` (defaults if absent).
    pub eval: EvalSettings,
    /// Where the credentials file was found (also locates the repo root
    /// for `deploy`/`runs/`).
    pub source_path: Option<PathBuf>,
}

/// A named bot identity (P0.A10): one `servers:` entry per bot user.
#[derive(Debug)]
pub struct BotEndpoint {
    /// The `servers:` entry name (also `deploy --user <name>`).
    pub name: String,
    pub endpoint: ServerEndpoint,
}

/// The `config/local.yml` settings — file-driven server-stack
/// configuration (P0.A2/A9; no host env vars, operator directive).
#[derive(Debug)]
pub struct EvalSettings {
    /// Steam Web API key, merged into the launcher config at runtime.
    /// Lives in the merged runtime config under `target/runtime/`
    /// (gitignored) — never in the committed template, never in logs.
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
    /// Spawn-placement preference for `bootstrap` (P0.A3), applied to
    /// the FIRST `bots:` entry only (later bots auto-pick a distinct
    /// room). All fields optional: room alone = auto-pick a tile in
    /// that room; room+x+y = exact placement (no fallback); nothing =
    /// auto-pick room and tile.
    pub spawn: SpawnPreference,
    /// Bot identities to bootstrap (P0.A10): names of `servers:`
    /// entries in `.screeps.yaml`. Default `["private-server"]`.
    pub bots: Vec<String>,
    /// Launcher image policy (P0.A9(d)): pull by default, optionally
    /// build from a launcher-repo clone.
    pub image: ImageSettings,
}

/// The optional `spawn:` section — where `bootstrap` places the first
/// bot's spawn. `x`/`y` are only honored together with `room`.
#[derive(Debug, Default, Clone)]
pub struct SpawnPreference {
    pub room: Option<String>,
    pub x: Option<u32>,
    pub y: Option<u32>,
}

/// The optional `image:` block (P0.A9(d)).
#[derive(Debug, Clone)]
pub struct ImageSettings {
    /// Image name/tag for the launcher container. Default
    /// [`DEFAULT_LAUNCHER_IMAGE`] (registry pull).
    pub name: String,
    /// When set, the image is BUILT from this context instead of pulled
    /// (`server build-image`, or automatically by `server up` when the
    /// image is absent).
    pub build: Option<ImageBuild>,
}

/// A Docker build context for the launcher image. The context must be a
/// FULL clone of the upstream screepers/screeps-launcher repository —
/// its Dockerfile lives at the repo root (a config-only directory like
/// a compose checkout is not buildable; see the README troubleshooting).
#[derive(Debug, Clone)]
pub struct ImageBuild {
    pub context: PathBuf,
    /// Dockerfile name within the context. Default `Dockerfile`.
    pub dockerfile: Option<String>,
}

impl ImageBuild {
    pub fn dockerfile_name(&self) -> &str {
        self.dockerfile.as_deref().unwrap_or("Dockerfile")
    }
}

impl Default for ImageSettings {
    fn default() -> Self {
        ImageSettings {
            name: DEFAULT_LAUNCHER_IMAGE.to_string(),
            build: None,
        }
    }
}

impl Default for EvalSettings {
    fn default() -> Self {
        EvalSettings {
            steam_key: None,
            game_port: DEFAULT_GAME_PORT,
            cli_port: DEFAULT_CLI_PORT,
            tick_ms: TICK_MS_SMOKE,
            spawn: SpawnPreference::default(),
            bots: vec![DEFAULT_SERVER_NAME.to_string()],
            image: ImageSettings::default(),
        }
    }
}

/// One server entry resolved from `.screeps.yaml`.
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
    /// entries we actually select. Unknown top-level sections
    /// (`configs:`, a leftover `eval:`) are ignored by default serde —
    /// the harness no longer reads anything but `servers:` here.
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
    DEFAULT_GAME_PORT
}

impl RawServer {
    fn into_endpoint(self) -> ServerEndpoint {
        ServerEndpoint {
            host: self.host,
            port: self.port,
            username: self.username,
            password: self.password,
            secure: self.secure,
        }
    }
}

// ---- raw config/local.yml shape ----

/// Raw `config/local.yml` shape — key names match
/// `config/local.example.yml`. `deny_unknown_fields` so a typo'd key is
/// a clear error, not a silently-ignored setting.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawLocal {
    steam_key: Option<SecretString>,
    #[serde(default)]
    ports: RawLocalPorts,
    tick_ms: Option<u64>,
    #[serde(default)]
    spawn: RawLocalSpawn,
    bots: Option<Vec<String>>,
    image: Option<RawLocalImage>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLocalPorts {
    game: Option<u16>,
    cli: Option<u16>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLocalSpawn {
    room: Option<String>,
    x: Option<u32>,
    y: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLocalImage {
    name: Option<String>,
    build: Option<RawLocalImageBuild>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLocalImageBuild {
    context: PathBuf,
    dockerfile: Option<String>,
}

impl From<RawLocal> for EvalSettings {
    fn from(raw: RawLocal) -> Self {
        let defaults = EvalSettings::default();
        let tick_ms = raw.tick_ms.unwrap_or(defaults.tick_ms);
        EvalSettings {
            steam_key: raw.steam_key,
            game_port: raw.ports.game.unwrap_or(defaults.game_port),
            cli_port: raw.ports.cli.unwrap_or(defaults.cli_port),
            // Clamp to the operator-established floor (plan D-2) rather
            // than erroring: the config is long-lived, and a too-low
            // value is a tuning mistake, not a corruption.
            tick_ms: tick_ms.max(TICK_MS_FLOOR),
            spawn: SpawnPreference {
                room: raw.spawn.room,
                x: raw.spawn.x,
                y: raw.spawn.y,
            },
            bots: raw.bots.unwrap_or(defaults.bots),
            image: match raw.image {
                None => defaults.image,
                Some(image) => ImageSettings {
                    name: image
                        .name
                        .unwrap_or_else(|| DEFAULT_LAUNCHER_IMAGE.to_string()),
                    build: image.build.map(|b| ImageBuild {
                        context: b.context,
                        dockerfile: b.dockerfile,
                    }),
                },
            },
        }
    }
}

impl EvalSettings {
    /// Parse `config/local.yml` content. An empty/whitespace-only file
    /// (or, at the caller, an absent one) yields all defaults.
    pub fn from_local_yaml_str(yaml: &str) -> Result<Self> {
        if yaml.trim().is_empty() {
            return Ok(EvalSettings::default());
        }
        let raw: RawLocal =
            serde_yaml::from_str(yaml).context("config/local.yml has an unexpected shape")?;
        Ok(EvalSettings::from(raw))
    }
}

impl EvalConfig {
    /// Load from the fixed paths: credentials at `../.screeps.yaml`
    /// (or `explicit`, the only override) + settings at
    /// `config/local.yml` (optional). `server_name` selects the
    /// `servers:` entry the harness acts as.
    pub fn load(explicit: Option<&Path>, server_name: &str) -> Result<Self> {
        let creds_path = explicit
            .map(Path::to_path_buf)
            .unwrap_or_else(default_creds_path);
        let creds_raw = std::fs::read_to_string(&creds_path).with_context(|| {
            format!(
                "reading credentials file {} (fixed path: ../.screeps.yaml next to this \
                 crate; --config is the only override — create one from .example-screeps.yaml)",
                creds_path.display()
            )
        })?;

        let local_path = local_config_path();
        let eval = match std::fs::read_to_string(&local_path) {
            Ok(raw) => EvalSettings::from_local_yaml_str(&raw)
                .with_context(|| format!("parsing {}", local_path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => EvalSettings::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", local_path.display())),
        };

        let mut cfg = Self::from_parts(&creds_raw, eval, server_name)
            .with_context(|| format!("parsing {}", creds_path.display()))?;
        cfg.source_path = Some(creds_path);
        Ok(cfg)
    }

    /// Assemble from already-parsed pieces (separated from I/O for
    /// testability — tests construct configs from literals, never from
    /// the real, secret-bearing files).
    pub fn from_parts(creds_yaml: &str, eval: EvalSettings, server_name: &str) -> Result<Self> {
        let raw: RawUnifiedConfig = serde_yaml::from_str(creds_yaml)?;
        let server = resolve_entry(&raw, server_name)?;
        let bots = eval
            .bots
            .iter()
            .map(|name| {
                let endpoint = resolve_entry(&raw, name).with_context(|| {
                    format!(
                        "resolving bots entry '{name}' (config/local.yml `bots:` names \
                         must be `servers:` entries with username+password)"
                    )
                })?;
                Ok(BotEndpoint {
                    name: name.clone(),
                    endpoint,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(EvalConfig {
            server,
            bots,
            eval,
            source_path: None,
        })
    }

    /// Test/convenience constructor from two YAML literals.
    pub fn from_yaml_strs(
        creds_yaml: &str,
        local_yaml: Option<&str>,
        server_name: &str,
    ) -> Result<Self> {
        let eval = match local_yaml {
            Some(yaml) => EvalSettings::from_local_yaml_str(yaml)?,
            None => EvalSettings::default(),
        };
        Self::from_parts(creds_yaml, eval, server_name)
    }
}

fn resolve_entry(raw: &RawUnifiedConfig, server_name: &str) -> Result<ServerEndpoint> {
    let Some(value) = raw.servers.get(server_name) else {
        let known: Vec<_> = raw.servers.keys().cloned().collect();
        bail!("server '{server_name}' not in .screeps.yaml (known: {known:?})");
    };
    let server: RawServer = serde_yaml::from_value(value.clone())
        .with_context(|| format!("server entry '{server_name}' has an unexpected shape"))?;
    Ok(server.into_endpoint())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PW: &str = "super-secret-test-pw-7391";
    const FAKE_KEY: &str = "steam-key-material-001";
    const FAKE_CREDS: &str = r#"
servers:
  private-server:
    host: 127.0.0.1
    port: 21025
    username: ibex
    password: super-secret-test-pw-7391
"#;

    /// Multi-entry credentials fixture: two bot entries + a token-auth
    /// official entry that must never break parsing (P0.A10).
    const FAKE_CREDS_MULTI: &str = r#"
servers:
  mmo:
    host: screeps.com
    secure: true
    token: not-a-real-token
  ibex:
    host: 127.0.0.1
    port: 21025
    username: ibex
    password: super-secret-test-pw-7391
  ibex-2:
    host: 127.0.0.1
    port: 21025
    username: ibex-2
    password: super-secret-test-pw-7391
  private-server:
    host: 127.0.0.1
    port: 21025
    username: ibex
    password: super-secret-test-pw-7391
"#;

    /// A full config/local.yml exercising every key.
    const FAKE_LOCAL_FULL: &str = r#"
steamKey: steam-key-material-001
ports:
  game: 31025
  cli: 31026
tickMs: 250
spawn:
  room: W5N3
  x: 18
  y: 14
bots:
  - ibex
  - ibex-2
image:
  name: screepers/screeps-launcher:local
  build:
    context: C:\some\launcher-clone
    dockerfile: Dockerfile.custom
"#;

    /// P0.A7(e): the redaction pin. Debug formatting of the whole
    /// config must never contain password or steamKey material —
    /// `SecretString` redacts by construction, and this test fails if
    /// anyone swaps it for a plain `String`.
    #[test]
    fn debug_output_redacts_secrets() {
        let cfg =
            EvalConfig::from_yaml_strs(FAKE_CREDS_MULTI, Some(FAKE_LOCAL_FULL), "private-server")
                .unwrap();
        assert!(
            cfg.eval.steam_key.is_some(),
            "fixture must exercise steamKey"
        );
        assert_eq!(cfg.bots.len(), 2, "fixture must exercise bot endpoints");
        let dump = format!("{:?}", cfg);
        assert!(
            !dump.contains(FAKE_PW),
            "password leaked into Debug: {dump}"
        );
        assert!(
            !dump.contains(FAKE_KEY),
            "steam key leaked into Debug: {dump}"
        );
        // The non-secret fields should still be present/diagnosable.
        assert!(dump.contains("127.0.0.1"));
        assert!(dump.contains("ibex"));
    }

    #[test]
    fn local_yaml_parses_fully() {
        let eval = EvalSettings::from_local_yaml_str(FAKE_LOCAL_FULL).unwrap();
        assert!(eval.steam_key.is_some());
        assert_eq!(eval.game_port, 31025);
        assert_eq!(eval.cli_port, 31026);
        assert_eq!(eval.tick_ms, 250);
        assert_eq!(eval.spawn.room.as_deref(), Some("W5N3"));
        assert_eq!(eval.spawn.x, Some(18));
        assert_eq!(eval.spawn.y, Some(14));
        assert_eq!(eval.bots, vec!["ibex", "ibex-2"]);
        assert_eq!(eval.image.name, "screepers/screeps-launcher:local");
        let build = eval.image.build.as_ref().unwrap();
        assert_eq!(build.context, PathBuf::from(r"C:\some\launcher-clone"));
        assert_eq!(build.dockerfile_name(), "Dockerfile.custom");
    }

    /// Absent file (None) and empty file ("") both mean defaults.
    #[test]
    fn absent_or_empty_local_yaml_yields_defaults() {
        for eval in [
            EvalSettings::from_local_yaml_str("").unwrap(),
            EvalSettings::from_local_yaml_str("  \n").unwrap(),
            EvalConfig::from_yaml_strs(FAKE_CREDS, None, "private-server")
                .unwrap()
                .eval,
        ] {
            assert!(eval.steam_key.is_none());
            assert_eq!(eval.game_port, DEFAULT_GAME_PORT);
            assert_eq!(eval.cli_port, DEFAULT_CLI_PORT);
            assert_eq!(eval.tick_ms, TICK_MS_SMOKE);
            assert!(eval.spawn.room.is_none());
            // P0.A10 compat: the default bots list is the historical
            // single entry.
            assert_eq!(eval.bots, vec![DEFAULT_SERVER_NAME]);
            assert_eq!(eval.image.name, DEFAULT_LAUNCHER_IMAGE);
            assert!(eval.image.build.is_none());
        }
    }

    #[test]
    fn partial_local_yaml_fills_defaults_and_clamps_tick_floor() {
        let eval = EvalSettings::from_local_yaml_str("tickMs: 10\n").unwrap();
        assert_eq!(eval.game_port, DEFAULT_GAME_PORT);
        assert_eq!(eval.cli_port, DEFAULT_CLI_PORT);
        // 10 ms is below the operator-established floor: clamped, not error.
        assert_eq!(eval.tick_ms, TICK_MS_FLOOR);
        assert_eq!(eval.bots, vec![DEFAULT_SERVER_NAME]);
    }

    /// Image block defaults: a bare `build:` keeps the default image
    /// name and the default Dockerfile name.
    #[test]
    fn image_block_defaults() {
        let eval =
            EvalSettings::from_local_yaml_str("image:\n  build:\n    context: /tmp/launcher\n")
                .unwrap();
        assert_eq!(eval.image.name, DEFAULT_LAUNCHER_IMAGE);
        let build = eval.image.build.as_ref().unwrap();
        assert_eq!(build.dockerfile_name(), "Dockerfile");

        // Name-only override (no build): still a pull, different tag.
        let eval = EvalSettings::from_local_yaml_str("image:\n  name: my/launcher:dev\n").unwrap();
        assert_eq!(eval.image.name, "my/launcher:dev");
        assert!(eval.image.build.is_none());
    }

    #[test]
    fn misspelled_local_key_is_a_clear_error() {
        let err = EvalSettings::from_local_yaml_str("steamkey: oops-wrong-case\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("local.yml"),
            "error should point at the file: {msg}"
        );
    }

    #[test]
    fn parses_creds_and_endpoint_defaults() {
        let cfg = EvalConfig::from_yaml_strs(FAKE_CREDS, None, "private-server").unwrap();
        assert_eq!(cfg.server.host, "127.0.0.1");
        assert_eq!(cfg.server.port, 21025);
        assert_eq!(cfg.server.username, "ibex");
        assert!(!cfg.server.secure);
        assert_eq!(cfg.server.http_base(), "http://127.0.0.1:21025");
        // Default bots list resolves against the same entry.
        assert_eq!(cfg.bots.len(), 1);
        assert_eq!(cfg.bots[0].name, DEFAULT_SERVER_NAME);
        assert_eq!(cfg.bots[0].endpoint.username, "ibex");
    }

    /// P0.A10: each `bots:` entry resolves to its own endpoint.
    #[test]
    fn bots_resolve_to_distinct_endpoints() {
        let cfg = EvalConfig::from_yaml_strs(
            FAKE_CREDS_MULTI,
            Some("bots:\n  - ibex\n  - ibex-2\n"),
            "private-server",
        )
        .unwrap();
        assert_eq!(cfg.bots.len(), 2);
        assert_eq!(cfg.bots[0].name, "ibex");
        assert_eq!(cfg.bots[0].endpoint.username, "ibex");
        assert_eq!(cfg.bots[1].name, "ibex-2");
        assert_eq!(cfg.bots[1].endpoint.username, "ibex-2");
    }

    /// A bots entry that is missing — or token-shaped (no
    /// username/password) — is a clear, named error.
    #[test]
    fn bad_bots_entries_are_clear_errors() {
        let err = EvalConfig::from_yaml_strs(
            FAKE_CREDS_MULTI,
            Some("bots: [nonexistent]\n"),
            "private-server",
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("'nonexistent'"), "unhelpful error: {msg}");
        assert!(msg.contains("bots"), "should blame the bots list: {msg}");

        let err =
            EvalConfig::from_yaml_strs(FAKE_CREDS_MULTI, Some("bots: [mmo]\n"), "private-server")
                .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unexpected shape"), "got: {msg}");
        assert!(msg.contains("'mmo'"), "should name the entry: {msg}");
    }

    /// Regression: the real .screeps.yaml has an `mmo` entry with a
    /// different shape (token auth, no username) and may still carry a
    /// leftover `eval:` section (no longer read — P0.A9(b)). Neither
    /// must break parsing of the selected entry.
    #[test]
    fn foreign_shapes_and_leftover_eval_section_are_ignored() {
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
configs:
  terser:
    '*': false
eval:
  serverConfig: C:\stale\launcher\config.yml
  tickMs: 9999
"#;
        let cfg = EvalConfig::from_yaml_strs(yaml, None, "private-server").unwrap();
        assert_eq!(cfg.server.host, "127.0.0.1");
        // The leftover eval: section is IGNORED — settings come from
        // config/local.yml (here: defaults), not from .screeps.yaml.
        assert_eq!(cfg.eval.tick_ms, TICK_MS_SMOKE);
        // ...and selecting the odd-shaped entry gives a clear error, not a panic.
        let err = EvalConfig::from_yaml_strs(yaml, None, "mmo").unwrap_err();
        assert!(format!("{err:#}").contains("unexpected shape"));
    }

    #[test]
    fn unknown_server_name_is_a_clear_error() {
        let err = EvalConfig::from_yaml_strs(FAKE_CREDS, None, "mmo").unwrap_err();
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

    /// P0.A9(c): the fixed paths anchor at the crate, not the cwd.
    #[test]
    fn fixed_paths_anchor_at_the_crate_dir() {
        assert!(default_creds_path().starts_with(crate_dir()));
        assert!(default_creds_path().ends_with(".screeps.yaml"));
        let local = local_config_path();
        assert!(local.starts_with(crate_dir()));
        assert!(local.ends_with(Path::new("config").join("local.yml")));
        // The committed example documenting local.yml must exist.
        assert!(
            crate_dir()
                .join("config")
                .join("local.example.yml")
                .is_file(),
            "config/local.example.yml is part of the contract"
        );
    }
}
