//! Step 1 — `cargo build --target wasm32-unknown-unknown` with the
//! per-server extra args, and exact artifact discovery from cargo's
//! `--message-format=json-render-diagnostics` stream (the trunk
//! pattern, `trunk src/pipelines/rust/mod.rs:406-537`: never guess at
//! target-dir layout).
//!
//! The invocation mirrors wasm-pack 0.14's
//! (`cargo build --lib [--release] --target wasm32-unknown-unknown`,
//! run from the crate directory so `rust-toolchain.toml` / `.cargo`
//! config discovery behave identically) — byte-parity of the resulting
//! artifact against the wasm-pack pipeline was verified live
//! (PARITY.md).

use crate::config::ServerConfig;
use crate::project::Project;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

/// Whole-build budget. Nightly + build-std + fat LTO: a cold build can
/// take many minutes; progress streams live.
pub const BUILD_BUDGET: Duration = Duration::from_secs(40 * 60);

/// The wasm32 target triple every Screeps build uses.
pub const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// The cargo argv (after `cargo`) for a bot build — pure, pinned by
/// tests: the per-server extra args pass through VERBATIM (risk #2).
pub fn build_args(server: &ServerConfig, debug: bool) -> Vec<String> {
    let mut args: Vec<String> = ["build", "--lib", "--target", WASM_TARGET]
        .map(str::to_string)
        .to_vec();
    if !debug {
        args.push("--release".to_string());
    }
    args.extend(server.extra_cargo_args.iter().cloned());
    args.push("--message-format=json-render-diagnostics".to_string());
    args
}

/// A located build artifact.
#[derive(Debug)]
pub struct BuiltArtifact {
    /// The cdylib `.wasm` produced for the bot package.
    pub wasm_path: PathBuf,
    /// The cargo target directory (derived from the artifact path:
    /// `<target>/<triple>/<profile>/<name>.wasm`) — also hosts the
    /// screeps-pack tool cache and output tree.
    pub target_dir: PathBuf,
}

/// Find the bot package's cdylib `.wasm` among cargo's JSON messages.
/// Pure (a fixture-tested parser): scans `compiler-artifact` messages
/// for the named package, keeps the LAST matching `.wasm` filename
/// (the final artifact of the build).
pub fn find_wasm_artifact(json_lines: &[String], package_name: &str) -> Option<PathBuf> {
    let mut found = None;
    for line in json_lines {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if msg.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        // package_id is a spec URL ending in `#name@version` (or
        // `…/name#version`); target.name is the lib target (underscored
        // on older cargo, dashed on newer) — accept either signal.
        let package_id = msg
            .get("package_id")
            .and_then(|p| p.as_str())
            .unwrap_or_default();
        let target_name = msg
            .pointer("/target/name")
            .and_then(|n| n.as_str())
            .unwrap_or_default();
        let underscored = package_name.replace('-', "_");
        let is_ours = target_name == package_name
            || target_name == underscored
            || package_id.contains(&format!("#{package_name}@"))
            || package_id.contains(&format!("/{package_name}#"));
        if !is_ours {
            continue;
        }
        let kinds = msg
            .pointer("/target/kind")
            .and_then(|k| k.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        if !kinds.contains(&"cdylib") {
            continue;
        }
        for filename in msg
            .get("filenames")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            if filename.ends_with(".wasm") {
                found = Some(PathBuf::from(filename));
            }
        }
    }
    found
}

/// `<target>/<triple>/<profile>/<name>.wasm` -> `<target>` (the cargo
/// target-dir layout; stable across cargo versions).
pub fn target_dir_of(wasm_path: &std::path::Path) -> Result<PathBuf> {
    wasm_path
        .ancestors()
        .nth(3)
        .map(PathBuf::from)
        .with_context(|| format!("{} is not under a cargo target dir", wasm_path.display()))
}

/// Run the build, streaming compiler progress to tracing, and locate
/// the artifact. Nightly is sanity-checked up front when the extra args
/// need it (`-Z`).
pub async fn build(project: &Project, server: &ServerConfig, debug: bool) -> Result<BuiltArtifact> {
    if server.extra_cargo_args.iter().any(|a| a == "-Z")
        || server.extra_cargo_args.iter().any(|a| a.starts_with("-Z"))
    {
        let version = tokio::process::Command::new("cargo")
            .arg("--version")
            .current_dir(&project.crate_dir)
            .output()
            .await
            .context("running `cargo --version` — is cargo on PATH?")?;
        let version = String::from_utf8_lossy(&version.stdout).trim().to_string();
        if !version.contains("nightly") {
            bail!(
                "the configured build args use `-Z` (build-std) which needs a nightly \
                 toolchain, but `cargo --version` in {} reports: {version} — pin nightly \
                 via rust-toolchain.toml (with the rust-src component) or adjust the \
                 wasm-pack-options",
                project.crate_dir.display()
            );
        }
    }

    let args = build_args(server, debug);
    tracing::info!(
        "cargo {} (cwd {})",
        args.join(" "),
        project.crate_dir.display()
    );
    let start = Instant::now();
    let mut child = tokio::process::Command::new("cargo")
        .args(&args)
        .current_dir(&project.crate_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning cargo — is it on PATH?")?;

    // stdout: the JSON message stream (collected); stderr: human
    // progress/diagnostics (streamed through tracing as it happens).
    let stdout = child.stdout.take().context("no stdout pipe")?;
    let stderr = child.stderr.take().context("no stderr pipe")?;
    let collect = tokio::spawn(async move {
        let mut lines = Vec::new();
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            lines.push(line);
        }
        lines
    });
    let stream = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tracing::info!(target: "cargo", "{line}");
        }
    });

    let status = match tokio::time::timeout(BUILD_BUDGET, child.wait()).await {
        Ok(status) => status.context("waiting for cargo")?,
        Err(_) => {
            let _ = child.kill().await;
            bail!("cargo build exceeded the {BUILD_BUDGET:?} budget — killed");
        }
    };
    let json_lines = collect.await.unwrap_or_default();
    let _ = stream.await;

    if !status.success() {
        bail!("cargo build failed with {status} (see the compiler output above)");
    }
    let wasm_path = find_wasm_artifact(&json_lines, &project.package_name).with_context(|| {
        format!(
            "cargo build succeeded but no cdylib .wasm artifact for '{}' appeared in \
             the JSON message stream",
            project.package_name
        )
    })?;
    let target_dir = target_dir_of(&wasm_path)?;
    tracing::info!(
        "built {} ({:.2} MiB) in {:.0?}",
        wasm_path.display(),
        std::fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0),
        start.elapsed()
    );
    Ok(BuiltArtifact {
        wasm_path,
        target_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_server_config;

    fn server(yaml_extra: &str) -> ServerConfig {
        let yaml = format!(
            "servers:\n  s:\n    host: h\n    username: u\n    password: p\nconfigs:\n  wasm-pack-options:\n{yaml_extra}"
        );
        parse_server_config(&yaml, "s").unwrap()
    }

    /// THE ARGV PIN (risk #2): release argv carries the build-std +
    /// rustflags args verbatim, in order, plus per-server features.
    #[test]
    fn release_argv_passes_flags_through_verbatim() {
        let server = server(
            "    '*': [\"--config\", \"build.rustflags=['-Ctarget-cpu=mvp']\", \"-Z\", \"build-std=std,panic_abort\"]\n    s: [\"--features\", \"mmo\"]\n",
        );
        assert_eq!(
            build_args(&server, false),
            [
                "build",
                "--lib",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
                "--config",
                "build.rustflags=['-Ctarget-cpu=mvp']",
                "-Z",
                "build-std=std,panic_abort",
                "--features",
                "mmo",
                "--message-format=json-render-diagnostics",
            ]
        );
    }

    /// Debug mode drops --release (wasm-pack `--dev` equivalence) and
    /// still passes the extra args.
    #[test]
    fn debug_argv_omits_release() {
        let server = server("    '*': [\"-Z\", \"build-std=std,panic_abort\"]\n");
        let args = build_args(&server, true);
        assert!(!args.contains(&"--release".to_string()));
        assert!(args
            .windows(2)
            .any(|w| w == ["-Z", "build-std=std,panic_abort"]));
    }

    /// Artifact discovery over a recorded-shape cargo JSON stream:
    /// dependency artifacts and rlib-only messages are skipped; the
    /// LAST matching cdylib .wasm wins; non-JSON lines are tolerated.
    #[test]
    fn artifact_discovery_picks_the_package_cdylib() {
        let lines: Vec<String> = [
            "warning: junk non-json line",
            r#"{"reason":"compiler-artifact","package_id":"registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0","target":{"name":"serde","kind":["lib"]},"filenames":["/t/wasm32-unknown-unknown/release/libserde.rlib"]}"#,
            r#"{"reason":"compiler-artifact","package_id":"path+file:///repo/screeps-ibex#0.0.1","target":{"name":"screeps-ibex","kind":["cdylib","rlib"]},"filenames":["/t/wasm32-unknown-unknown/release/screeps_ibex.wasm","/t/wasm32-unknown-unknown/release/libscreeps_ibex.rlib"]}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .map(str::to_string)
        .to_vec();
        let found = find_wasm_artifact(&lines, "screeps-ibex").unwrap();
        assert!(found.to_string_lossy().ends_with("screeps_ibex.wasm"));
        assert!(find_wasm_artifact(&lines, "other-bot").is_none());
    }

    /// package_id spec-URL matching (`#name@version`, newer cargo).
    #[test]
    fn artifact_discovery_matches_spec_url_ids() {
        let lines = vec![
            r#"{"reason":"compiler-artifact","package_id":"path+file:///repo/my-bot#my-bot@0.1.0","target":{"name":"my_bot","kind":["cdylib"]},"filenames":["/t/w/release/my_bot.wasm"]}"#.to_string(),
        ];
        assert!(find_wasm_artifact(&lines, "my-bot").is_some());
    }

    /// `<target>/<triple>/<profile>/x.wasm` -> `<target>`.
    #[test]
    fn target_dir_derivation() {
        let target = target_dir_of(std::path::Path::new(
            "/repo/target/wasm32-unknown-unknown/release/screeps_ibex.wasm",
        ))
        .unwrap();
        assert_eq!(target, PathBuf::from("/repo/target"));
    }
}
