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
//! - `screepers/screeps-launcher` (PULLED from the registry — building
//!   from the local clone is a recorded future investigation, not
//!   Phase-0 scope): volume `screeps-eval-data:/screeps`, the merged
//!   runtime config bind-mounted at `/screeps/config.yml`, env
//!   `MONGO_HOST=mongo` / `REDIS_HOST=redis` (consumed by
//!   screepsmod-mongo; the launcher passes its own environment through
//!   to the server processes — launcher/server.go `os.Environ()`),
//!   game + CLI ports published per `eval.ports`.
//!
//! Everything is named `screeps-eval-*` so the stack is recognizable
//! and `destroy` cannot touch anything else.

use crate::config::EvalSettings;
use crate::server_config;
use anyhow::{bail, Context, Result};
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, EndpointSettings, HealthConfig, HostConfig,
    NetworkCreateRequest, NetworkingConfig, PortBinding, VolumeCreateOptions,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, InspectContainerOptions,
    ListContainersOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    RemoveVolumeOptions, StartContainerOptions, StopContainerOptionsBuilder,
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

/// Image policy (P0.A2, operator-resolved): PULL the published image by
/// default. Building from the local launcher clone is a recorded
/// investigation item, not Phase-0 scope.
pub const LAUNCHER_IMAGE: &str = "screepers/screeps-launcher:latest";
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

fn launcher_body(runtime_config: &Path, game_port: u16, cli_port: u16) -> ContainerCreateBody {
    let game_key = format!("{game_port}/tcp");
    let cli_key = format!("{cli_port}/tcp");
    let binding = |host_port: u16| {
        Some(vec![PortBinding {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(host_port.to_string()),
        }])
    };
    ContainerCreateBody {
        image: Some(LAUNCHER_IMAGE.to_string()),
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
pub async fn up(eval: &EvalSettings) -> Result<()> {
    // Fail fast on config problems (e.g. no steamKey anywhere) before
    // touching Docker at all.
    let runtime_config = server_config::prepare_runtime_config(eval)?;
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

    for image in [MONGO_IMAGE, REDIS_IMAGE, LAUNCHER_IMAGE] {
        ensure_image(&docker, image).await?;
    }
    ensure_network(&docker).await?;
    for volume in [DATA_VOLUME, MONGO_VOLUME, REDIS_VOLUME] {
        ensure_volume(&docker, volume).await?;
    }

    // Databases first (compose: depends_on service_healthy).
    ensure_container_running(&docker, MONGO_CONTAINER, mongo_body()).await?;
    ensure_container_running(&docker, REDIS_CONTAINER, redis_body()).await?;
    wait_container_healthy(&docker, MONGO_CONTAINER, Duration::from_secs(120)).await?;
    wait_container_healthy(&docker, REDIS_CONTAINER, Duration::from_secs(120)).await?;

    warn_if_launcher_ports_stale(&docker, eval).await?;
    let created = ensure_container_running(
        &docker,
        LAUNCHER_CONTAINER,
        launcher_body(&runtime_config, eval.game_port, eval.cli_port),
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
    let elapsed = wait_api_ready(&docker, eval.game_port, budget).await?;
    tracing::info!(
        "server is up: game API answering on http://127.0.0.1:{} after {:.0?}",
        eval.game_port,
        elapsed
    );
    Ok(())
}

/// An existing launcher container keeps the published ports it was
/// created with — if `eval.ports` changed since, `up` would silently
/// publish the old ones. Detect and tell the operator what to do.
async fn warn_if_launcher_ports_stale(docker: &Docker, eval: &EvalSettings) -> Result<()> {
    let Some(inspect) = inspect_opt(docker, LAUNCHER_CONTAINER).await? else {
        return Ok(());
    };
    let bindings = inspect
        .host_config
        .and_then(|hc| hc.port_bindings)
        .unwrap_or_default();
    let want_game = format!("{}/tcp", eval.game_port);
    let want_cli = format!("{}/tcp", eval.cli_port);
    if !bindings.contains_key(&want_game) || !bindings.contains_key(&want_cli) {
        tracing::warn!(
            "existing launcher container publishes {:?}, but eval.ports wants {}/{} — \
             ports are fixed at container creation; run `server destroy --yes` then `server up`",
            bindings.keys().collect::<Vec<_>>(),
            eval.game_port,
            eval.cli_port
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
    let name = find_launcher(&docker)
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
/// container running the launcher image (covers a manually-started
/// stack, e.g. the compose reference in the launcher clone).
async fn find_launcher(docker: &Docker) -> Result<Option<String>> {
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
            .is_some_and(|i| i.contains("screeps-launcher"))
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
pub async fn status(eval: &EvalSettings) -> Result<String> {
    let docker = connect()?;
    let mut out = String::new();
    let launcher = find_launcher(&docker).await?;

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
                    // using the eval config (fall back to launcher
                    // defaults 21025/21026 for a foreign stack).
                    let host_port_num: Option<u16> = host_port.parse().ok();
                    if container_port == &format!("{}/tcp", eval.game_port)
                        || container_port == "21025/tcp"
                    {
                        discovered_game_port = discovered_game_port.or(host_port_num);
                    } else if container_port == &format!("{}/tcp", eval.cli_port)
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
    let game_port = discovered_game_port.unwrap_or(eval.game_port);
    let cli_port = discovered_cli_port.unwrap_or(eval.cli_port);

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
