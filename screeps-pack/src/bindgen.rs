//! Step 2 — wasm-bindgen: resolve the version the BOT crate linked
//! from its `Cargo.lock`, fetch the MATCHING prebuilt CLI, run it with
//! `--target nodejs`.
//!
//! NEVER a static `wasm-bindgen-cli-support` pin: the bindgen schema
//! embedded in the `.wasm` must exactly match the CLI version
//! (wasm-bindgen#1587 `verify_schema_matches`) — the user's lockfile
//! decides, the tool follows (investigation risk #1; trunk
//! `wasm_bindgen.rs:58-90` is the reference mechanism).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Cargo.lock shape (the documented TOML format — parsed with the
/// `toml` crate, never hand-rolled).
#[derive(Deserialize)]
struct Lockfile {
    #[serde(default)]
    package: Vec<LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    name: String,
    version: String,
}

/// Find the lockfile governing `crate_dir`: the crate's own
/// `Cargo.lock` or the nearest ancestor's (workspace members lock at
/// the workspace root — cargo's own resolution rule).
pub fn find_lockfile(crate_dir: &Path) -> Result<PathBuf> {
    for dir in crate_dir.ancestors() {
        let candidate = dir.join("Cargo.lock");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!(
        "no Cargo.lock found at or above {} — build the crate once so cargo writes one",
        crate_dir.display()
    )
}

/// The `wasm-bindgen` version pinned in lockfile content (pure).
pub fn wasm_bindgen_version_in(lock_toml: &str) -> Result<String> {
    let lock: Lockfile = toml::from_str(lock_toml).context("parsing Cargo.lock")?;
    lock.package
        .iter()
        .find(|p| p.name == "wasm-bindgen")
        .map(|p| p.version.clone())
        .context("wasm-bindgen is not in the lockfile — does the crate depend on it?")
}

/// Resolve the wasm-bindgen version for the bot crate.
pub fn resolve_version(crate_dir: &Path) -> Result<String> {
    let lockfile = find_lockfile(crate_dir)?;
    let raw = std::fs::read_to_string(&lockfile)
        .with_context(|| format!("reading {}", lockfile.display()))?;
    let version = wasm_bindgen_version_in(&raw)
        .with_context(|| format!("resolving wasm-bindgen from {}", lockfile.display()))?;
    tracing::info!("wasm-bindgen {version} (from {})", lockfile.display());
    Ok(version)
}

/// Bindgen output: the nodejs-target JS glue + the processed wasm.
#[derive(Debug)]
pub struct BindgenOutput {
    pub js_path: PathBuf,
    pub wasm_path: PathBuf,
    pub version: String,
}

/// Run the lockfile-matched `wasm-bindgen --target nodejs` over the
/// cargo artifact, into `out_dir`.
pub async fn run(
    target_dir: &Path,
    crate_dir: &Path,
    artifact_wasm: &Path,
    module_name: &str,
    out_dir: &Path,
) -> Result<BindgenOutput> {
    let version = resolve_version(crate_dir)?;
    let cli = crate::tools::ensure_wasm_bindgen(target_dir, &version).await?;

    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let output = tokio::process::Command::new(&cli)
        .arg("--target")
        .arg("nodejs")
        .arg("--out-dir")
        .arg(out_dir)
        .arg("--out-name")
        .arg(module_name)
        .arg("--no-typescript")
        .arg(artifact_wasm)
        .output()
        .await
        .with_context(|| format!("spawning {}", cli.display()))?;
    if !output.status.success() {
        bail!(
            "wasm-bindgen {version} failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let js_path = out_dir.join(format!("{module_name}.js"));
    let wasm_path = out_dir.join(format!("{module_name}_bg.wasm"));
    for required in [&js_path, &wasm_path] {
        if !required.is_file() {
            bail!(
                "wasm-bindgen succeeded but {} is missing — output layout changed?",
                required.display()
            );
        }
    }
    tracing::info!(
        "wasm-bindgen {version}: {} ({:.2} MiB)",
        wasm_path.display(),
        std::fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0)
    );
    Ok(BindgenOutput {
        js_path,
        wasm_path,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workspace-root lockfile shape (this repo's Cargo.lock pins
    /// wasm-bindgen 0.2.108 today).
    #[test]
    fn lockfile_version_parses() {
        let lock = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.219"

[[package]]
name = "wasm-bindgen"
version = "0.2.108"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        assert_eq!(wasm_bindgen_version_in(lock).unwrap(), "0.2.108");
    }

    #[test]
    fn missing_wasm_bindgen_is_a_clear_error() {
        let err =
            wasm_bindgen_version_in("[[package]]\nname = \"x\"\nversion = \"1\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("wasm-bindgen"));
    }

    /// Workspace members resolve to the workspace-root lockfile.
    #[test]
    fn lockfile_walks_up_to_the_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let member = dir.path().join("bot");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "version = 4\n").unwrap();
        let found = find_lockfile(&member).unwrap();
        assert_eq!(found, dir.path().join("Cargo.lock"));
    }
}
