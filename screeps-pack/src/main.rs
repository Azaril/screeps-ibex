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
    /// cdylib member)
    #[arg(long, global = true, default_value = "Cargo.toml")]
    manifest_path: PathBuf,

    /// SS3 credentials file (servers: entries + configs:)
    #[arg(long, global = true, default_value = ".screeps.yaml")]
    creds: PathBuf,

    #[command(subcommand)]
    command: Command,
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
    match cli.command {
        Command::Deploy {
            server,
            debug,
            dryrun,
        } => {
            let options = PackOptions {
                manifest_path: cli.manifest_path,
                creds_path: cli.creds,
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
                manifest_path: cli.manifest_path,
                creds_path: cli.creds,
                server,
                debug,
            };
            let outcome = screeps_pack::build(&options).await?;
            println!("{outcome}");
            Ok(())
        }
        Command::Check { server } => check(cli.manifest_path, cli.creds, server),
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
