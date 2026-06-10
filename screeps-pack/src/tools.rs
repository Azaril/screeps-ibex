//! Version-locked tool acquisition (the trunk pattern,
//! `trunk src/tools.rs:177-199,253-376`): download a prebuilt release
//! archive for the host triple, extract it into a content-named cache
//! directory, return the executable path. The cache lives under the
//! BUILT CRATE's cargo target dir (`<target>/screeps-pack/tools/`), so
//! it is per-project, survives `cargo clean -p`, and never touches
//! host-global state.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// wasm-bindgen release-archive triple for the host.
pub const WASM_BINDGEN_TRIPLE: &str = if cfg!(all(windows, target_arch = "x86_64")) {
    "x86_64-pc-windows-msvc"
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    "x86_64-unknown-linux-musl"
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    "aarch64-apple-darwin"
} else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
    "x86_64-apple-darwin"
} else {
    // Checked at runtime in `wasm_bindgen_url` — keeps the const total.
    "unsupported"
};

/// binaryen release-archive triple for the host.
pub const BINARYEN_TRIPLE: &str = if cfg!(all(windows, target_arch = "x86_64")) {
    "x86_64-windows"
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    "x86_64-linux"
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    "arm64-macos"
} else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
    "x86_64-macos"
} else {
    "unsupported"
};

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// `wasm-bindgen-<v>-<triple>.tar.gz` from the wasm-bindgen org's
/// GitHub releases (trunk `tools.rs:177-191`; the old rustwasm org URL
/// redirects here).
pub fn wasm_bindgen_url(version: &str) -> Result<String> {
    if WASM_BINDGEN_TRIPLE == "unsupported" {
        bail!("no prebuilt wasm-bindgen-cli archive for this host platform");
    }
    Ok(format!(
        "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/{version}/wasm-bindgen-{version}-{WASM_BINDGEN_TRIPLE}.tar.gz"
    ))
}

/// `binaryen-version_<n>-<triple>.tar.gz` from WebAssembly/binaryen
/// releases (trunk `tools.rs:196-199`).
pub fn binaryen_url(version: &str) -> Result<String> {
    if BINARYEN_TRIPLE == "unsupported" {
        bail!("no prebuilt binaryen archive for this host platform");
    }
    Ok(format!(
        "https://github.com/WebAssembly/binaryen/releases/download/version_{version}/binaryen-version_{version}-{BINARYEN_TRIPLE}.tar.gz"
    ))
}

/// The tool cache root for a build: `<target>/screeps-pack/tools`.
pub fn cache_root(target_dir: &Path) -> PathBuf {
    target_dir.join("screeps-pack").join("tools")
}

/// Ensure `wasm-bindgen` of exactly `version` is available; returns the
/// executable path. Cache hit = no network.
pub async fn ensure_wasm_bindgen(target_dir: &Path, version: &str) -> Result<PathBuf> {
    let dir = cache_root(target_dir).join(format!("wasm-bindgen-{version}"));
    let exe_path = dir
        .join(format!("wasm-bindgen-{version}-{WASM_BINDGEN_TRIPLE}"))
        .join(exe("wasm-bindgen"));
    if exe_path.is_file() {
        return Ok(exe_path);
    }
    let url = wasm_bindgen_url(version)?;
    download_and_extract(&url, &dir).await?;
    if !exe_path.is_file() {
        bail!(
            "downloaded {url} but {} is missing — archive layout changed?",
            exe_path.display()
        );
    }
    Ok(exe_path)
}

/// Ensure binaryen's `wasm-opt` of exactly `version` (e.g. `"116"`);
/// returns the executable path. Cache hit = no network.
pub async fn ensure_wasm_opt(target_dir: &Path, version: &str) -> Result<PathBuf> {
    let dir = cache_root(target_dir).join(format!("binaryen-{version}"));
    let exe_path = dir
        .join(format!("binaryen-version_{version}"))
        .join("bin")
        .join(exe("wasm-opt"));
    if exe_path.is_file() {
        return Ok(exe_path);
    }
    let url = binaryen_url(version)?;
    download_and_extract(&url, &dir).await?;
    if !exe_path.is_file() {
        bail!(
            "downloaded {url} but {} is missing — archive layout changed?",
            exe_path.display()
        );
    }
    Ok(exe_path)
}

/// Download a `.tar.gz` and unpack it under `dest` (extraction into a
/// temp sibling + rename, so a torn download never poisons the cache).
async fn download_and_extract(url: &str, dest: &Path) -> Result<()> {
    tracing::info!("downloading {url}");
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("downloading {url}"))?;
    if !response.status().is_success() {
        bail!("downloading {url}: HTTP {}", response.status());
    }
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("reading {url}"))?;

    let parent = dest.parent().context("cache dir has no parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let staging = parent.join(format!(
        ".tmp-{}-{}",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("dl"),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);

    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes.as_ref()));
    tar::Archive::new(gz)
        .unpack(&staging)
        .with_context(|| format!("extracting {url}"))?;

    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&staging, dest)
        .with_context(|| format!("moving extracted archive into {}", dest.display()))?;
    tracing::info!("cached under {}", dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// URL shapes pinned from trunk's tool table + live downloads
    /// (binaryen 116 verified live 2026-06-10).
    #[test]
    fn release_urls_are_pinned() {
        assert_eq!(
            wasm_bindgen_url("0.2.108").unwrap(),
            format!(
                "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/0.2.108/wasm-bindgen-0.2.108-{WASM_BINDGEN_TRIPLE}.tar.gz"
            )
        );
        assert_eq!(
            binaryen_url("116").unwrap(),
            format!(
                "https://github.com/WebAssembly/binaryen/releases/download/version_116/binaryen-version_116-{BINARYEN_TRIPLE}.tar.gz"
            )
        );
    }

    #[test]
    fn cache_paths_live_under_the_target_dir() {
        let root = cache_root(Path::new("/repo/target"));
        assert!(root.starts_with("/repo/target"));
        assert!(root.ends_with(Path::new("screeps-pack").join("tools")));
    }
}
