//! Step 4 — optional `wasm-opt` (binaryen) over the bindgen output.
//!
//! Args come from the bot crate's existing
//! `[package.metadata.wasm-pack.profile.*] wasm-opt` tables
//! ([`crate::project`]) so behavior is identical to the wasm-pack
//! pipeline. The binary: the pinned binaryen release is downloaded into
//! the tool cache (default version [`DEFAULT_BINARYEN_VERSION`] =
//! what wasm-pack 0.14 bundles via the `wasm-opt` crate — byte-parity
//! verified live, PARITY.md); if the download fails, a `wasm-opt` on
//! PATH is tried; if neither exists the step is SKIPPED gracefully
//! (the un-optimized wasm is valid, just larger).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// binaryen version matching wasm-pack 0.14's bundled `wasm-opt` crate
/// (0.116 = binaryen 116). Byte-identical output vs the wasm-pack
/// pipeline was verified live with this pin (PARITY.md).
pub const DEFAULT_BINARYEN_VERSION: &str = "116";

/// Outcome of the optional optimization step.
#[derive(Debug)]
pub enum OptOutcome {
    /// Optimized in place: tool + before/after sizes.
    Optimized {
        tool: PathBuf,
        bytes_before: u64,
        bytes_after: u64,
    },
    /// Step skipped, with the reason (disabled by metadata, or no
    /// binary available).
    Skipped { reason: String },
}

/// Run `wasm-opt <args> <wasm> -o <wasm>` (in place, via a temp file).
/// `args: None` means the profile disabled it (`wasm-opt = false`).
pub async fn run(
    target_dir: &Path,
    wasm_path: &Path,
    args: Option<&[String]>,
) -> Result<OptOutcome> {
    let Some(args) = args else {
        return Ok(OptOutcome::Skipped {
            reason: "disabled by [package.metadata.wasm-pack.profile.*] wasm-opt = false".into(),
        });
    };

    let tool = match locate_wasm_opt(target_dir).await {
        Ok(tool) => tool,
        Err(e) => {
            tracing::warn!(
                "wasm-opt unavailable — skipping optimization (the wasm is valid, just \
                 larger): {e:#}"
            );
            return Ok(OptOutcome::Skipped {
                reason: format!("wasm-opt unavailable: {e:#}"),
            });
        }
    };

    let bytes_before = std::fs::metadata(wasm_path)
        .with_context(|| format!("reading {}", wasm_path.display()))?
        .len();
    let tmp_out = wasm_path.with_extension("wasm.opt-tmp");

    // Args BEFORE input, then `-o` — the exact invocation shape whose
    // output was byte-compared against wasm-pack's (PARITY.md).
    let output = tokio::process::Command::new(&tool)
        .args(args)
        .arg(wasm_path)
        .arg("-o")
        .arg(&tmp_out)
        .output()
        .await
        .with_context(|| format!("spawning {}", tool.display()))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp_out);
        bail!(
            "wasm-opt {} failed ({}):\n{}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::rename(&tmp_out, wasm_path)
        .with_context(|| format!("replacing {}", wasm_path.display()))?;
    let bytes_after = std::fs::metadata(wasm_path)?.len();
    tracing::info!(
        "wasm-opt {}: {:.2} MiB -> {:.2} MiB",
        args.join(" "),
        bytes_before as f64 / (1024.0 * 1024.0),
        bytes_after as f64 / (1024.0 * 1024.0),
    );
    Ok(OptOutcome::Optimized {
        tool,
        bytes_before,
        bytes_after,
    })
}

/// Pinned download first (parity), PATH fallback second.
async fn locate_wasm_opt(target_dir: &Path) -> Result<PathBuf> {
    match crate::tools::ensure_wasm_opt(target_dir, DEFAULT_BINARYEN_VERSION).await {
        Ok(tool) => Ok(tool),
        Err(download_err) => {
            // PATH fallback (version unpinned — logged so a parity
            // surprise is diagnosable).
            let name = if cfg!(windows) {
                "wasm-opt.exe"
            } else {
                "wasm-opt"
            };
            if let Some(found) = which(name) {
                tracing::warn!(
                    "binaryen {DEFAULT_BINARYEN_VERSION} download failed ({download_err:#}); \
                     using `{}` from PATH — version unpinned",
                    found.display()
                );
                return Ok(found);
            }
            Err(download_err)
        }
    }
}

/// Minimal PATH search (std-only; no extra dependency for one lookup).
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parity-critical pin: wasm-pack 0.14 bundles wasm-opt crate
    /// 0.116 = binaryen 116 (verified byte-identical live, PARITY.md).
    #[test]
    fn binaryen_pin_matches_wasm_pack_014() {
        assert_eq!(DEFAULT_BINARYEN_VERSION, "116");
    }

    #[tokio::test]
    async fn disabled_profile_skips() {
        let outcome = run(
            Path::new("/nonexistent"),
            Path::new("/nonexistent/x.wasm"),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, OptOutcome::Skipped { reason } if reason.contains("false")));
    }
}
