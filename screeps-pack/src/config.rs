//! `.screeps.yaml` parsing — the SS3 unified credentials file
//! (<https://github.com/screepers/screepers-standards/blob/master/SS3-Unified_Credentials_File.md>)
//! plus the two `configs:` sections the deploy.js pipeline established:
//! `wasm-pack-options` (per-server extra cargo args) is honored as-is
//! for drop-in parity; `terser` is read-and-ignored (screeps-pack does
//! not minify — see the README's size-delta note).
//!
//! ## Pinned semantics (js_tools/deploy.js, read 2026-06-10)
//!
//! - `servers.<name>`: `host`, optional `port`, `secure` (default
//!   false), `branch` (default `"default"`), optional `path` (URL
//!   prefix, e.g. `/ptr`), and EITHER `token` OR `username`+`password`.
//!   Token wins when both exist (screeps-api `fromConfig`,
//!   ScreepsAPI.js:1393: password auth only `if (!conf.token ...)`).
//! - Default port: explicit wins; otherwise 443 when `secure` (the
//!   screeps-api `DEFAULTS`, ScreepsAPI.js:1357-1362), 21025 otherwise
//!   (the private-server default).
//! - `configs.wasm-pack-options`: the `'*'` key's array is
//!   **concatenated** with the per-server array, `'*'` first
//!   (deploy.js:60-69) — these are extra args appended to the cargo
//!   build invocation (wasm-pack passed them through to cargo, and so
//!   do we; a silently-dropped flag is investigation risk #2, so the
//!   passthrough is pinned by tests over the parsed argv).
//!
//! SECRETS (P0.A7 discipline): `token`/`password` live in
//! [`SecretString`] from the moment they are parsed — `Debug` redacts
//! by construction (pinned by a test).

use anyhow::{bail, Context, Result};
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Default game-API port for plain-HTTP (private) servers.
pub const DEFAULT_PLAIN_PORT: u16 = 21025;
/// Default port for `secure: true` servers (screeps-api DEFAULTS).
pub const DEFAULT_SECURE_PORT: u16 = 443;

/// How the selected server entry authenticates.
#[derive(Debug)]
pub enum ServerAuth {
    /// `token:` — official servers (mmo/ptr/season) and any entry that
    /// sets one (token wins over username/password, the screeps-api
    /// `fromConfig` rule).
    Token(SecretString),
    /// `username:` + `password:` — private servers (screepsmod-auth).
    UserPass {
        username: String,
        password: SecretString,
    },
}

/// One resolved `servers:` entry plus its per-server build args.
#[derive(Debug)]
pub struct ServerConfig {
    /// The `servers:` entry name this was resolved from.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub secure: bool,
    /// URL prefix (`path:`, e.g. `/ptr`) — empty when absent.
    pub path: String,
    /// Upload branch (default `"default"`).
    pub branch: String,
    pub auth: ServerAuth,
    /// `configs.wasm-pack-options` resolved for this entry:
    /// `'*'` ++ per-server, passed through to `cargo build` verbatim.
    pub extra_cargo_args: Vec<String>,
}

impl ServerConfig {
    /// `scheme://host:port[/prefix]` — no trailing slash.
    pub fn base_url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{}://{}:{}{}", scheme, self.host, self.port, self.path)
    }
}

// ---- raw YAML shapes ----

#[derive(Deserialize)]
struct RawUnified {
    /// Entries stay lazily-typed so foreign-shaped entries never break
    /// parsing of the one we select (the screeps-server-kit precedent).
    servers: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    configs: RawConfigs,
}

#[derive(Deserialize, Default)]
struct RawConfigs {
    #[serde(rename = "wasm-pack-options", default)]
    wasm_pack_options: BTreeMap<String, Vec<String>>,
    // `terser:` intentionally ignored — screeps-pack does not minify.
}

#[derive(Deserialize)]
struct RawServer {
    host: String,
    port: Option<u16>,
    #[serde(default)]
    secure: bool,
    branch: Option<String>,
    path: Option<String>,
    token: Option<SecretString>,
    username: Option<String>,
    password: Option<SecretString>,
}

/// The deploy.js `'*'`-then-per-server concatenation (deploy.js:60-69),
/// pinned by tests — risk #2 (silent flag loss) lives or dies here.
pub fn resolve_extra_args(
    wasm_pack_options: &BTreeMap<String, Vec<String>>,
    server: &str,
) -> Vec<String> {
    let mut args = wasm_pack_options.get("*").cloned().unwrap_or_default();
    if let Some(per_server) = wasm_pack_options.get(server) {
        args.extend(per_server.iter().cloned());
    }
    args
}

/// Parse the credentials YAML and resolve `server` (pure — I/O-free for
/// testability; tests never read the real, secret-bearing file).
pub fn parse_server_config(creds_yaml: &str, server: &str) -> Result<ServerConfig> {
    let raw: RawUnified = serde_yaml::from_str(creds_yaml).context("parsing .screeps.yaml")?;
    let Some(value) = raw.servers.get(server) else {
        let known: Vec<_> = raw.servers.keys().cloned().collect();
        bail!("server '{server}' not in the credentials file (known entries: {known:?})");
    };
    let entry: RawServer = serde_yaml::from_value(value.clone())
        .with_context(|| format!("server entry '{server}' has an unexpected shape"))?;

    let auth = match (entry.token, entry.username, entry.password) {
        // Token wins over username/password (ScreepsAPI.js:1393).
        (Some(token), _, _) => ServerAuth::Token(token),
        (None, Some(username), Some(password)) => ServerAuth::UserPass { username, password },
        _ => bail!(
            "server entry '{server}' has neither `token` nor `username`+`password` — \
             one auth method is required"
        ),
    };

    let port = entry.port.unwrap_or(if entry.secure {
        DEFAULT_SECURE_PORT
    } else {
        DEFAULT_PLAIN_PORT
    });

    // Normalize the URL prefix: "" or "/prefix" (no trailing slash).
    let path = match entry.path.as_deref() {
        None | Some("") | Some("/") => String::new(),
        Some(p) => {
            let trimmed = p.trim_end_matches('/');
            if trimmed.starts_with('/') {
                trimmed.to_string()
            } else {
                format!("/{trimmed}")
            }
        }
    };

    Ok(ServerConfig {
        name: server.to_string(),
        host: entry.host,
        port,
        secure: entry.secure,
        path,
        branch: entry.branch.unwrap_or_else(|| "default".to_string()),
        auth,
        extra_cargo_args: resolve_extra_args(&raw.configs.wasm_pack_options, server),
    })
}

/// [`parse_server_config`] over a file on disk.
pub fn load_server_config(creds_path: &Path, server: &str) -> Result<ServerConfig> {
    let raw = std::fs::read_to_string(creds_path).with_context(|| {
        format!(
            "reading credentials file {} (an SS3 .screeps.yaml; pass --creds to point \
             elsewhere)",
            creds_path.display()
        )
    })?;
    parse_server_config(&raw, server)
        .with_context(|| format!("resolving server '{server}' from {}", creds_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PW: &str = "super-secret-test-pw-7391";
    const FAKE_TOKEN: &str = "ffffffff-aaaa-bbbb-cccc-fake-token-material";

    /// The repo's .example-screeps.yaml `configs:` section, verbatim —
    /// THE flag-passthrough fixture (investigation risk #2).
    const EXAMPLE_YAML: &str = r#"
servers:
  mmo:
    host: screeps.com
    secure: true
    token: ffffffff-aaaa-bbbb-cccc-fake-token-material
    branch: default
  ptr:
    host: screeps.com
    secure: true
    token: ffffffff-aaaa-bbbb-cccc-fake-token-material
    path: /ptr
    branch: default
  private-server:
    host: 127.0.0.1
    port: 21025
    secure: false
    username: ibex
    password: super-secret-test-pw-7391
    branch: default
configs:
  terser:
    '*': false
    ptr: false
  wasm-pack-options:
    '*': ["--config", "build.rustflags=['-Ctarget-cpu=mvp']", "-Z", "build-std=std,panic_abort"]
    mmo: ["--features", "mmo"]
    ptr: ["--features", "mmo"]
"#;

    /// THE FLAG-PASSTHROUGH PIN: '*' args survive verbatim and the
    /// per-server feature flags are concatenated AFTER them — exactly
    /// deploy.js:60-69. A silently-dropped flag here ships wasm that
    /// older server V8s reject (risk #2).
    #[test]
    fn wasm_pack_options_concatenate_star_then_server() {
        let mmo = parse_server_config(EXAMPLE_YAML, "mmo").unwrap();
        assert_eq!(
            mmo.extra_cargo_args,
            [
                "--config",
                "build.rustflags=['-Ctarget-cpu=mvp']",
                "-Z",
                "build-std=std,panic_abort",
                "--features",
                "mmo",
            ]
        );
        // No per-server entry -> '*' only, still verbatim.
        let private = parse_server_config(EXAMPLE_YAML, "private-server").unwrap();
        assert_eq!(
            private.extra_cargo_args,
            [
                "--config",
                "build.rustflags=['-Ctarget-cpu=mvp']",
                "-Z",
                "build-std=std,panic_abort",
            ]
        );
    }

    /// Missing `configs:` entirely -> empty extra args, not an error.
    #[test]
    fn missing_configs_section_means_no_extra_args() {
        let yaml = "servers:\n  s:\n    host: h\n    username: u\n    password: p\n";
        let cfg = parse_server_config(yaml, "s").unwrap();
        assert!(cfg.extra_cargo_args.is_empty());
        assert_eq!(cfg.branch, "default");
    }

    /// Port defaults pinned from screeps-api DEFAULTS (443 for secure)
    /// and the private-server convention (21025 plain); explicit wins.
    #[test]
    fn port_and_url_defaults() {
        let mmo = parse_server_config(EXAMPLE_YAML, "mmo").unwrap();
        assert_eq!(mmo.port, 443);
        assert_eq!(mmo.base_url(), "https://screeps.com:443");

        let ptr = parse_server_config(EXAMPLE_YAML, "ptr").unwrap();
        assert_eq!(ptr.base_url(), "https://screeps.com:443/ptr");

        let private = parse_server_config(EXAMPLE_YAML, "private-server").unwrap();
        assert_eq!(private.port, 21025);
        assert!(!private.secure);
        assert_eq!(private.base_url(), "http://127.0.0.1:21025");
    }

    /// Token wins over username/password (ScreepsAPI.js:1393).
    #[test]
    fn token_wins_over_userpass() {
        let yaml = format!(
            "servers:\n  both:\n    host: h\n    token: {FAKE_TOKEN}\n    username: u\n    password: {FAKE_PW}\n"
        );
        let cfg = parse_server_config(&yaml, "both").unwrap();
        assert!(matches!(cfg.auth, ServerAuth::Token(_)));
    }

    #[test]
    fn missing_auth_is_a_clear_error() {
        let yaml = "servers:\n  bare:\n    host: h\n";
        let err = parse_server_config(yaml, "bare").unwrap_err();
        assert!(format!("{err:#}").contains("auth"), "got: {err:#}");
    }

    #[test]
    fn unknown_server_lists_known_entries() {
        let err = parse_server_config(EXAMPLE_YAML, "nope").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("'nope'"));
        assert!(msg.contains("private-server"), "should list entries: {msg}");
    }

    /// P0.A7 redaction pin: Debug of a resolved config must never
    /// contain token/password material.
    #[test]
    fn debug_redacts_secrets() {
        for entry in ["mmo", "private-server"] {
            let cfg = parse_server_config(EXAMPLE_YAML, entry).unwrap();
            let dump = format!("{cfg:?}");
            assert!(!dump.contains(FAKE_TOKEN), "token leaked: {dump}");
            assert!(!dump.contains(FAKE_PW), "password leaked: {dump}");
        }
    }

    /// Foreign-shaped sibling entries must never break the selected one.
    #[test]
    fn foreign_entries_are_tolerated() {
        let yaml = r#"
servers:
  weird:
    completely: [different, shape]
  s:
    host: h
    username: u
    password: p
"#;
        assert!(parse_server_config(yaml, "s").is_ok());
        assert!(parse_server_config(yaml, "weird").is_err());
    }
}
