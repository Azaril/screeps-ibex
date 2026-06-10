//! Server-stack lifecycle via bollard <-> Docker Desktop (P0.A2).
//!
//! Manages the screeps-launcher stack natively through the Docker API
//! (named pipe on Windows via `connect_with_local_defaults`) — no
//! docker-compose dependency. The compose file in the local launcher
//! clone (`C:\code\screeps-launcher\docker-compose.yml`) stays the
//! *reference* for the topology mirrored here:
//!
//! - `mongo:8`    volume `screeps-eval-mongo:/data/db`, alias `mongo`
//! - `redis:7`    volume `screeps-eval-redis:/data`,    alias `redis`
//! - the launcher image (`image.name` in `config/local.yml`, default
//!   `screepers/screeps-launcher:latest` pulled from the registry;
//!   optionally BUILT from a launcher-repo clone via `image.build` —
//!   P0.A9(d)): volume `screeps-eval-data:/screeps`, the merged
//!   runtime config bind-mounted at `/screeps/config.yml`, env
//!   `MONGO_HOST=mongo` / `REDIS_HOST=redis` (consumed by
//!   screepsmod-mongo; the launcher passes its own environment through
//!   to the server processes — launcher/server.go `os.Environ()`),
//!   game + CLI ports published per `ports`.
//!
//! Everything is named `screeps-eval-*` so the stack is recognizable
//! and `destroy` cannot touch anything else.
//!
//! NOTE (P0.A14): the `screeps-eval-*` object names predate this crate's
//! split/rename to screeps-server-kit and are KEPT deliberately —
//! renaming them would orphan every existing stack (containers, the
//! installed-server data volume, the world databases) on operator
//! machines and force a cold first boot. Candidate rename at the D-1
//! extraction, behind an explicit migration.

use crate::config::{ImageSettings, StackSettings};
use crate::server_config;
use anyhow::{bail, Context, Result};
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, EndpointSettings, HealthConfig, HostConfig,
    NetworkCreateRequest, NetworkingConfig, PortBinding, VolumeCreateOptions,
};
use bollard::query_parameters::{
    BuildImageOptionsBuilder, CreateContainerOptionsBuilder, CreateImageOptionsBuilder,
    InspectContainerOptions, ListContainersOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, RemoveVolumeOptions, StartContainerOptions,
    StopContainerOptionsBuilder,
};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

pub const NETWORK: &str = "screeps-eval-net";
pub const LAUNCHER_CONTAINER: &str = "screeps-eval-launcher";
pub const MONGO_CONTAINER: &str = "screeps-eval-mongo-db";
pub const REDIS_CONTAINER: &str = "screeps-eval-redis-db";
pub const DATA_VOLUME: &str = "screeps-eval-data";
pub const MONGO_VOLUME: &str = "screeps-eval-mongo";
pub const REDIS_VOLUME: &str = "screeps-eval-redis";

/// Image policy (pull by default, optional local build): the launcher
/// image name comes from the `image:` config block (default
/// [`crate::config::DEFAULT_LAUNCHER_IMAGE`], pulled; with
/// `image.build` configured it is built from a launcher-repo clone).
/// The databases are always pulled.
pub const MONGO_IMAGE: &str = "mongo:8";
pub const REDIS_IMAGE: &str = "redis:7";

/// First-boot budget: the launcher runs an in-container `npm install`
/// of the whole server + mods on first start (plan: "10 min+").
pub const FIRST_BOOT_BUDGET: Duration = Duration::from_secs(12 * 60);
/// Warm-restart budget: node_modules already present in the data volume.
pub const WARM_BOOT_BUDGET: Duration = Duration::from_secs(4 * 60);

/// Connect to the local Docker daemon (named pipe `//./pipe/docker_engine`
/// on Windows — Docker Desktop must be running).
pub fn connect() -> Result<Docker> {
    Docker::connect_with_local_defaults()
        .context("connecting to Docker — is Docker Desktop running?")
}

fn is_not_found(e: &bollard::errors::Error) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn is_not_modified(e: &bollard::errors::Error) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 304,
            ..
        }
    )
}

// ---------------------------------------------------------------- images

/// Pull `image` if it is not already present locally (pull-not-build).
pub async fn ensure_image(docker: &Docker, image: &str) -> Result<()> {
    match docker.inspect_image(image).await {
        Ok(_) => {
            tracing::debug!(image, "image already present");
            return Ok(());
        }
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e).with_context(|| format!("inspecting image {image}")),
    }
    tracing::info!(image, "pulling image (not present locally)");
    let opts = CreateImageOptionsBuilder::new().from_image(image).build();
    let mut stream = docker.create_image(Some(opts), None, None);
    let mut last_log = Instant::now();
    while let Some(item) = stream.next().await {
        let info = item.with_context(|| format!("pulling image {image}"))?;
        // Progress, throttled — a full pull emits thousands of events.
        if last_log.elapsed() > Duration::from_secs(3) {
            let status = info.status.as_deref().unwrap_or("...");
            let progress = info.progress.as_deref().unwrap_or("");
            tracing::info!(image, "{status} {progress}");
            last_log = Instant::now();
        }
    }
    tracing::info!(image, "pull complete");
    Ok(())
}

/// Ensure the launcher image exists: present locally -> use it; absent
/// with `image.build` configured -> build it; absent otherwise -> pull
/// (P0.A9(d)).
pub async fn ensure_launcher_image(docker: &Docker, image: &ImageSettings) -> Result<()> {
    match docker.inspect_image(&image.name).await {
        Ok(_) => {
            tracing::debug!(image = %image.name, "launcher image already present");
            return Ok(());
        }
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e).with_context(|| format!("inspecting image {}", image.name)),
    }
    if image.build.is_some() {
        build_launcher_image(docker, image).await
    } else {
        ensure_image(docker, &image.name).await
    }
}

/// Build the launcher image from the configured context (P0.A9(d);
/// `server build-image`, or automatically by `up` when the image is
/// absent). Streams build output through tracing; bails on the first
/// in-band build error. Docker's layer cache applies — an explicit
/// rebuild after upstream changes is cheap.
pub async fn build_launcher_image(docker: &Docker, image: &ImageSettings) -> Result<()> {
    let Some(build) = &image.build else {
        bail!(
            "image.build is not configured in config/local.yml — `server build-image` \
             needs an image.build.context pointing at a full screepers/screeps-launcher \
             clone (see config/local.example.yml)"
        );
    };
    let dockerfile = build.dockerfile_name();
    let tar_bytes = build_context_tar(&build.context, dockerfile)?;
    // bollard's BuildImageOptions ALWAYS serializes `platform`, and an
    // empty value is rejected by newer daemons (Docker 29, live
    // 2026-06-10: `failed to parse platform : "" is an invalid OS
    // component`). Build for the daemon's own platform: `/version`
    // reports Go-convention `Os`/`Arch` values ("linux"/"amd64"),
    // which are exactly the components the build endpoint accepts.
    let version = docker
        .version()
        .await
        .context("querying the daemon version (for the build platform)")?;
    let platform = match (version.os.as_deref(), version.arch.as_deref()) {
        (Some(os), Some(arch)) => format!("{os}/{arch}"),
        _ => bail!("the Docker daemon did not report Os/Arch — cannot pick a build platform"),
    };
    tracing::info!(
        image = %image.name,
        context = %build.context.display(),
        dockerfile,
        platform = %platform,
        tar_kib = tar_bytes.len() / 1024,
        "building launcher image (BuildKit)"
    );
    // BuildKit, not the classic builder: the upstream launcher
    // Dockerfile uses `FROM --platform=$BUILDPLATFORM`, `RUN
    // --mount=type=cache`, and heredoc RUN blocks — all BuildKit-only
    // (the classic builder leaves $BUILDPLATFORM empty and fails with
    // `failed to parse platform : ""`; verified live 2026-06-10).
    // BuildKit requires a session id alongside the builder version.
    let session = format!("screeps-server-kit-build-{}", std::process::id());
    let options = BuildImageOptionsBuilder::new()
        .t(&image.name)
        .dockerfile(dockerfile)
        .platform(&platform)
        .rm(true)
        .version(bollard::query_parameters::BuilderVersion::BuilderBuildKit)
        .session(&session)
        .build();
    let mut stream = docker.build_image(
        options,
        None,
        Some(bollard::body_full(bytes::Bytes::from(tar_bytes))),
    );
    let mut last_log = Instant::now();
    while let Some(item) = stream.next().await {
        let info = item.with_context(|| format!("building image {}", image.name))?;
        if let Some(error) = info.error {
            let detail = info
                .error_detail
                .and_then(|d| d.message)
                .unwrap_or_default();
            bail!("image build failed: {error} {detail}");
        }
        // Classic-builder text lines (kept for non-BuildKit daemons).
        if let Some(line) = info.stream {
            let line = line.trim_end();
            if !line.is_empty() {
                tracing::info!(target: "docker_build", "{line}");
            }
        }
        // BuildKit progress arrives as status graphs; surface vertex
        // errors immediately, log step names (throttled).
        if let Some(bollard::models::BuildInfoAux::BuildKit(status)) = info.aux {
            for vertex in &status.vertexes {
                if !vertex.error.is_empty() {
                    bail!("image build failed: {} ({})", vertex.error, vertex.name);
                }
                if vertex.started.is_some()
                    && vertex.completed.is_none()
                    && last_log.elapsed() > Duration::from_secs(3)
                {
                    tracing::info!(target: "docker_build", "{}", vertex.name);
                    last_log = Instant::now();
                }
            }
        }
    }
    // Verify the tag actually exists (a stream that ends without error
    // but also without tagging would otherwise pass silently).
    docker
        .inspect_image(&image.name)
        .await
        .with_context(|| format!("image {} not present after the build", image.name))?;
    tracing::info!(image = %image.name, "image build complete");
    Ok(())
}

/// PURE-ish (filesystem in, bytes out — no Docker): tar the build
/// context for the Docker build API. Validates first: the context must
/// be a directory containing the named Dockerfile — the upstream
/// screepers/screeps-launcher repo root qualifies; a config-only
/// directory (compose file + config.yml, like a typical local launcher
/// *deployment* folder) does NOT, and gets a pointed error. `.git`
/// directories are skipped (they dwarf the actual context). Entries use
/// forward-slash relative names, sorted for determinism.
pub fn build_context_tar(context: &Path, dockerfile: &str) -> Result<Vec<u8>> {
    if !context.is_dir() {
        bail!(
            "image.build.context {} is not a directory — point it at a full clone of \
             https://github.com/screepers/screeps-launcher",
            context.display()
        );
    }
    if !context.join(dockerfile).is_file() {
        bail!(
            "image.build.context {} has no {dockerfile} — the Dockerfile lives at the \
             ROOT of the upstream screepers/screeps-launcher repo; a config-only \
             directory (docker-compose.yml + config.yml) is not buildable. Clone the \
             upstream repo and point image.build.context at the clone",
            context.display()
        );
    }
    let mut builder = tar::Builder::new(Vec::new());
    builder.follow_symlinks(false);
    append_dir_recursive(&mut builder, context, "")?;
    builder
        .into_inner()
        .context("finalizing the build-context tar")
}

/// Append a directory's contents under `prefix` (forward-slash relative
/// names; deterministic order; `.git` skipped at any depth).
fn append_dir_recursive(
    builder: &mut tar::Builder<Vec<u8>>,
    dir: &Path,
    prefix: &str,
) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading build context dir {}", dir.display()))?
        .collect::<std::io::Result<_>>()
        .with_context(|| format!("reading build context dir {}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            bail!(
                "non-UTF-8 file name {:?} in build context {}",
                name,
                dir.display()
            );
        };
        if name == ".git" {
            continue;
        }
        let tar_name = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat {}", path.display()))?;
        if file_type.is_dir() {
            builder
                .append_dir(&tar_name, &path)
                .with_context(|| format!("adding dir {} to the build tar", path.display()))?;
            append_dir_recursive(builder, &path, &tar_name)?;
        } else if file_type.is_file() {
            let mut file = std::fs::File::open(&path)
                .with_context(|| format!("opening {}", path.display()))?;
            builder
                .append_file(&tar_name, &mut file)
                .with_context(|| format!("adding {} to the build tar", path.display()))?;
        }
        // Symlinks and other special files are skipped (follow_symlinks
        // is off; the launcher repo has none that matter for the build).
    }
    Ok(())
}

// ------------------------------------------------------ network / volumes

pub async fn ensure_network(docker: &Docker) -> Result<()> {
    match docker
        .inspect_network(
            NETWORK,
            None::<bollard::query_parameters::InspectNetworkOptions>,
        )
        .await
    {
        Ok(_) => return Ok(()),
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e).context("inspecting network"),
    }
    tracing::info!(network = NETWORK, "creating network");
    docker
        .create_network(NetworkCreateRequest {
            name: NETWORK.to_string(),
            ..Default::default()
        })
        .await
        .context("creating network")?;
    Ok(())
}

pub async fn ensure_volume(docker: &Docker, name: &str) -> Result<()> {
    match docker.inspect_volume(name).await {
        Ok(_) => return Ok(()),
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e).with_context(|| format!("inspecting volume {name}")),
    }
    tracing::info!(volume = name, "creating volume");
    docker
        .create_volume(VolumeCreateOptions {
            name: Some(name.to_string()),
            ..Default::default()
        })
        .await
        .with_context(|| format!("creating volume {name}"))?;
    Ok(())
}

// ------------------------------------------------------------- containers

async fn inspect_opt(docker: &Docker, name: &str) -> Result<Option<ContainerInspectResponse>> {
    match docker
        .inspect_container(name, None::<InspectContainerOptions>)
        .await
    {
        Ok(c) => Ok(Some(c)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("inspecting container {name}")),
    }
}

fn is_running(inspect: &ContainerInspectResponse) -> bool {
    inspect
        .state
        .as_ref()
        .and_then(|s| s.running)
        .unwrap_or(false)
}

/// Create-if-absent + start-if-stopped. Returns `true` if the container
/// was created fresh (used to pick the first-boot vs warm budget).
async fn ensure_container_running(
    docker: &Docker,
    name: &str,
    body: ContainerCreateBody,
) -> Result<bool> {
    let mut created = false;
    let existing = inspect_opt(docker, name).await?;
    match existing {
        Some(c) if is_running(&c) => {
            tracing::debug!(container = name, "already running");
            return Ok(false);
        }
        Some(_) => {
            tracing::info!(container = name, "starting existing container (warm)");
        }
        None => {
            tracing::info!(container = name, "creating container");
            let opts = CreateContainerOptionsBuilder::new().name(name).build();
            docker
                .create_container(Some(opts), body)
                .await
                .with_context(|| format!("creating container {name}"))?;
            created = true;
        }
    }
    docker
        .start_container(name, None::<StartContainerOptions>)
        .await
        .with_context(|| format!("starting container {name}"))?;
    Ok(created)
}

/// Health-config helper (compose-reference healthchecks, faster cadence
/// so `up` converges quickly). Times are nanoseconds.
fn health(test: Vec<&str>) -> HealthConfig {
    const SEC: i64 = 1_000_000_000;
    HealthConfig {
        test: Some(test.into_iter().map(String::from).collect()),
        interval: Some(5 * SEC),
        timeout: Some(5 * SEC),
        retries: Some(24),
        start_period: Some(5 * SEC),
        ..Default::default()
    }
}

fn endpoint_alias(alias: &str) -> Option<NetworkingConfig> {
    Some(NetworkingConfig {
        endpoints_config: Some(HashMap::from([(
            NETWORK.to_string(),
            EndpointSettings {
                aliases: Some(vec![alias.to_string()]),
                ..Default::default()
            },
        )])),
    })
}

fn mongo_body() -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(MONGO_IMAGE.to_string()),
        healthcheck: Some(health(vec![
            "CMD-SHELL",
            r#"echo 'db.runCommand("ping").ok' | mongosh localhost:27017/test --quiet"#,
        ])),
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{MONGO_VOLUME}:/data/db")]),
            network_mode: Some(NETWORK.to_string()),
            ..Default::default()
        }),
        networking_config: endpoint_alias("mongo"),
        ..Default::default()
    }
}

fn redis_body() -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(REDIS_IMAGE.to_string()),
        healthcheck: Some(health(vec!["CMD", "redis-cli", "ping"])),
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{REDIS_VOLUME}:/data")]),
            network_mode: Some(NETWORK.to_string()),
            ..Default::default()
        }),
        networking_config: endpoint_alias("redis"),
        ..Default::default()
    }
}

fn launcher_body(
    image: &str,
    runtime_config: &Path,
    game_port: u16,
    cli_port: u16,
) -> ContainerCreateBody {
    let game_key = format!("{game_port}/tcp");
    let cli_key = format!("{cli_port}/tcp");
    let binding = |host_port: u16| {
        Some(vec![PortBinding {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(host_port.to_string()),
        }])
    };
    ContainerCreateBody {
        image: Some(image.to_string()),
        // MONGO_HOST/REDIS_HOST: consumed by screepsmod-mongo inside the
        // server processes — the launcher passes its container env through
        // (launcher/server.go builds child env from os.Environ()).
        env: Some(vec![
            "MONGO_HOST=mongo".to_string(),
            "REDIS_HOST=redis".to_string(),
        ]),
        // Container-side ports equal the host-side ports because the merge
        // forces env.backend GAME_PORT/CLI_PORT to eval.ports — one number
        // per port end to end.
        exposed_ports: Some(HashMap::from([
            (game_key.clone(), HashMap::new()),
            (cli_key.clone(), HashMap::new()),
        ])),
        host_config: Some(HostConfig {
            binds: Some(vec![
                // Merged runtime config (carries the steamKey; gitignored
                // location — P0.A7(b)) over the data volume, like the
                // compose reference.
                format!("{}:/screeps/config.yml", runtime_config.display()),
                format!("{DATA_VOLUME}:/screeps"),
            ]),
            port_bindings: Some(HashMap::from([
                (game_key, binding(game_port)),
                (cli_key, binding(cli_port)),
            ])),
            network_mode: Some(NETWORK.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ------------------------------------------------------------------- up

/// Bring the stack up: merge+write the runtime config, pull missing
/// images, create network/volumes/containers as needed, start
/// everything, and wait until the game API answers.
pub async fn up(stack: &StackSettings) -> Result<()> {
    // Fail fast on config problems (e.g. no steamKey anywhere) before
    // touching Docker at all.
    let runtime_config = server_config::prepare_runtime_config(stack)?;
    tracing::info!(
        path = %runtime_config.display(),
        "merged launcher config written (gitignored; carries the steamKey — never commit/log it)"
    );

    let docker = connect()?;
    let version = docker.version().await.context("querying Docker version")?;
    tracing::info!(
        docker = version.version.as_deref().unwrap_or("?"),
        "connected to Docker daemon"
    );

    for image in [MONGO_IMAGE, REDIS_IMAGE] {
        ensure_image(&docker, image).await?;
    }
    ensure_launcher_image(&docker, &stack.image).await?;
    ensure_network(&docker).await?;
    for volume in [DATA_VOLUME, MONGO_VOLUME, REDIS_VOLUME] {
        ensure_volume(&docker, volume).await?;
    }

    // Databases first (compose: depends_on service_healthy).
    ensure_container_running(&docker, MONGO_CONTAINER, mongo_body()).await?;
    ensure_container_running(&docker, REDIS_CONTAINER, redis_body()).await?;
    wait_container_healthy(&docker, MONGO_CONTAINER, Duration::from_secs(120)).await?;
    wait_container_healthy(&docker, REDIS_CONTAINER, Duration::from_secs(120)).await?;

    warn_if_launcher_ports_stale(&docker, stack).await?;
    let created = ensure_container_running(
        &docker,
        LAUNCHER_CONTAINER,
        launcher_body(
            &stack.image.name,
            &runtime_config,
            stack.game_port,
            stack.cli_port,
        ),
    )
    .await?;

    let budget = if created {
        FIRST_BOOT_BUDGET
    } else {
        WARM_BOOT_BUDGET
    };
    if created {
        tracing::info!(
            "first boot: the launcher npm-installs the server + mods in-container; \
             this can take ~10 minutes (budget {}s) — progress below",
            budget.as_secs()
        );
    }
    let elapsed = wait_api_ready(&docker, stack.game_port, budget).await?;
    tracing::info!(
        "server is up: game API answering on http://127.0.0.1:{} after {:.0?}",
        stack.game_port,
        elapsed
    );
    Ok(())
}

/// An existing launcher container keeps the published ports it was
/// created with — if the configured `ports` changed since, `up` would silently
/// publish the old ones. Detect and tell the operator what to do.
async fn warn_if_launcher_ports_stale(docker: &Docker, stack: &StackSettings) -> Result<()> {
    let Some(inspect) = inspect_opt(docker, LAUNCHER_CONTAINER).await? else {
        return Ok(());
    };
    let bindings = inspect
        .host_config
        .and_then(|hc| hc.port_bindings)
        .unwrap_or_default();
    let want_game = format!("{}/tcp", stack.game_port);
    let want_cli = format!("{}/tcp", stack.cli_port);
    if !bindings.contains_key(&want_game) || !bindings.contains_key(&want_cli) {
        tracing::warn!(
            "existing launcher container publishes {:?}, but the configured ports want {}/{} — \
             ports are fixed at container creation; run `server destroy --yes` then `server up`",
            bindings.keys().collect::<Vec<_>>(),
            stack.game_port,
            stack.cli_port
        );
    }
    Ok(())
}

async fn wait_container_healthy(docker: &Docker, name: &str, budget: Duration) -> Result<()> {
    use bollard::models::HealthStatusEnum;
    let start = Instant::now();
    loop {
        let inspect = inspect_opt(docker, name)
            .await?
            .with_context(|| format!("container {name} vanished while waiting for health"))?;
        if !is_running(&inspect) && start.elapsed() > Duration::from_secs(5) {
            let logs = tail_logs(docker, name, 15).await;
            bail!(
                "container {name} is not running; last logs:\n{}",
                logs.join("\n")
            );
        }
        let status = inspect
            .state
            .as_ref()
            .and_then(|s| s.health.as_ref())
            .and_then(|h| h.status);
        if status == Some(HealthStatusEnum::HEALTHY) {
            tracing::info!(container = name, "healthy after {:.0?}", start.elapsed());
            return Ok(());
        }
        if start.elapsed() > budget {
            let logs = tail_logs(docker, name, 15).await;
            bail!(
                "container {name} not healthy after {budget:?} (status {status:?}); last logs:\n{}",
                logs.join("\n")
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Poll the game API (`/api/version`) until it answers 200, with early
/// abort if the launcher container dies, and progress lines showing the
/// launcher's latest log output (first boot is a long npm install).
pub async fn wait_api_ready(docker: &Docker, game_port: u16, budget: Duration) -> Result<Duration> {
    let url = format!("http://127.0.0.1:{game_port}/api/version");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("building HTTP client")?;
    let start = Instant::now();
    let mut last_progress = Instant::now();
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(start.elapsed());
            }
        }
        if start.elapsed() > budget {
            let logs = tail_logs(docker, LAUNCHER_CONTAINER, 25).await;
            bail!(
                "game API at {url} not ready after {budget:?}; \
                 inspect with `server logs`. Last launcher logs:\n{}",
                logs.join("\n")
            );
        }
        // Early abort: a crashed launcher will never become ready.
        if let Some(inspect) = inspect_opt(docker, LAUNCHER_CONTAINER).await? {
            if !is_running(&inspect) {
                let logs = tail_logs(docker, LAUNCHER_CONTAINER, 25).await;
                bail!(
                    "launcher container exited while waiting for the API; last logs:\n{}",
                    logs.join("\n")
                );
            }
        }
        if last_progress.elapsed() > Duration::from_secs(15) {
            let last = tail_logs(docker, LAUNCHER_CONTAINER, 1).await;
            tracing::info!(
                "waiting for game API ({:.0?} elapsed) — launcher: {}",
                start.elapsed(),
                last.last().map(String::as_str).unwrap_or("(no logs yet)")
            );
            last_progress = Instant::now();
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn tail_logs(docker: &Docker, name: &str, n: usize) -> Vec<String> {
    let opts = LogsOptionsBuilder::new()
        .stdout(true)
        .stderr(true)
        .tail(&n.to_string())
        .build();
    let mut stream = docker.logs(name, Some(opts));
    let mut lines = Vec::new();
    while let Some(Ok(msg)) = stream.next().await {
        let text = String::from_utf8_lossy(&msg.into_bytes())
            .trim_end()
            .to_string();
        if !text.is_empty() {
            lines.push(text);
        }
    }
    lines
}

// ----------------------------------------------------------------- logs

/// Tail (and optionally follow) the launcher container's logs to stdout.
pub async fn logs(follow: bool, tail: u32) -> Result<()> {
    let docker = connect()?;
    let name = find_launcher(&docker, None)
        .await?
        .context("no launcher container found — `server up` first")?;
    let opts = LogsOptionsBuilder::new()
        .stdout(true)
        .stderr(true)
        .follow(follow)
        .tail(&tail.to_string())
        .build();
    let mut stream = docker.logs(&name, Some(opts));
    while let Some(item) = stream.next().await {
        match item {
            Ok(msg) => print!("{}", String::from_utf8_lossy(&msg.into_bytes())),
            Err(e) => bail!("log stream error: {e}"),
        }
    }
    Ok(())
}

// --------------------------------------------------------------- status

/// Locate the launcher container: our canonical name first, then any
/// container running a launcher-ish image (the configured name when
/// known, or anything `screeps-launcher` — covers a manually-started
/// stack, e.g. a compose deployment).
async fn find_launcher(docker: &Docker, image_name: Option<&str>) -> Result<Option<String>> {
    if inspect_opt(docker, LAUNCHER_CONTAINER).await?.is_some() {
        return Ok(Some(LAUNCHER_CONTAINER.to_string()));
    }
    let containers = docker
        .list_containers(Some(ListContainersOptionsBuilder::new().all(true).build()))
        .await
        .context("listing containers")?;
    for c in containers {
        if c.image
            .as_deref()
            .is_some_and(|i| i.contains("screeps-launcher") || Some(i) == image_name)
        {
            if let Some(name) = c.names.and_then(|n| n.first().cloned()) {
                return Ok(Some(name.trim_start_matches('/').to_string()));
            }
        }
    }
    Ok(None)
}

/// Human-readable status: container table (state, health, the ACTUAL
/// published ports discovered from inspect — works against a manually-
/// started stack too), plus a live API ping and a CLI TCP probe.
pub async fn status(stack: &StackSettings) -> Result<String> {
    let docker = connect()?;
    let mut out = String::new();
    let launcher = find_launcher(&docker, Some(&stack.image.name)).await?;

    let mut rows: Vec<[String; 5]> = vec![[
        "CONTAINER".into(),
        "IMAGE".into(),
        "STATE".into(),
        "HEALTH".into(),
        "PUBLISHED PORTS".into(),
    ]];
    let mut discovered_game_port = None;
    let mut discovered_cli_port = None;

    let names: Vec<String> = launcher
        .clone()
        .into_iter()
        .chain([MONGO_CONTAINER.to_string(), REDIS_CONTAINER.to_string()])
        .collect();
    for name in &names {
        let Some(inspect) = inspect_opt(&docker, name).await? else {
            rows.push([
                name.clone(),
                "-".into(),
                "not found".into(),
                "-".into(),
                "-".into(),
            ]);
            continue;
        };
        let image = inspect
            .config
            .as_ref()
            .and_then(|c| c.image.clone())
            .unwrap_or_else(|| "?".into());
        let state = inspect
            .state
            .as_ref()
            .and_then(|s| s.status)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "?".into());
        let health = inspect
            .state
            .as_ref()
            .and_then(|s| s.health.as_ref())
            .and_then(|h| h.status)
            .map(|h| h.to_string())
            .unwrap_or_else(|| "-".into());

        // ACTUAL published ports, from inspect — not from our config.
        let mut ports = Vec::new();
        if let Some(port_map) = inspect
            .network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
        {
            let mut entries: Vec<_> = port_map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (container_port, bindings) in entries {
                for b in bindings.iter().flatten() {
                    let host_ip = b.host_ip.as_deref().unwrap_or("?");
                    let host_port = b.host_port.as_deref().unwrap_or("?");
                    ports.push(format!("{container_port} -> {host_ip}:{host_port}"));
                    // Map container-side ports to discovered host ports
                    // using the configured ports (fall back to launcher
                    // defaults 21025/21026 for a foreign stack).
                    let host_port_num: Option<u16> = host_port.parse().ok();
                    if container_port == &format!("{}/tcp", stack.game_port)
                        || container_port == "21025/tcp"
                    {
                        discovered_game_port = discovered_game_port.or(host_port_num);
                    } else if container_port == &format!("{}/tcp", stack.cli_port)
                        || container_port == "21026/tcp"
                    {
                        discovered_cli_port = discovered_cli_port.or(host_port_num);
                    }
                }
            }
        }
        let ports = if ports.is_empty() {
            "-".to_string()
        } else {
            ports.join(", ")
        };
        rows.push([name.clone(), image, state, health, ports]);
    }

    // Column-aligned table.
    let mut widths = [0usize; 5];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            out.push_str(&format!("{:<width$}  ", cell, width = widths[i]));
        }
        out.push('\n');
    }
    out.push('\n');

    // Live probes on the DISCOVERED ports (config ports as fallback).
    let game_port = discovered_game_port.unwrap_or(stack.game_port);
    let cli_port = discovered_cli_port.unwrap_or(stack.cli_port);

    let api_url = format!("http://127.0.0.1:{game_port}/api/version");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    match client.get(&api_url).send().await {
        Ok(resp) => out.push_str(&format!("game API  {api_url} -> {}\n", resp.status())),
        Err(e) => out.push_str(&format!(
            "game API  {api_url} -> unreachable ({})\n",
            e.without_url()
        )),
    }

    let cli_addr = format!("127.0.0.1:{cli_port}");
    match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&cli_addr),
    )
    .await
    {
        Ok(Ok(_)) => out.push_str(&format!("server CLI  tcp://{cli_addr} -> connectable\n")),
        Ok(Err(e)) => out.push_str(&format!(
            "server CLI  tcp://{cli_addr} -> {e} \
             (fallback: docker exec -it {} screeps-launcher cli)\n",
            launcher.as_deref().unwrap_or(LAUNCHER_CONTAINER)
        )),
        Err(_) => out.push_str(&format!(
            "server CLI  tcp://{cli_addr} -> connect timeout\n"
        )),
    }

    Ok(out)
}

// ----------------------------------------------------------- down/destroy

/// Stop the stack's containers. Volumes, network, and containers are
/// kept — the next `up` is a warm restart.
pub async fn down() -> Result<()> {
    let docker = connect()?;
    // Launcher first so the server shuts down before its databases.
    for (name, grace) in [
        (LAUNCHER_CONTAINER, 30),
        (MONGO_CONTAINER, 15),
        (REDIS_CONTAINER, 15),
    ] {
        let opts = StopContainerOptionsBuilder::new().t(grace).build();
        match docker.stop_container(name, Some(opts)).await {
            Ok(()) => tracing::info!(container = name, "stopped"),
            Err(e) if is_not_found(&e) => tracing::info!(container = name, "not present"),
            Err(e) if is_not_modified(&e) => tracing::info!(container = name, "already stopped"),
            Err(e) => return Err(e).with_context(|| format!("stopping {name}")),
        }
    }
    Ok(())
}

/// Remove containers, network, and volumes (ALL world data). The CLI
/// gates this behind `--yes`.
pub async fn destroy() -> Result<()> {
    let docker = connect()?;
    for name in [LAUNCHER_CONTAINER, MONGO_CONTAINER, REDIS_CONTAINER] {
        let opts = RemoveContainerOptionsBuilder::new().force(true).build();
        match docker.remove_container(name, Some(opts)).await {
            Ok(()) => tracing::info!(container = name, "removed"),
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(e).with_context(|| format!("removing container {name}")),
        }
    }
    match docker.remove_network(NETWORK).await {
        Ok(()) => tracing::info!(network = NETWORK, "removed"),
        Err(e) if is_not_found(&e) => {}
        Err(e) => return Err(e).context("removing network"),
    }
    for name in [DATA_VOLUME, MONGO_VOLUME, REDIS_VOLUME] {
        match docker
            .remove_volume(name, None::<RemoveVolumeOptions>)
            .await
        {
            Ok(()) => tracing::info!(volume = name, "removed"),
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(e).with_context(|| format!("removing volume {name}")),
        }
    }
    Ok(())
}

// ===================================================================
// tests — the offline parts of the image-build path (P0.A9(d)): tar
// construction + context validation. No Docker daemon is touched; the
// actual `docker build` is a live-gauntlet item.
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_context(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("screeps-eval-tar-tests-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a launcher-repo-shaped fixture context and pin the tar:
    /// relative forward-slash entry names, deterministic order, file
    /// contents intact, `.git` skipped.
    #[test]
    fn context_tar_contains_relative_entries_and_skips_git() {
        let dir = temp_context("buildable");
        std::fs::write(dir.join("Dockerfile"), "FROM golang AS builder\n").unwrap();
        std::fs::write(dir.join("go.mod"), "module screeps-launcher\n").unwrap();
        std::fs::create_dir_all(dir.join("launcher")).unwrap();
        std::fs::write(dir.join("launcher").join("config.go"), "package launcher\n").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let bytes = build_context_tar(&dir, "Dockerfile").unwrap();

        let mut archive = tar::Archive::new(&bytes[..]);
        let mut names = Vec::new();
        let mut dockerfile_content = String::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            if name == "Dockerfile" {
                use std::io::Read;
                entry.read_to_string(&mut dockerfile_content).unwrap();
            }
            names.push(name);
        }
        // Deterministic order (sorted per directory), forward slashes,
        // no .git, no absolute paths.
        assert_eq!(
            names,
            vec!["Dockerfile", "go.mod", "launcher", "launcher/config.go"]
        );
        assert_eq!(dockerfile_content, "FROM golang AS builder\n");
    }

    /// The named failure mode: a config-only launcher *deployment*
    /// directory (compose + config.yml — e.g. C:\code\screeps-launcher)
    /// is not a buildable context; the error must say what IS.
    #[test]
    fn config_only_context_is_a_clear_error() {
        let dir = temp_context("config-only");
        std::fs::write(dir.join("config.yml"), "steamKey: nope\n").unwrap();
        std::fs::write(dir.join("docker-compose.yml"), "services: {}\n").unwrap();

        let err = build_context_tar(&dir, "Dockerfile").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no Dockerfile"), "got: {msg}");
        assert!(
            msg.contains("screepers/screeps-launcher"),
            "error must point at the upstream repo: {msg}"
        );
    }

    #[test]
    fn missing_context_dir_is_a_clear_error() {
        let missing = std::env::temp_dir().join("screeps-eval-tar-tests-definitely-missing");
        let err = build_context_tar(&missing, "Dockerfile").unwrap_err();
        assert!(format!("{err:#}").contains("not a directory"));
    }

    /// A custom dockerfile name is honored by the validation.
    #[test]
    fn custom_dockerfile_name_is_validated() {
        let dir = temp_context("custom-dockerfile");
        std::fs::write(dir.join("Dockerfile.custom"), "FROM scratch\n").unwrap();
        assert!(build_context_tar(&dir, "Dockerfile.custom").is_ok());
        assert!(build_context_tar(&dir, "Dockerfile").is_err());
    }
}
