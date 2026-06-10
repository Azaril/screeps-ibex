//! screeps-pack — npm-free build + deploy for Rust Screeps bots
//! (Phase 0 P0.A13; design: docs/design/rust-native-deploy-investigation.md,
//! verdict GO).
//!
//! Pipeline (each step a module; the CLI in `main.rs` is a thin clap
//! wrapper, and `screeps-server-kit` drives the same functions as a
//! library):
//!
//! 1. [`config`]      — `.screeps.yaml` (SS3) + `configs.wasm-pack-options`
//!    honored per-server, exactly as deploy.js resolved them
//! 2. [`project`]     — bot-crate resolution + metadata knobs
//! 3. [`cargo_build`] — `cargo build --target wasm32-unknown-unknown`
//!    + JSON-stream artifact discovery (trunk pattern)
//! 4. [`bindgen`]     — lockfile-resolved prebuilt wasm-bindgen-cli,
//!    `--target nodejs` (never a static cli-support pin — risk #1)
//! 5. [`glue`]        — anchored patch of the bindgen JS + the embedded
//!    loader template (bucket gate, staged init, #3130 halt trap)
//! 6. [`opt`]         — optional wasm-opt (binaryen pinned for parity;
//!    skips gracefully when unavailable)
//! 7. [`upload`]      — module map -> `POST /api/user/code` via the
//!    shared `screeps-rest-api` client (token or user/pass entries)
//!
//! SECRETS: tokens/passwords ride in `secrecy::SecretString` end to
//! end; nothing secret is ever logged or written into dist artifacts.

pub mod bindgen;
pub mod cargo_build;
pub mod config;
pub mod glue;
pub mod opt;
pub mod project;
pub mod tools;
pub mod upload;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub use config::{ServerAuth, ServerConfig};
pub use project::Project;
pub use upload::ModuleInfo;

/// What to build and where to send it. Library callers pass explicit
/// paths; the CLI defaults both to the invocation directory's
/// `Cargo.toml` / `.screeps.yaml` (the deploy.js/SS3 convention).
#[derive(Debug, Clone)]
pub struct PackOptions {
    /// The bot crate's `Cargo.toml`, or a workspace root containing
    /// exactly one cdylib member.
    pub manifest_path: PathBuf,
    /// The SS3 credentials file.
    pub creds_path: PathBuf,
    /// The `servers:` entry to resolve (build flags + upload target).
    pub server: String,
    /// Debug build (`cargo build` without `--release`; the dev
    /// wasm-opt profile).
    pub debug: bool,
}

/// The result of a pack run (build or deploy).
#[derive(Debug)]
pub struct PackOutcome {
    pub server: String,
    pub branch: String,
    pub mode: &'static str,
    pub module_name: String,
    /// Lockfile-resolved wasm-bindgen version used.
    pub bindgen_version: String,
    /// Human summary of the wasm-opt step.
    pub opt_summary: String,
    /// Uploaded module names/sizes/hashes.
    pub modules: Vec<ModuleInfo>,
    pub used_mib: f64,
    pub used_percent: f64,
    /// Where the dist files + manifest.json were written
    /// (`<target>/screeps-pack/dist/<server>/<mode>/`).
    pub dist_dir: PathBuf,
    pub duration: Duration,
    /// False for `build`/dry runs.
    pub uploaded: bool,
}

impl std::fmt::Display for PackOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} '{}' ({} build, wasm-bindgen {}) in {:.0?}",
            if self.uploaded {
                "deployed to"
            } else {
                "built for"
            },
            self.server,
            self.mode,
            self.bindgen_version,
            self.duration
        )?;
        for m in &self.modules {
            writeln!(
                f,
                "  {:<24} {:>9} bytes  {}  sha256:{}",
                m.name,
                m.bytes,
                m.kind,
                &m.sha256[..12]
            )?;
        }
        writeln!(f, "  wasm-opt: {}", self.opt_summary)?;
        write!(
            f,
            "  branch '{}'; using {:.2} MiB of {:.1} MiB code size limit ({:.2}%)",
            self.branch,
            self.used_mib,
            upload::CODE_SIZE_LIMIT_MIB,
            self.used_percent
        )
    }
}

/// Build the full module map for `options.server` without uploading.
pub async fn build(options: &PackOptions) -> Result<PackOutcome> {
    run_pipeline(options, false).await
}

/// Build and upload to `options.server`.
pub async fn deploy(options: &PackOptions) -> Result<PackOutcome> {
    run_pipeline(options, true).await
}

async fn run_pipeline(options: &PackOptions, do_upload: bool) -> Result<PackOutcome> {
    let start = Instant::now();
    let mode = if options.debug { "debug" } else { "release" };

    // 1+2. config + project
    let server = config::load_server_config(&options.creds_path, &options.server)?;
    let project = project::resolve_project(&options.manifest_path)?;
    tracing::info!(
        "packing {} as module '{}' for server '{}' ({mode})",
        project.package_name,
        project.module_name,
        server.name
    );

    // 3. cargo build + artifact discovery
    let artifact = cargo_build::build(&project, &server, options.debug).await?;

    // The per-server/per-mode dist tree — separate from deploy.js's
    // repo-root dist/ and pkg/ by design (parity plan step 5: no
    // cross-contamination).
    let dist_dir = artifact
        .target_dir
        .join("screeps-pack")
        .join("dist")
        .join(&server.name)
        .join(mode);
    if dist_dir.exists() {
        std::fs::remove_dir_all(&dist_dir)
            .with_context(|| format!("cleaning {}", dist_dir.display()))?;
    }
    std::fs::create_dir_all(&dist_dir)
        .with_context(|| format!("creating {}", dist_dir.display()))?;

    // 4. wasm-bindgen (lockfile-matched, --target nodejs)
    let bindgen_out = bindgen::run(
        &artifact.target_dir,
        &project.crate_dir,
        &artifact.wasm_path,
        &project.module_name,
        &dist_dir,
    )
    .await?;

    // 5. wasm-opt (optional; profile args from the crate metadata)
    let opt_outcome = opt::run(
        &artifact.target_dir,
        &bindgen_out.wasm_path,
        project.wasm_opt_args(options.debug),
    )
    .await?;
    let opt_summary = match &opt_outcome {
        opt::OptOutcome::Optimized {
            bytes_before,
            bytes_after,
            ..
        } => format!(
            "binaryen {}: {:.2} MiB -> {:.2} MiB",
            opt::DEFAULT_BINARYEN_VERSION,
            *bytes_before as f64 / (1024.0 * 1024.0),
            *bytes_after as f64 / (1024.0 * 1024.0)
        ),
        opt::OptOutcome::Skipped { reason } => format!("skipped ({reason})"),
    };

    // 6. glue: patch the bindgen JS + render the loader
    let raw_glue = std::fs::read_to_string(&bindgen_out.js_path)
        .with_context(|| format!("reading {}", bindgen_out.js_path.display()))?;
    let glue_js = glue::patch_bindgen_js(&raw_glue, &project.module_name, &bindgen_out.version)?;
    let loader_js = glue::render_loader(&project.module_name, project.bucket_boot_threshold)?;
    std::fs::write(&bindgen_out.js_path, &glue_js)
        .with_context(|| format!("writing {}", bindgen_out.js_path.display()))?;
    std::fs::write(dist_dir.join("main.js"), &loader_js)
        .with_context(|| format!("writing {}", dist_dir.join("main.js").display()))?;

    // 7. module map (+ manifest), then upload
    let wasm_bytes = std::fs::read(&bindgen_out.wasm_path)
        .with_context(|| format!("reading {}", bindgen_out.wasm_path.display()))?;
    let map = upload::assemble(&project.module_name, &loader_js, &glue_js, &wasm_bytes);
    upload::write_manifest(&dist_dir, &map)?;

    if map.used_mib > upload::CODE_SIZE_LIMIT_MIB {
        tracing::warn!(
            "module map is {:.2} MiB — OVER the {} MiB code limit; the server will \
             likely reject it",
            map.used_mib,
            upload::CODE_SIZE_LIMIT_MIB
        );
    }

    let uploaded = if do_upload {
        tracing::info!(
            "Uploading to branch {}; using {:.2} MiB of {:.1} MiB code size limit ({:.2}%)",
            server.branch,
            map.used_mib,
            upload::CODE_SIZE_LIMIT_MIB,
            map.used_percent
        );
        upload::upload(&server, &map).await?;
        tracing::info!("upload ok");
        true
    } else {
        false
    };

    Ok(PackOutcome {
        server: server.name.clone(),
        branch: server.branch.clone(),
        mode,
        module_name: project.module_name.clone(),
        bindgen_version: bindgen_out.version,
        opt_summary,
        modules: map.infos,
        used_mib: map.used_mib,
        used_percent: map.used_percent,
        dist_dir,
        duration: start.elapsed(),
        uploaded,
    })
}
