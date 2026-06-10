//! Bot-crate resolution: which crate to build, what to call its
//! modules, and the `[package.metadata.*]` knobs the pipeline honors.
//!
//! Zero-config convention (community design, investigation §4): point
//! screeps-pack at a crate that links `wasm-bindgen` with
//! `crate-type = ["cdylib", ...]` and exports `setup` + `game_loop`
//! (the rustyscreeps-starter convention). Module name = crate name
//! underscored. When the manifest is a *virtual workspace* root, the
//! workspace is searched for exactly ONE cdylib member — so running
//! from a starter-style workspace root needs no flags at all.
//!
//! Metadata honored (all optional):
//! - `[package.metadata.wasm-pack.profile.{dev,release}] wasm-opt = [..]|false`
//!   — the existing wasm-pack tables, honored as-is for drop-in parity
//!   (absent ⇒ wasm-pack's default `["-O"]`; `false` ⇒ skip).
//! - `[package.metadata.screeps-pack] bucket-boot-threshold = <u32>`
//!   — the loader's boot gate (default 1500).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The loader's default bucket gate (js_src/main.js:7 convention).
pub const DEFAULT_BUCKET_BOOT_THRESHOLD: u32 = 1500;

/// wasm-pack's default wasm-opt invocation when no metadata table exists.
pub const DEFAULT_WASM_OPT_ARGS: &[&str] = &["-O"];

/// The resolved bot crate.
#[derive(Debug, Clone)]
pub struct Project {
    /// Directory containing the bot crate's Cargo.toml (cargo runs here).
    pub crate_dir: PathBuf,
    /// Cargo package name (e.g. `screeps-ibex`).
    pub package_name: String,
    /// Module name = package name underscored (e.g. `screeps_ibex`) —
    /// the uploaded JS glue module; the wasm bytes upload as
    /// `<module_name>_bg`.
    pub module_name: String,
    /// Loader boot gate.
    pub bucket_boot_threshold: u32,
    /// `wasm-opt` args per mode; `None` = explicitly disabled.
    pub wasm_opt_release: Option<Vec<String>>,
    pub wasm_opt_dev: Option<Vec<String>>,
}

impl Project {
    pub fn wasm_module_name(&self) -> String {
        format!("{}_bg", self.module_name)
    }

    pub fn wasm_opt_args(&self, debug: bool) -> Option<&[String]> {
        let args = if debug {
            &self.wasm_opt_dev
        } else {
            &self.wasm_opt_release
        };
        args.as_deref()
    }
}

/// Resolve the bot crate from a manifest path: a `[package]` manifest is
/// used directly; a virtual-workspace root resolves to its single cdylib
/// member (ambiguity is a clear error naming the candidates).
pub fn resolve_project(manifest_path: &Path) -> Result<Project> {
    let manifest_path = manifest_path
        .canonicalize()
        .with_context(|| format!("manifest {} not found", manifest_path.display()))?;
    let root: toml::Value = parse_manifest(&manifest_path)?;

    if root.get("package").is_some() {
        return project_from_package(&manifest_path, &root);
    }

    let Some(workspace) = root.get("workspace") else {
        bail!(
            "{} has neither [package] nor [workspace] — not a Cargo manifest?",
            manifest_path.display()
        );
    };
    let members = workspace
        .get("members")
        .and_then(|m| m.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let dir = manifest_path.parent().context("manifest has no parent")?;

    // A virtual workspace: the bot crate is the single cdylib member.
    let mut candidates = Vec::new();
    for member in &members {
        // Globs are not supported (rare in bot workspaces); plain paths only.
        let member_manifest = dir.join(member).join("Cargo.toml");
        let Ok(parsed) = parse_manifest(&member_manifest) else {
            continue;
        };
        if is_cdylib(&parsed) {
            candidates.push(member_manifest);
        }
    }
    match candidates.as_slice() {
        [single] => {
            let parsed = parse_manifest(single)?;
            project_from_package(single, &parsed)
        }
        [] => bail!(
            "{} is a workspace root with no cdylib member — pass the bot crate's \
             manifest explicitly (--manifest-path <crate>/Cargo.toml)",
            manifest_path.display()
        ),
        many => bail!(
            "{} is a workspace root with multiple cdylib members ({:?}) — pass the bot \
             crate's manifest explicitly (--manifest-path <crate>/Cargo.toml)",
            manifest_path.display(),
            many.iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        ),
    }
}

fn parse_manifest(path: &Path) -> Result<toml::Value> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    raw.parse::<toml::Value>()
        .with_context(|| format!("parsing {}", path.display()))
}

fn is_cdylib(manifest: &toml::Value) -> bool {
    manifest
        .get("lib")
        .and_then(|l| l.get("crate-type"))
        .and_then(|c| c.as_array())
        .map(|kinds| kinds.iter().any(|k| k.as_str() == Some("cdylib")))
        .unwrap_or(false)
}

fn project_from_package(manifest_path: &Path, manifest: &toml::Value) -> Result<Project> {
    let package_name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .with_context(|| format!("{} has no package.name", manifest_path.display()))?
        .to_string();
    if !is_cdylib(manifest) {
        bail!(
            "{} ({package_name}) is not a cdylib — a Screeps wasm bot needs \
             `[lib] crate-type = [\"cdylib\", ...]`",
            manifest_path.display()
        );
    }

    let metadata = manifest.get("package").and_then(|p| p.get("metadata"));
    let bucket_boot_threshold = metadata
        .and_then(|m| m.get("screeps-pack"))
        .and_then(|s| s.get("bucket-boot-threshold"))
        .and_then(|v| v.as_integer())
        .map(|v| v as u32)
        .unwrap_or(DEFAULT_BUCKET_BOOT_THRESHOLD);

    let wasm_opt_profile = |profile: &str| -> Option<Vec<String>> {
        let value = metadata
            .and_then(|m| m.get("wasm-pack"))
            .and_then(|w| w.get("profile"))
            .and_then(|p| p.get(profile))
            .and_then(|p| p.get("wasm-opt"));
        match value {
            // `wasm-opt = false` — explicitly disabled.
            Some(toml::Value::Boolean(false)) => None,
            Some(toml::Value::Array(args)) => Some(
                args.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            ),
            // Absent (or any other shape): wasm-pack's default.
            _ => Some(
                DEFAULT_WASM_OPT_ARGS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
        }
    };

    Ok(Project {
        crate_dir: manifest_path
            .parent()
            .context("manifest has no parent")?
            .to_path_buf(),
        module_name: package_name.replace('-', "_"),
        package_name,
        bucket_boot_threshold,
        wasm_opt_release: wasm_opt_profile("release"),
        wasm_opt_dev: wasm_opt_profile("dev"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The bot crate's real metadata shape (screeps-ibex/Cargo.toml:46-59).
    const IBEX_LIKE: &str = r#"
[package]
name = "screeps-ibex"
version = "0.0.1"

[lib]
crate-type = ["cdylib", "rlib"]

[package.metadata.wasm-pack.profile.dev]
wasm-opt = ["--signext-lowering"]

[package.metadata.wasm-pack.profile.release]
wasm-opt = ["-O4", "--signext-lowering"]
"#;

    fn write_tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        dir
    }

    /// Direct package manifest: name underscoring + the wasm-pack
    /// metadata tables honored as-is (drop-in parity).
    #[test]
    fn package_manifest_resolves_with_wasm_pack_metadata() {
        let dir = write_tree(&[("Cargo.toml", IBEX_LIKE)]);
        let p = resolve_project(&dir.path().join("Cargo.toml")).unwrap();
        assert_eq!(p.package_name, "screeps-ibex");
        assert_eq!(p.module_name, "screeps_ibex");
        assert_eq!(p.wasm_module_name(), "screeps_ibex_bg");
        assert_eq!(p.bucket_boot_threshold, DEFAULT_BUCKET_BOOT_THRESHOLD);
        assert_eq!(
            p.wasm_opt_args(false).unwrap(),
            ["-O4", "--signext-lowering"]
        );
        assert_eq!(p.wasm_opt_args(true).unwrap(), ["--signext-lowering"]);
    }

    /// Virtual workspace root: the single cdylib member is the bot.
    #[test]
    fn workspace_root_finds_single_cdylib_member() {
        let dir = write_tree(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"bot\", \"helper\"]\n",
            ),
            ("bot/Cargo.toml", IBEX_LIKE),
            (
                "helper/Cargo.toml",
                "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n",
            ),
        ]);
        let p = resolve_project(&dir.path().join("Cargo.toml")).unwrap();
        assert_eq!(p.package_name, "screeps-ibex");
        assert!(p.crate_dir.ends_with("bot"));
    }

    #[test]
    fn workspace_without_cdylib_member_is_a_clear_error() {
        let dir = write_tree(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"helper\"]\n"),
            (
                "helper/Cargo.toml",
                "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n",
            ),
        ]);
        let err = resolve_project(&dir.path().join("Cargo.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("--manifest-path"));
    }

    /// Defaults + overrides: bucket threshold from
    /// [package.metadata.screeps-pack]; wasm-opt = false disables; no
    /// metadata at all -> wasm-pack's default ["-O"].
    #[test]
    fn metadata_knobs() {
        let manifest = r#"
[package]
name = "my-bot"
version = "0.1.0"

[lib]
crate-type = ["cdylib"]

[package.metadata.screeps-pack]
bucket-boot-threshold = 500

[package.metadata.wasm-pack.profile.release]
wasm-opt = false
"#;
        let dir = write_tree(&[("Cargo.toml", manifest)]);
        let p = resolve_project(&dir.path().join("Cargo.toml")).unwrap();
        assert_eq!(p.bucket_boot_threshold, 500);
        assert!(p.wasm_opt_args(false).is_none(), "false disables");
        // dev table absent -> wasm-pack default.
        assert_eq!(p.wasm_opt_args(true).unwrap(), ["-O"]);
    }

    #[test]
    fn non_cdylib_package_is_a_clear_error() {
        let dir = write_tree(&[(
            "Cargo.toml",
            "[package]\nname = \"plain\"\nversion = \"0.1.0\"\n",
        )]);
        let err = resolve_project(&dir.path().join("Cargo.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("cdylib"));
    }
}
