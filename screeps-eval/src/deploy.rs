//! Deploy wrapper around `js_tools/deploy.js` (P0.A4).
//!
//! OPERATOR DIRECTIVE (phase-0.md P0.A4): the deploy script and the wasm
//! build pipeline (wasm-pack/rollup/npm) are **never modified** — this
//! module wraps the script: working directory, environment, output
//! streaming, and honest success detection. Native `POST /api/user/code`
//! is deferred indefinitely.
//!
//! ## Pinned facts from `js_tools/deploy.js` (read 2026-06-09, unmodified)
//!
//! - yargs options (deploy.js:15-30): `--server <name>` (required; the
//!   `.screeps.yaml` `servers:` entry), `--dryrun` (bool, default false),
//!   `--mode debug|release` (default release). `--mode debug` maps to
//!   `wasm-pack build --dev` (deploy.js:85) — that is what our
//!   `deploy --debug` passes through.
//! - The script reads `.screeps.yaml` ITSELF from its CWD (deploy.js:38)
//!   and authenticates via `ScreepsAPI.fromConfig(server)` (deploy.js:156)
//!   — **no credentials on argv or env** (verified; the A7 sweep relies
//!   on this).
//! - It requires the `npm_package_name` env var (deploy.js:32), normally
//!   injected by `npm run deploy`. The wrapper reads `package.json`'s
//!   `name` and sets that one (non-secret) variable so plain `node`
//!   invocation works identically.
//! - **The exit code is NOT a success signal.** `run().catch(console.error)`
//!   (deploy.js:178) exits 0 after printing any error; a wasm-pack failure
//!   returns silently (deploy.js:169-171: `if (build_result.status !== 0)
//!   return`); an unknown server entry logs and returns (deploy.js:41-44).
//!   Success is therefore detected from output: the upload banner
//!   `Uploading to branch <b>; using <x> MiB of 5.0 MiB ...`
//!   (deploy.js:151,155) followed by the API response JSON
//!   (deploy.js:158, `{"ok":1}` from the private server's
//!   `POST /api/user/code`).

use crate::config::EvalConfig;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

/// Whole-deploy budget. The wasm build is nightly + fat LTO; a cold
/// first build (empty target dir) can take many minutes, warm rebuilds
/// are far quicker. Generous on purpose — progress streams live.
pub const DEPLOY_BUDGET: Duration = Duration::from_secs(40 * 60);

#[derive(Debug)]
pub struct DeployReport {
    pub branch: String,
    /// Parsed from the upload banner (informational).
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

/// Locate the repo root deploy.js must run from: the directory holding
/// `.screeps.yaml` (which the script reads from CWD), validated to also
/// contain the script and its `package.json`.
pub fn find_repo_root(cfg: &EvalConfig) -> Result<PathBuf> {
    let root = cfg
        .source_path
        .as_deref()
        .and_then(Path::parent)
        .context("config was not loaded from a file — cannot locate the repo root")?;
    for required in ["js_tools/deploy.js", "package.json"] {
        if !root.join(required).is_file() {
            bail!(
                "{required} not found in {} — deploy must run from the screeps-ibex \
                 repo root (the directory containing .screeps.yaml)",
                root.display()
            );
        }
    }
    Ok(root.to_path_buf())
}

/// `package.json` `name` — substituted for the `npm_package_name` env
/// var that `npm run deploy` would set (deploy.js:32 requires it).
fn package_name(root: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(root.join("package.json"))
        .with_context(|| format!("reading {}", root.join("package.json").display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw).context("parsing package.json")?;
    json.get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .context("package.json has no `name` field")
}

// ===================================================================
// output verdict (pure — deploy.js exits 0 even on failure)
// ===================================================================

#[derive(Debug, PartialEq)]
pub enum DeployVerdict {
    Uploaded {
        branch: String,
        used_mib: Option<f64>,
    },
    Failed {
        reason: String,
    },
}

/// Decide success from the script's output (see module docs for why the
/// exit code cannot be trusted). Success = the upload banner followed by
/// an API response JSON object with `ok: 1`.
pub fn parse_deploy_output(lines: &[String]) -> DeployVerdict {
    let mut upload: Option<(usize, String, Option<f64>)> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(rest) = line.strip_prefix("Uploading to branch ") {
            let branch = rest.split(';').next().unwrap_or("").trim().to_string();
            upload = Some((i, branch, parse_used_mib(rest)));
        }
    }
    let Some((idx, branch, used_mib)) = upload else {
        // deploy.js:169-171 — a failed wasm-pack/rollup build returns
        // silently after the "Building in <mode> mode" banner.
        if lines.iter().any(|l| l.starts_with("Building in ")) {
            return DeployVerdict::Failed {
                reason: "the build failed before upload (wasm-pack/rollup; deploy.js \
                         exits 0 on build failure — see its output above)"
                    .into(),
            };
        }
        return DeployVerdict::Failed {
            reason: "no build/upload output recognized — deploy.js likely failed \
                     before building (e.g. unknown --server entry, deploy.js:41-44)"
                .into(),
        };
    };
    // deploy.js:158 prints JSON.stringify(response) after the banner.
    for line in &lines[idx + 1..] {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if !v.is_object() {
            continue;
        }
        if v.get("ok").and_then(serde_json::Value::as_i64) == Some(1) {
            return DeployVerdict::Uploaded { branch, used_mib };
        }
        return DeployVerdict::Failed {
            reason: format!("upload API response was not ok: {}", line.trim()),
        };
    }
    DeployVerdict::Failed {
        reason: "upload started but no API response followed (the upload threw; \
                 deploy.js:178 prints the error and exits 0)"
            .into(),
    }
}

/// `default; using 2.93 MiB of 5.0 MiB code size limit (58.62%)` → 2.93
fn parse_used_mib(banner_rest: &str) -> Option<f64> {
    let after = banner_rest.split("using ").nth(1)?;
    after.split_whitespace().next()?.parse().ok()
}

// ===================================================================
// the wrapper
// ===================================================================

/// Build + upload the bot via `node js_tools/deploy.js --server <name>
/// --mode <debug|release>` from the repo root. Output streams through
/// tracing as it happens; success is decided by [`parse_deploy_output`].
pub async fn deploy(cfg: &EvalConfig, server_name: &str, debug: bool) -> Result<DeployReport> {
    let root = find_repo_root(cfg)?;
    if !root.join("node_modules").is_dir() {
        bail!(
            "node_modules missing in {} — run `npm install` there once \
             (deploy.js needs its rollup/screeps-api toolchain)",
            root.display()
        );
    }
    let pkg = package_name(&root)?;
    let mode = if debug { "debug" } else { "release" };

    tracing::info!(
        "deploy: node js_tools/deploy.js --server {server_name} --mode {mode} (cwd {})",
        root.display()
    );
    tracing::info!(
        "the full wasm build runs now (nightly toolchain, LTO) — a cold build \
         takes several minutes; output streams below"
    );

    let start = Instant::now();
    let mut child = tokio::process::Command::new("node")
        .arg("js_tools/deploy.js")
        .arg("--server")
        .arg(server_name)
        .arg("--mode")
        .arg(mode)
        .current_dir(&root)
        // deploy.js:32 — normally set by `npm run deploy`; non-secret.
        .env("npm_package_name", &pkg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning node — is Node.js installed and on PATH?")?;

    // Stream both pipes line-by-line into tracing AND a transcript for
    // the verdict. wasm-pack/cargo run with stdio:'inherit' inside
    // deploy.js (deploy.js:87), so their progress arrives here too.
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stdout = child.stdout.take().context("no stdout pipe")?;
    let stderr = child.stderr.take().context("no stderr pipe")?;
    let out_task = tokio::spawn(stream_lines(stdout, lines.clone(), false));
    let err_task = tokio::spawn(stream_lines(stderr, lines.clone(), true));

    let status = match tokio::time::timeout(DEPLOY_BUDGET, child.wait()).await {
        Ok(status) => status.context("waiting for deploy.js")?,
        Err(_) => {
            let _ = child.kill().await;
            bail!(
                "deploy.js exceeded the {:?} budget — killed. Check the streamed \
                 output above (a cold nightly wasm build is slow, but not THIS slow).",
                DEPLOY_BUDGET
            );
        }
    };
    let _ = out_task.await;
    let _ = err_task.await;
    let lines = Arc::try_unwrap(lines)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_default();

    if !status.success() {
        bail!(
            "deploy.js exited with {status} (a crash — its own failures exit 0). \
             Last output:\n{}",
            tail(&lines, 15)
        );
    }
    match parse_deploy_output(&lines) {
        DeployVerdict::Uploaded { branch, used_mib } => {
            let report = DeployReport {
                branch,
                used_mib,
                mode,
                duration: start.elapsed(),
            };
            tracing::info!("{report}");
            Ok(report)
        }
        DeployVerdict::Failed { reason } => bail!(
            "deploy failed: {reason}\nlast output:\n{}",
            tail(&lines, 15)
        ),
    }
}

async fn stream_lines(
    pipe: impl tokio::io::AsyncRead + Unpin,
    sink: Arc<Mutex<Vec<String>>>,
    is_stderr: bool,
) {
    let mut reader = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if is_stderr {
            // cargo/wasm-pack progress arrives on stderr — informational.
            tracing::info!(target: "deploy_js", "! {line}");
        } else {
            tracing::info!(target: "deploy_js", "{line}");
        }
        if let Ok(mut v) = sink.lock() {
            v.push(line);
        }
    }
}

fn tail(lines: &[String], n: usize) -> String {
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Literal output shapes from deploy.js:151-158 (success path).
    #[test]
    fn verdict_success_requires_banner_and_ok_response() {
        let lines = s(&[
            "Building in release mode",
            "[INFO]: Compiling to Wasm...",
            "Uploading to branch default; using 2.93 MiB of 5.0 MiB code size limit (58.62%)",
            r#"{"ok":1}"#,
        ]);
        assert_eq!(
            parse_deploy_output(&lines),
            DeployVerdict::Uploaded {
                branch: "default".into(),
                used_mib: Some(2.93),
            }
        );
    }

    /// deploy.js:169-171 — wasm-pack failure returns silently, exit 0.
    #[test]
    fn verdict_build_failure_is_detected_despite_exit_zero() {
        let lines = s(&[
            "Building in release mode",
            "error[E0308]: mismatched types",
            "[ERROR]: Compiling your crate to WebAssembly failed",
        ]);
        let DeployVerdict::Failed { reason } = parse_deploy_output(&lines) else {
            panic!("build failure must not pass");
        };
        assert!(reason.contains("build failed"), "got: {reason}");
    }

    /// deploy.js:178 — run().catch(console.error): an upload throw is
    /// printed (not JSON) and the process exits 0.
    #[test]
    fn verdict_upload_throw_is_a_failure() {
        let lines = s(&[
            "Building in release mode",
            "Uploading to branch default; using 2.93 MiB of 5.0 MiB code size limit (58.62%)",
            "Error: connect ECONNREFUSED 127.0.0.1:21025",
        ]);
        let DeployVerdict::Failed { reason } = parse_deploy_output(&lines) else {
            panic!("upload throw must not pass");
        };
        assert!(reason.contains("no API response"), "got: {reason}");
    }

    #[test]
    fn verdict_not_ok_response_is_a_failure() {
        let lines = s(&[
            "Uploading to branch default; using 0.50 MiB of 5.0 MiB code size limit (10.00%)",
            r#"{"error":"code length exceeds 5 MiB limit"}"#,
        ]);
        let DeployVerdict::Failed { reason } = parse_deploy_output(&lines) else {
            panic!("not-ok response must not pass");
        };
        assert!(reason.contains("not ok"), "got: {reason}");
    }

    /// deploy.js:41-44 — unknown server entry logs and returns, exit 0.
    #[test]
    fn verdict_unknown_server_is_a_failure() {
        let lines = s(&["no configuration found for server foo in .screeps.yaml"]);
        assert!(matches!(
            parse_deploy_output(&lines),
            DeployVerdict::Failed { .. }
        ));
        assert!(matches!(
            parse_deploy_output(&[]),
            DeployVerdict::Failed { .. }
        ));
    }

    #[test]
    fn used_mib_parses_from_banner() {
        assert_eq!(
            parse_used_mib("default; using 2.93 MiB of 5.0 MiB code size limit (58.62%)"),
            Some(2.93)
        );
        assert_eq!(parse_used_mib("default"), None);
    }
}
