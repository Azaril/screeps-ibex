//! Deploy orchestration — a library call into **`screeps-pack`**
//! (P0.A13, cut over after the parity gate went green; evidence:
//! `../screeps-pack/PARITY.md`).
//!
//! The pipeline (all inside screeps-pack): `cargo build --target
//! wasm32-unknown-unknown` honoring the `.screeps.yaml`
//! `configs.wasm-pack-options` per-server → lockfile-matched
//! wasm-bindgen (`--target nodejs`) → CJS glue (loader template +
//! anchored bindgen patch) → wasm-opt (binaryen, pinned) → upload via
//! the shared `screeps-rest-api` client. Deploying *some crate* to
//! *some server entry* is bot-agnostic, which is why this seam lives in
//! the kit; the public surface (`deploy(cfg, server_entry, debug)` →
//! [`DeployReport`]) is the contract consumers keep calling.
//!
//! The bot project is located from the kit's own config anchor: the
//! credentials file's directory (`../.screeps.yaml` → the repo root)
//! is the workspace root, and screeps-pack resolves the single cdylib
//! member from there — no js_tools/, no package.json, no node. The
//! npm pipeline (`js_tools/deploy.js`) remains in the repo ONLY as an
//! optional user-customization escape hatch; nothing here invokes it.

use crate::config::KitConfig;
use anyhow::{Context, Result};
use screeps_pack::PackOptions;
use std::path::Path;
use std::time::Duration;

#[derive(Debug)]
pub struct DeployReport {
    pub branch: String,
    /// Size of the uploaded module map against the 5 MiB code limit.
    pub used_mib: Option<f64>,
    pub mode: &'static str,
    pub duration: Duration,
}

impl std::fmt::Display for DeployReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "deployed branch '{}' ({} build) in {:.0?}",
            self.branch, self.mode, self.duration
        )?;
        if let Some(mib) = self.used_mib {
            write!(f, " — {mib:.2} MiB of the 5 MiB code limit")?;
        }
        Ok(())
    }
}

/// The screeps-pack request for a deploy as `server_entry` (pure —
/// pinned by a unit test): credentials = the file the kit's config came
/// from; the bot manifest = the workspace root next to it (screeps-pack
/// resolves the single cdylib member from there).
pub fn pack_options(cfg: &KitConfig, server_entry: &str, debug: bool) -> Result<PackOptions> {
    let creds_path = cfg
        .source_path
        .clone()
        .context("config was not loaded from a file — cannot locate the bot repo")?;
    let root = creds_path
        .parent()
        .map(Path::to_path_buf)
        .context("credentials file has no parent directory")?;
    Ok(PackOptions {
        manifest_path: root.join("Cargo.toml"),
        creds_path,
        server: server_entry.to_string(),
        debug,
    })
}

/// Build + upload the bot as `server_entry` via screeps-pack (library
/// call). `server_entry` is the `.screeps.yaml` `servers:` entry the
/// upload targets: the kit's own acting identity by default, or a bot
/// entry selected via `deploy --user <entry>` (P0.A10) — its
/// credentials AND its per-server build flags both come from that
/// entry. Build/tool progress streams through tracing as it happens.
pub async fn deploy(cfg: &KitConfig, server_entry: &str, debug: bool) -> Result<DeployReport> {
    let options = pack_options(cfg, server_entry, debug)?;
    tracing::info!(
        "deploy: screeps-pack --server {} ({} build) — a cold wasm build takes \
         minutes; progress streams below",
        server_entry,
        options.mode_str()
    );
    let outcome = screeps_pack::deploy(&options).await?;
    let report = DeployReport {
        branch: outcome.branch,
        used_mib: Some(outcome.used_mib),
        mode: outcome.mode,
        duration: outcome.duration,
    };
    tracing::info!("{report}");
    Ok(report)
}

/// Tiny helper so the log line above can name the mode without
/// duplicating the debug→mode mapping everywhere.
trait ModeStr {
    fn mode_str(&self) -> &'static str;
}
impl ModeStr for PackOptions {
    fn mode_str(&self) -> &'static str {
        if self.debug {
            "debug"
        } else {
            "release"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cfg_with_source(source: Option<PathBuf>) -> KitConfig {
        let creds = "servers:\n  private-server:\n    host: 127.0.0.1\n    port: 21025\n    username: ibex\n    password: not-a-real-pw\n";
        let mut cfg = KitConfig::from_yaml_strs(creds, None, Some("private-server")).unwrap();
        cfg.source_path = source;
        cfg
    }

    /// THE SEAM PIN (P0.A10/A13): the pack request points credentials at
    /// the kit's own config source and the manifest at the workspace
    /// root next to it; `--user <entry>` passes through as the server
    /// entry; `--debug` maps to a debug build.
    #[test]
    fn pack_options_map_the_kit_config() {
        let cfg = cfg_with_source(Some(PathBuf::from(r"C:\repo\.screeps.yaml")));
        let options = pack_options(&cfg, "ibex-2", false).unwrap();
        assert_eq!(options.creds_path, PathBuf::from(r"C:\repo\.screeps.yaml"));
        assert_eq!(options.manifest_path, PathBuf::from(r"C:\repo\Cargo.toml"));
        assert_eq!(options.server, "ibex-2");
        assert!(!options.debug);

        let debug = pack_options(&cfg, "private-server", true).unwrap();
        assert!(debug.debug);
        assert_eq!(debug.mode_str(), "debug");
    }

    /// A config not loaded from a file cannot locate the bot repo —
    /// clear error, not a panic.
    #[test]
    fn missing_source_path_is_a_clear_error() {
        let cfg = cfg_with_source(None);
        let err = pack_options(&cfg, "private-server", false).unwrap_err();
        assert!(format!("{err:#}").contains("not loaded from a file"));
    }
}
