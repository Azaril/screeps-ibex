//! screeps-pack CLI — npm-free build + deploy for Rust Screeps bots.
//!
//! Run from a bot crate (or its workspace root): `screeps-pack deploy
//! --server <entry>`. Credentials come from `./.screeps.yaml` (the SS3
//! unified file every screepers tool shares); `--creds`/
//! `--manifest-path` override the conventions.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use screeps_pack::PackOptions;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "screeps-pack",
    about = "npm-free build + deploy for Rust Screeps bots: cargo build (wasm32) -> \
             lockfile-matched wasm-bindgen -> CJS loader glue -> optional wasm-opt -> \
             code upload to any .screeps.yaml server entry",
    version
)]
struct Cli {
    /// Bot crate Cargo.toml (or a workspace root with exactly one
    /// cdylib member). Default: ./Cargo.toml if the current directory
    /// has one, else ../Cargo.toml relative to this crate (the
    /// in-repo layout, where screeps-pack is a workspace-excluded
    /// sibling of the bot — the convention all the tool crates share).
    #[arg(long, global = true)]
    manifest_path: Option<PathBuf>,

    /// SS3 credentials file (servers: entries + configs:). Default:
    /// ./.screeps.yaml if present, else ../.screeps.yaml relative to
    /// this crate (the sibling-crate convention).
    #[arg(long, global = true)]
    creds: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// Resolve a default path: prefer `name` in the current directory
/// (external users running from their bot crate), falling back to
/// `../name` anchored at this crate (the in-repo sibling layout).
/// An explicit flag always wins and is used verbatim.
fn resolve_default(explicit: Option<PathBuf>, name: &str) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    let cwd_candidate = PathBuf::from(name);
    // "Exists in cwd" wins — except when that file is screeps-pack's OWN
    // copy (running `cargo run` from inside this crate in-repo): the
    // tool's own manifest is never the bot, so fall through to the
    // sibling convention.
    let is_own_file = || {
        let own = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
        matches!(
            (cwd_candidate.canonicalize(), own.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
    };
    if cwd_candidate.exists() && !is_own_file() {
        return cwd_candidate;
    }
    let crate_anchored = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(name);
    if crate_anchored.exists() {
        return crate_anchored;
    }
    // Neither exists: return the cwd form so the error message names
    // the conventional location.
    cwd_candidate
}

#[derive(Subcommand)]
enum Command {
    /// Build and upload to a .screeps.yaml servers: entry
    Deploy {
        /// Server entry: build flags (configs.wasm-pack-options) and
        /// upload target
        #[arg(long)]
        server: String,
        /// Debug build (no --release; the dev wasm-opt profile)
        #[arg(long)]
        debug: bool,
        /// Build everything, print the module map, upload nothing
        #[arg(long)]
        dryrun: bool,
    },
    /// Build the full module map without uploading (same artifacts
    /// `deploy` would push; written under <target>/screeps-pack/dist/)
    Build {
        #[arg(long)]
        server: String,
        #[arg(long)]
        debug: bool,
    },
    /// Resolve and print the plan: server entry, bot crate, cargo argv,
    /// wasm-bindgen version — no build, no upload
    Check {
        #[arg(long)]
        server: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "screeps_pack=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let manifest_path = resolve_default(cli.manifest_path, "Cargo.toml");
    let creds = resolve_default(cli.creds, ".screeps.yaml");
    match cli.command {
        Command::Deploy {
            server,
            debug,
            dryrun,
        } => {
            let options = PackOptions {
                manifest_path: manifest_path.clone(),
                creds_path: creds.clone(),
                server,
                debug,
            };
            let outcome = if dryrun {
                let outcome = screeps_pack::build(&options).await?;
                println!("Not uploading due to --dryrun");
                outcome
            } else {
                screeps_pack::deploy(&options).await?
            };
            println!("{outcome}");
            Ok(())
        }
        Command::Build { server, debug } => {
            let options = PackOptions {
                manifest_path: manifest_path.clone(),
                creds_path: creds.clone(),
                server,
                debug,
            };
            let outcome = screeps_pack::build(&options).await?;
            println!("{outcome}");
            Ok(())
        }
        Command::Check { server } => check(manifest_path, creds, server),
    }
}

/// Resolve everything resolvable without building, and print the plan.
/// Secrets never print (SecretString redacts by construction).
fn check(manifest_path: PathBuf, creds: PathBuf, server: String) -> Result<()> {
    let server_cfg = screeps_pack::config::load_server_config(&creds, &server)?;
    println!(
        "server '{}': {} (branch '{}', {} auth)",
        server_cfg.name,
        server_cfg.base_url(),
        server_cfg.branch,
        match server_cfg.auth {
            screeps_pack::ServerAuth::Token(_) => "token",
            screeps_pack::ServerAuth::UserPass { .. } => "username/password",
        }
    );

    let project = screeps_pack::project::resolve_project(&manifest_path)?;
    println!(
        "bot crate: {} at {} (modules: main, {}, {})",
        project.package_name,
        project.crate_dir.display(),
        project.module_name,
        project.wasm_module_name()
    );
    println!(
        "bucket boot threshold: {}; wasm-opt release {:?}, dev {:?}",
        project.bucket_boot_threshold,
        project.wasm_opt_release.as_deref().unwrap_or_default(),
        project.wasm_opt_dev.as_deref().unwrap_or_default()
    );

    for (label, debug) in [("release", false), ("debug", true)] {
        println!(
            "cargo argv ({label}): cargo {}",
            screeps_pack::cargo_build::build_args(&server_cfg, debug).join(" ")
        );
    }

    let bindgen_version = screeps_pack::bindgen::resolve_version(&project.crate_dir)
        .context("resolving wasm-bindgen from the lockfile")?;
    println!("wasm-bindgen: {bindgen_version} (from the crate's Cargo.lock)");
    println!(
        "wasm-bindgen archive: {}",
        screeps_pack::tools::wasm_bindgen_url(&bindgen_version)?
    );
    println!(
        "binaryen (wasm-opt) pin: {}",
        screeps_pack::opt::DEFAULT_BINARYEN_VERSION
    );
    Ok(())
}
