//! Configuration loading for the prospector.
//!
//! Fixed-path pattern (the P0.A9(c) precedent): the crate is always
//! invoked from its own directory, so credentials default to
//! `../.screeps.yaml` (the same file the deploy tooling and screeps-server-kit
//! read) and `--config` is the only override. No directory walking, no
//! environment variables (operator directive).
//!
//! SECRETS POLICY (Phase 0 plan P0.A7, applied to Workstream P): every
//! credential lives in a [`SecretString`] from the moment it is parsed —
//! `Debug`/`Display` redact by construction, so secrets cannot reach
//! logs through formatting. The credential file is read AT RUNTIME only;
//! tests construct configs from literal fixtures, never from the real
//! `.screeps.yaml`.

use anyhow::{bail, Context, Result};
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default `servers:` entry in `.screeps.yaml`.
pub const DEFAULT_SERVER_NAME: &str = "private-server";

/// Default credentials file, relative to the crate directory (the crate
/// is always invoked from its own directory — fixed-path rule).
pub const DEFAULT_CONFIG_PATH: &str = "../.screeps.yaml";

/// How the client authenticates — the shared client's type, re-exported
/// so config consumers keep one import. Selection rule (this crate's):
/// official servers support token auth only; when a `servers:` entry
/// carries both `token:` and `username:`+`password:`, the token wins
/// (it is the only method that works everywhere it appears).
pub use screeps_rest_api::AuthMode;

/// Resolved configuration for one `servers:` entry.
#[derive(Debug)]
pub struct ProspectorConfig {
    /// The `servers:` entry name this was resolved from.
    pub server_name: String,
    /// `scheme://host[:port][/path]` — ready to prefix `/api/...` paths.
    pub base_url: String,
    pub auth: AuthMode,
    /// Where the config file was found (diagnostics only).
    pub source_path: Option<PathBuf>,
}

// ---- raw .screeps.yaml shape (only the fields we consume) ----

#[derive(Deserialize)]
struct RawUnifiedConfig {
    /// Entries stay lazily-typed: other entries (private-server uses
    /// user/pass, mmo uses token, season has `path:`) must not break
    /// parsing of the one entry we actually select. Unknown top-level
    /// sections (`eval:`, `configs:`) are ignored by default serde.
    servers: HashMap<String, serde_yaml::Value>,
}

#[derive(Deserialize)]
struct RawServer {
    host: String,
    port: Option<u16>,
    #[serde(default)]
    secure: bool,
    /// URL prefix for official sub-realms (e.g. `/ptr`, `/season`).
    path: Option<String>,
    username: Option<String>,
    password: Option<SecretString>,
    token: Option<SecretString>,
}

impl ProspectorConfig {
    /// Load from an explicit path or the fixed default
    /// (`../.screeps.yaml` from the crate directory). `server_name`
    /// selects the `servers:` entry.
    pub fn load(explicit: Option<&Path>, server_name: &str) -> Result<Self> {
        let path = explicit
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "reading {} (the fixed default is {DEFAULT_CONFIG_PATH} relative to the \
                 crate directory; pass --config to override)",
                path.display()
            )
        })?;
        let mut cfg = Self::from_yaml_str(&raw, server_name)
            .with_context(|| format!("parsing {}", path.display()))?;
        cfg.source_path = Some(path);
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

        let auth = match (server.token, server.username, server.password) {
            // Token wins when both are present: official servers accept
            // only tokens, and a token also works on private servers.
            (Some(token), _, _) => AuthMode::Token(token),
            (None, Some(username), Some(password)) => AuthMode::UserPass { username, password },
            _ => bail!(
                "server '{server_name}' needs either `token:` or `username:`+`password:` \
                 (official servers support token auth only)"
            ),
        };

        Ok(ProspectorConfig {
            server_name: server_name.to_owned(),
            base_url: build_base_url(
                &server.host,
                server.port,
                server.secure,
                server.path.as_deref(),
            ),
            auth,
            source_path: None,
        })
    }

    /// MMO-safety classification (P0.P4): a server entry is treated as
    /// OFFICIAL when it authenticates by token (official servers are
    /// token-only, and a token entry pointed anywhere deserves the same
    /// caution) OR its URL targets screeps.com. Official entries refuse
    /// `auto` outright and require `--i-understand-this-is-mmo` for
    /// `place` — see [`crate::place`]. The rule itself lives in the
    /// shared client ([`screeps_rest_api::is_official_target`]), where
    /// it also engages per-endpoint quota pacing — one classification,
    /// both consumers.
    pub fn is_official(&self) -> bool {
        screeps_rest_api::is_official_target(&self.base_url, &self.auth)
    }
}

/// Assemble `scheme://host[:port][/path]`. The scheme-default ports
/// (443/https, 80/http) are omitted for clean URLs; private servers
/// default to 21025 when no port is given and `secure` is false.
fn build_base_url(host: &str, port: Option<u16>, secure: bool, path: Option<&str>) -> String {
    let scheme = if secure { "https" } else { "http" };
    let default_port = if secure { 443 } else { 21025 };
    let port = port.unwrap_or(default_port);
    let scheme_default = if secure { 443 } else { 80 };
    let mut url = if port == scheme_default {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    };
    if let Some(p) = path {
        let trimmed = p.trim_end_matches('/');
        if !trimmed.is_empty() {
            if !trimmed.starts_with('/') {
                url.push('/');
            }
            url.push_str(trimmed);
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PW: &str = "super-secret-test-pw-7391";
    const FAKE_TOKEN: &str = "ffffffff-aaaa-bbbb-cccc-fake-token-material-0042";
    const FAKE_YAML: &str = r#"
servers:
  mmo:
    host: screeps.com
    secure: true
    token: ffffffff-aaaa-bbbb-cccc-fake-token-material-0042
    branch: default
  season:
    host: screeps.com
    secure: true
    token: ffffffff-aaaa-bbbb-cccc-fake-token-material-0042
    path: /season
  private-server:
    host: 127.0.0.1
    port: 21025
    secure: false
    username: ibex
    password: super-secret-test-pw-7391
    branch: default
"#;

    /// The redaction pin (P0.A7(e) pattern): Debug formatting of the
    /// whole config must never contain credential material —
    /// `SecretString` redacts by construction, and this test fails if
    /// anyone swaps it for a plain `String`. Covers BOTH auth modes.
    #[test]
    fn debug_output_redacts_secrets() {
        let pw_cfg = ProspectorConfig::from_yaml_str(FAKE_YAML, "private-server").unwrap();
        let dump = format!("{pw_cfg:?}");
        assert!(
            !dump.contains(FAKE_PW),
            "password leaked into Debug: {dump}"
        );
        // Non-secret fields stay diagnosable.
        assert!(dump.contains("127.0.0.1"));
        assert!(dump.contains("ibex"));

        let token_cfg = ProspectorConfig::from_yaml_str(FAKE_YAML, "mmo").unwrap();
        let dump = format!("{token_cfg:?}");
        assert!(
            !dump.contains(FAKE_TOKEN),
            "token leaked into Debug: {dump}"
        );
        assert!(dump.contains("screeps.com"));
    }

    #[test]
    fn private_server_entry_selects_userpass() {
        let cfg = ProspectorConfig::from_yaml_str(FAKE_YAML, "private-server").unwrap();
        assert_eq!(cfg.base_url, "http://127.0.0.1:21025");
        match &cfg.auth {
            AuthMode::UserPass { username, .. } => assert_eq!(username, "ibex"),
            other => panic!("expected UserPass, got {other:?}"),
        }
    }

    #[test]
    fn mmo_entry_selects_token_and_clean_https_url() {
        let cfg = ProspectorConfig::from_yaml_str(FAKE_YAML, "mmo").unwrap();
        // 443 is the https scheme default — omitted for clean URLs.
        assert_eq!(cfg.base_url, "https://screeps.com");
        assert!(matches!(cfg.auth, AuthMode::Token(_)));
    }

    #[test]
    fn season_path_prefix_is_appended() {
        let cfg = ProspectorConfig::from_yaml_str(FAKE_YAML, "season").unwrap();
        assert_eq!(cfg.base_url, "https://screeps.com/season");
    }

    #[test]
    fn unknown_server_name_is_a_clear_error() {
        let err = ProspectorConfig::from_yaml_str(FAKE_YAML, "nope").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("'nope'"), "unhelpful error: {msg}");
        assert!(
            msg.contains("private-server"),
            "should list known servers: {msg}"
        );
    }

    #[test]
    fn entry_without_any_credentials_is_a_clear_error() {
        let yaml = r#"
servers:
  broken:
    host: 127.0.0.1
"#;
        let err = ProspectorConfig::from_yaml_str(yaml, "broken").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("token"),
            "should explain the auth options: {msg}"
        );
    }

    /// The MMO-safety classification (P0.P4): token auth OR a
    /// screeps.com host marks an entry official; only username/password
    /// against a non-screeps.com host is "private".
    #[test]
    fn official_server_classification() {
        let mmo = ProspectorConfig::from_yaml_str(FAKE_YAML, "mmo").unwrap();
        assert!(mmo.is_official(), "token + screeps.com");
        let season = ProspectorConfig::from_yaml_str(FAKE_YAML, "season").unwrap();
        assert!(season.is_official());
        let private = ProspectorConfig::from_yaml_str(FAKE_YAML, "private-server").unwrap();
        assert!(!private.is_official(), "user/pass on localhost is private");
        // Token auth alone is enough — even against a private host.
        let token_private = r#"
servers:
  token-local:
    host: 127.0.0.1
    port: 21025
    token: ffffffff-aaaa-bbbb-cccc-fake-token-material-0042
"#;
        let cfg = ProspectorConfig::from_yaml_str(token_private, "token-local").unwrap();
        assert!(cfg.is_official(), "token auth is treated with MMO caution");
    }

    #[test]
    fn base_url_construction_matrix() {
        assert_eq!(build_base_url("h", None, false, None), "http://h:21025");
        assert_eq!(build_base_url("h", Some(80), false, None), "http://h");
        assert_eq!(
            build_base_url("h", Some(8080), true, None),
            "https://h:8080"
        );
        assert_eq!(
            build_base_url("h", None, true, Some("/ptr")),
            "https://h/ptr"
        );
        assert_eq!(
            build_base_url("h", None, true, Some("ptr/")),
            "https://h/ptr"
        );
    }
}
