//! room_audit — READ-ONLY defensive-coverage audit of owned MMO rooms.
//!
//! Enumerates every room the configured account OWNS (controller level
//! >= 1) on each shard, pulls terrain + live structures, runs the
//! screeps-foreman room planner, and flood-fills BOTH the planned and
//! the live rampart/wall perimeter from the room exits to find
//! "unprotected sections" — core structures an enemy creep could reach
//! without crossing a barrier.
//!
//! It is a pure analysis tool: it performs only GET/map-stats reads and
//! never writes to the server. Output is written under `--out`:
//!   - `owned-rooms.json`  — bench-format map (terrain + planner objects),
//!                           re-runnable through screeps-foreman-bench
//!   - `live-structures.json` — raw built structures per room
//!   - `report.json`       — machine-readable findings
//!   - `<room>.txt`        — ASCII plan/live/exposure maps per room
//!
//! Run from the crate directory: `cargo run --bin room_audit -- --server-name mmo`.

use anyhow::{Context, Result};
use clap::Parser;
use screeps_foreman::planner::plan_room_with_timeout;
use screeps_foreman::room_data::{PlanLocation, PlannerRoomDataSource};
use screeps_foreman::terrain::{FastRoomTerrain, TerrainFlags};
use screeps_foreman::StructureType;
use screeps_prospector::cache::terrain_hex_to_vec;
use screeps_prospector::config::ProspectorConfig;
use screeps_rest_api::{console_lines, enumerate_room_names, Client, ConsoleSocket};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "room_audit", about = "Defensive-coverage audit of owned MMO rooms")]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, default_value = "mmo")]
    server_name: String,
    /// Restrict to one shard (default: every shard reported by the server)
    #[arg(long)]
    shard: Option<String>,
    /// Restrict to an explicit room list (skips owned-room discovery);
    /// requires --shard. Comma-separated, e.g. W1N1,W2N1
    #[arg(long)]
    rooms: Option<String>,
    /// Dump live room state (controller, hostiles, containers, creeps) for one
    /// room (requires --shard) and exit — read-only diagnostic.
    #[arg(long)]
    dump_room: Option<String>,
    /// Read-only: tail the bot's console websocket for N seconds and exit.
    #[arg(long)]
    tail_console: Option<u64>,
    /// One-shot LIVE action: set `Memory._features.construction.bucket_threshold`
    /// to this value on --shard (the planning CPU-bucket gate) and exit.
    #[arg(long)]
    set_planning_bucket: Option<i32>,
    /// One-shot LIVE action: set `Memory._features.reset.room_plans = true`
    /// on --shard so the bot drops all stored plans and re-plans with the
    /// current planner, then exit. Requires --shard. This is the only
    /// write this tool performs.
    #[arg(long)]
    reset_room_plans: bool,
    /// Per-room planner wall-clock budget (seconds)
    #[arg(long, default_value_t = 180)]
    plan_timeout_secs: u64,
    /// Output directory
    #[arg(long, default_value = "output/audit")]
    out: PathBuf,
    #[arg(long, default_value_t = screeps_rest_api::DEFAULT_MIN_DELAY_MS)]
    min_delay_ms: u64,
}

fn client_for(cli: &Cli, shard: Option<String>) -> Result<Client> {
    // Re-load config per client so each owns its own auth (AuthMode is
    // not Clone). The file read is cheap and keeps secrets confined.
    let cfg = ProspectorConfig::load(cli.config.as_deref(), &cli.server_name)?;
    let client = Client::new(
        cfg.base_url,
        shard,
        cfg.auth,
        Duration::from_millis(cli.min_delay_ms),
    )?;
    Ok(client)
}

// ----------------------------- structure taxonomy -----------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Category {
    Barrier,   // rampart / wall — blocks enemy movement
    Valuable,  // spawn, tower, storage, terminal, lab, link, extension, ...
    Container, // mining/best-effort, cheap, not core
    Controller,
    Source,
    Mineral,
    Road,
    Other,
}

fn category_for_str(t: &str) -> Category {
    match t {
        "rampart" | "constructedWall" => Category::Barrier,
        "spawn" | "tower" | "storage" | "terminal" | "lab" | "link" | "extension" | "factory"
        | "nuker" | "observer" | "powerSpawn" | "extractor" => Category::Valuable,
        "container" => Category::Container,
        "controller" => Category::Controller,
        "source" => Category::Source,
        "mineral" => Category::Mineral,
        "road" => Category::Road,
        _ => Category::Other,
    }
}

fn category_for_plan(t: StructureType) -> Category {
    match t {
        StructureType::Rampart | StructureType::Wall => Category::Barrier,
        StructureType::Spawn
        | StructureType::Tower
        | StructureType::Storage
        | StructureType::Terminal
        | StructureType::Lab
        | StructureType::Link
        | StructureType::Extension
        | StructureType::Factory
        | StructureType::Nuker
        | StructureType::Observer
        | StructureType::PowerSpawn
        | StructureType::Extractor => Category::Valuable,
        StructureType::Container => Category::Container,
        StructureType::Road => Category::Road,
        _ => Category::Other,
    }
}

// ----------------------------- flood-fill core --------------------------------

const W: usize = 50;
const H: usize = 50;

fn idx(x: u8, y: u8) -> usize {
    y as usize * W + x as usize
}

/// 8-connected flood from every non-wall border tile (the exits an enemy
/// enters through), stopping at natural walls and `barriers`. Returns the
/// boolean reachable grid.
fn reachable_from_exits(terrain: &FastRoomTerrain, barriers: &HashSet<(u8, u8)>) -> Vec<bool> {
    let mut reached = vec![false; W * H];
    let mut q: VecDeque<(u8, u8)> = VecDeque::new();

    let seed = |x: u8, y: u8, reached: &mut Vec<bool>, q: &mut VecDeque<(u8, u8)>| {
        if terrain.is_wall(x, y) || barriers.contains(&(x, y)) {
            return;
        }
        let i = idx(x, y);
        if !reached[i] {
            reached[i] = true;
            q.push_back((x, y));
        }
    };
    for x in 0..W as u8 {
        seed(x, 0, &mut reached, &mut q);
        seed(x, (H - 1) as u8, &mut reached, &mut q);
    }
    for y in 0..H as u8 {
        seed(0, y, &mut reached, &mut q);
        seed((W - 1) as u8, y, &mut reached, &mut q);
    }

    while let Some((x, y)) = q.pop_front() {
        for dy in -1i16..=1 {
            for dx in -1i16..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as i16 + dx;
                let ny = y as i16 + dy;
                if nx < 0 || ny < 0 || nx >= W as i16 || ny >= H as i16 {
                    continue;
                }
                let (nx, ny) = (nx as u8, ny as u8);
                if terrain.is_wall(nx, ny) || barriers.contains(&(nx, ny)) {
                    continue;
                }
                let ni = idx(nx, ny);
                if !reached[ni] {
                    reached[ni] = true;
                    q.push_back((nx, ny));
                }
            }
        }
    }
    reached
}

/// Is a tile reachable, OR (for adjacency-threats like attackController)
/// is any of its 8 neighbors reachable?
fn neighbor_reachable(reached: &[bool], x: u8, y: u8) -> bool {
    for dy in -1i16..=1 {
        for dx in -1i16..=1 {
            let nx = x as i16 + dx;
            let ny = y as i16 + dy;
            if nx < 0 || ny < 0 || nx >= W as i16 || ny >= H as i16 {
                continue;
            }
            if reached[idx(nx as u8, ny as u8)] {
                return true;
            }
        }
    }
    false
}

// ----------------------------- data model -------------------------------------

struct StructTile {
    cat: Category,
    type_str: String,
    x: u8,
    y: u8,
}

struct RoomData {
    shard: String,
    name: String,
    level: u32,
    terrain_hex: String,
    /// planner objects (bench shape): source/controller/mineral
    planner_objects: Vec<Value>,
    /// live structures (incl. ramparts/walls)
    live: Vec<StructTile>,
}

struct PlannerSource {
    terrain: FastRoomTerrain,
    controllers: Vec<PlanLocation>,
    sources: Vec<PlanLocation>,
    minerals: Vec<PlanLocation>,
}
impl PlannerRoomDataSource for PlannerSource {
    fn get_terrain(&self) -> &FastRoomTerrain {
        &self.terrain
    }
    fn get_controllers(&self) -> &[PlanLocation] {
        &self.controllers
    }
    fn get_sources(&self) -> &[PlanLocation] {
        &self.sources
    }
    fn get_minerals(&self) -> &[PlanLocation] {
        &self.minerals
    }
}

#[derive(Serialize)]
struct Exposure {
    type_str: String,
    x: u8,
    y: u8,
}

#[derive(Serialize)]
struct RoomFinding {
    shard: String,
    room: String,
    controller_level: u32,
    planned_ok: bool,
    planner_error: Option<String>,
    plan_barrier_count: usize,
    /// PLAN: valuable core structures reachable from an exit (genuine planner gap)
    plan_exposed_valuable: Vec<Exposure>,
    /// PLAN: mining containers reachable (best-effort / edge limitation)
    plan_exposed_containers: Vec<Exposure>,
    /// PLAN: controller reachable for attackController
    plan_controller_attackable: bool,
    /// LIVE: built barriers present
    live_barrier_count: usize,
    live_exposed_valuable: Vec<Exposure>,
    live_exposed_containers: Vec<Exposure>,
    live_controller_attackable: bool,
    /// Stale-plan test: of the live built barriers, how many sit on a tile
    /// the CURRENT planner also marks as a barrier (subset = same plan,
    /// stalled build) vs off-plan (live built from a different/older plan).
    live_barriers_on_plan: usize,
    live_barriers_off_plan: usize,
    classification: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "room_audit=info,screeps_rest_api=warn".into()),
        )
        .init();
    let cli = Cli::parse();

    // Read-only console tail (token auth via fresh_token; works for MMO).
    if let Some(secs) = cli.tail_console {
        let client = client_for(&cli, cli.shard.clone().or(Some("shardX".to_owned())))?;
        let user = client.me().await.context("me")?;
        let token = client.fresh_token().await.context("fresh_token")?;
        let ws_url = client.ws_url();
        let mut socket = ConsoleSocket::connect(&ws_url, token, &user.id)
            .await
            .context("connect console websocket")?;
        eprintln!("tailing console for {} ({}s) ...", user.username, secs);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                event = socket.next_event() => {
                    let event = event.context("console websocket")?;
                    if !event.channel.ends_with("/console") {
                        continue;
                    }
                    for line in console_lines(&event.payload) {
                        println!("{}", line.line);
                    }
                }
            }
        }
        return Ok(());
    }

    // Read-only live room dump for diagnostics.
    if let Some(room) = &cli.dump_room {
        let shard = cli
            .shard
            .clone()
            .context("--dump-room requires --shard (e.g. --shard shardX)")?;
        let client = client_for(&cli, Some(shard.clone()))?;
        let me = client.me().await.context("me")?;
        println!("my user id: {}", me.id);
        match client.game_time().await {
            Ok(t) => println!("current game time (shardX): {}", t.time),
            Err(e) => eprintln!("game-time failed: {e}"),
        }
        let objs = client.room_objects(room).await.context("room-objects")?;
        for obj in &objs.objects {
            let Some(map) = obj.as_object() else { continue };
            let t = map.get("type").and_then(Value::as_str).unwrap_or("?");
            // Dump the full record for the load-bearing diagnostic types.
            if matches!(t, "controller" | "creep" | "container" | "powerCreep" | "constructionSite") {
                println!("{}", serde_json::to_string(obj).unwrap_or_default());
            }
        }
        return Ok(());
    }

    // One-shot live set of the planning bucket-threshold feature flag.
    if let Some(value) = cli.set_planning_bucket {
        let shard = cli
            .shard
            .clone()
            .context("--set-planning-bucket requires --shard (e.g. --shard shardX)")?;
        let client = client_for(&cli, Some(shard.clone()))?;
        let expr = format!(
            "Memory._features=Memory._features||{{}};\
             Memory._features.construction=Memory._features.construction||{{}};\
             Memory._features.construction.bucket_threshold={value};\
             'planning bucket_threshold = '+Memory._features.construction.bucket_threshold"
        );
        println!("Setting planning bucket_threshold={value} on {shard} via /api/user/console ...");
        let resp = client
            .console(&expr)
            .await
            .context("POST /api/user/console")?;
        println!(
            "console accepted (ok={}). Planning will now run when bucket >= {value}, \
             so the plan-less rooms can re-plan and resume construction.",
            resp.ok
        );
        return Ok(());
    }

    // One-shot live re-plan trigger (the only write this tool makes).
    if cli.reset_room_plans {
        let shard = cli
            .shard
            .clone()
            .context("--reset-room-plans requires --shard (e.g. --shard shardX)")?;
        let client = client_for(&cli, Some(shard.clone()))?;
        // Defensive path creation so it works whether or not _features/reset exist.
        let expr = "Memory._features=Memory._features||{};\
                    Memory._features.reset=Memory._features.reset||{};\
                    Memory._features.reset.room_plans=true;\
                    'room_plans reset armed: '+JSON.stringify(Memory._features.reset)";
        println!("Arming one-shot room-plan reset on {shard} via /api/user/console ...");
        let resp = client
            .console(expr)
            .await
            .context("POST /api/user/console")?;
        println!(
            "console accepted (ok={}). The bot will drop all stored plans next tick, \
             clear the flag, and re-plan every room with the current planner.",
            resp.ok
        );
        return Ok(());
    }

    std::fs::create_dir_all(&cli.out)?;

    // 1. Identity + shards.
    let bootstrap = client_for(&cli, cli.shard.clone().or(Some("shard3".to_owned())))?;
    let me = bootstrap.me().await.context("GET /api/auth/me")?;
    let my_id = me.id.clone();
    println!("account: {} (id {})", me.username, my_id);

    let shards: Vec<String> = match &cli.shard {
        Some(s) => vec![s.clone()],
        None => match bootstrap.shards_info().await {
            Ok(info) => info.shards.into_iter().map(|s| s.name).collect(),
            Err(e) => {
                eprintln!("shards/info failed ({e}); defaulting to shard0..shard3");
                (0..4).map(|i| format!("shard{i}")).collect()
            }
        },
    };
    println!("shards: {shards:?}");

    // 2. Discover owned rooms per shard (or use the explicit --rooms list).
    let mut rooms: Vec<RoomData> = Vec::new();
    for shard in &shards {
        let client = client_for(&cli, Some(shard.clone()))?;
        let owned: Vec<(String, u32)> = if let Some(list) = &cli.rooms {
            list.split(',')
                .map(|s| (s.trim().to_owned(), 1u32))
                .filter(|(s, _)| !s.is_empty())
                .collect()
        } else {
            discover_owned(&client, &my_id).await?
        };
        if owned.is_empty() {
            println!("  {shard}: no owned rooms");
            continue;
        }
        println!(
            "  {shard}: {} owned room(s): {:?}",
            owned.len(),
            owned.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
        );
        for (room, level) in owned {
            match fetch_room(&client, shard, &room, level).await {
                Ok(rd) => rooms.push(rd),
                Err(e) => eprintln!("  fetch {shard}/{room} failed: {e}"),
            }
        }
    }

    if rooms.is_empty() {
        println!("\nNo owned rooms found. Nothing to audit.");
        return Ok(());
    }

    // 3. Persist the raw datasets (bench-format + live structures).
    write_bench_json(&cli.out, &rooms)?;
    write_live_json(&cli.out, &rooms)?;

    // 4. Plan + analyze each room.
    let mut findings: Vec<RoomFinding> = Vec::new();
    for rd in &rooms {
        println!("\n=== {}/{} (RCL {}) ===", rd.shard, rd.name, rd.level);
        let finding = analyze_room(rd, Duration::from_secs(cli.plan_timeout_secs), &cli.out)?;
        println!(
            "  plan: {} barriers, {} exposed valuable, {} exposed containers, ctrl-attackable={}",
            finding.plan_barrier_count,
            finding.plan_exposed_valuable.len(),
            finding.plan_exposed_containers.len(),
            finding.plan_controller_attackable
        );
        println!(
            "  live: {} barriers, {} exposed valuable, {} exposed containers, ctrl-attackable={}",
            finding.live_barrier_count,
            finding.live_exposed_valuable.len(),
            finding.live_exposed_containers.len(),
            finding.live_controller_attackable
        );
        println!(
            "  stale-plan test: {} of {} live barriers match the current plan, {} are OFF-plan (built from a different/older plan)",
            finding.live_barriers_on_plan,
            finding.live_barrier_count,
            finding.live_barriers_off_plan
        );
        println!("  => {}", finding.classification);
        findings.push(finding);
    }

    // 5. Report.
    let report_path = cli.out.join("report.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&findings)?)?;
    println!("\nReport: {}", report_path.display());

    // Summary of likely planner defects.
    let defects: Vec<&RoomFinding> = findings
        .iter()
        .filter(|f| !f.plan_exposed_valuable.is_empty() || f.plan_controller_attackable)
        .collect();
    println!("\n==== SUMMARY ====");
    println!("rooms audited: {}", findings.len());
    println!("rooms whose PLAN leaves a core structure exposed (candidate planner defects): {}", defects.len());
    for f in &defects {
        println!(
            "  {}/{}: {} exposed valuable, ctrl-attackable={}",
            f.shard,
            f.room,
            f.plan_exposed_valuable.len(),
            f.plan_controller_attackable
        );
    }
    Ok(())
}

/// map-stats owner0 over the whole shard; keep rooms whose owner is us.
async fn discover_owned(client: &Client, my_id: &str) -> Result<Vec<(String, u32)>> {
    let world = client.world_size().await.context("world-size")?;
    let names = enumerate_room_names(world.width, world.height);
    let mut owned = Vec::new();
    for chunk in names.chunks(5000) {
        let resp = client
            .map_stats(chunk, "owner0")
            .await
            .context("map-stats owner0")?;
        for (room, stat) in resp.stats {
            if let Some(owner) = &stat.own {
                if owner.user == my_id && owner.level >= 1 {
                    owned.push((room, owner.level));
                }
            }
        }
    }
    owned.sort();
    Ok(owned)
}

async fn fetch_room(client: &Client, shard: &str, room: &str, level: u32) -> Result<RoomData> {
    let terrain = client.room_terrain_encoded(room).await.context("terrain")?;
    let terrain_hex = terrain
        .terrain
        .get(0)
        .map(|e| e.terrain.clone())
        .context("empty terrain response")?;
    let objects = client.room_objects(room).await.context("objects")?;

    let mut planner_objects = Vec::new();
    let mut live = Vec::new();
    for obj in &objects.objects {
        let Some(map) = obj.as_object() else { continue };
        let Some(t) = map.get("type").and_then(Value::as_str) else {
            continue;
        };
        let (Some(x), Some(y)) = (
            map.get("x").and_then(Value::as_i64),
            map.get("y").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let (x, y) = (x as u8, y as u8);
        let cat = category_for_str(t);
        if matches!(cat, Category::Source | Category::Controller | Category::Mineral) {
            let mut o = serde_json::Map::new();
            o.insert("type".into(), Value::from(t));
            o.insert("x".into(), Value::from(x));
            o.insert("y".into(), Value::from(y));
            if let Some(mt) = map.get("mineralType") {
                o.insert("mineralType".into(), mt.clone());
            }
            planner_objects.push(Value::Object(o));
        }
        live.push(StructTile {
            cat,
            type_str: t.to_owned(),
            x,
            y,
        });
    }

    Ok(RoomData {
        shard: shard.to_owned(),
        name: room.to_owned(),
        level,
        terrain_hex,
        planner_objects,
        live,
    })
}

fn write_bench_json(out: &PathBuf, rooms: &[RoomData]) -> Result<()> {
    #[derive(Serialize)]
    struct BenchRoom<'a> {
        room: &'a str,
        shard: &'a str,
        terrain: &'a str,
        objects: &'a [Value],
    }
    #[derive(Serialize)]
    struct Bench<'a> {
        description: String,
        rooms: Vec<BenchRoom<'a>>,
    }
    let bench = Bench {
        description: "room_audit owned-room dump (bench format)".into(),
        rooms: rooms
            .iter()
            .map(|r| BenchRoom {
                room: &r.name,
                shard: &r.shard,
                terrain: &r.terrain_hex,
                objects: &r.planner_objects,
            })
            .collect(),
    };
    std::fs::write(out.join("owned-rooms.json"), serde_json::to_string_pretty(&bench)?)?;
    Ok(())
}

fn write_live_json(out: &PathBuf, rooms: &[RoomData]) -> Result<()> {
    let mut map = BTreeMap::new();
    for r in rooms {
        let objs: Vec<Value> = r
            .live
            .iter()
            .map(|s| {
                serde_json::json!({"type": s.type_str, "x": s.x, "y": s.y})
            })
            .collect();
        map.insert(format!("{}/{}", r.shard, r.name), objs);
    }
    std::fs::write(out.join("live-structures.json"), serde_json::to_string_pretty(&map)?)?;
    Ok(())
}

fn analyze_room(rd: &RoomData, timeout: Duration, out: &PathBuf) -> Result<RoomFinding> {
    let terrain = FastRoomTerrain::new(terrain_hex_to_vec(&rd.terrain_hex)?);

    let get_locs = |t: &str| -> Vec<PlanLocation> {
        rd.planner_objects
            .iter()
            .filter_map(|o| o.as_object())
            .filter(|o| o.get("type").and_then(Value::as_str) == Some(t))
            .filter_map(|o| {
                Some(PlanLocation::new(
                    o.get("x")?.as_i64()? as i8,
                    o.get("y")?.as_i64()? as i8,
                ))
            })
            .collect()
    };

    let source = PlannerSource {
        terrain: FastRoomTerrain::new(terrain_hex_to_vec(&rd.terrain_hex)?),
        controllers: get_locs("controller"),
        sources: get_locs("source"),
        minerals: get_locs("mineral"),
    };

    // --- Run the planner. ---
    let deadline = Instant::now() + timeout;
    let plan_res = plan_room_with_timeout(&source, move || Instant::now() < deadline);

    let mut finding = RoomFinding {
        shard: rd.shard.clone(),
        room: rd.name.clone(),
        controller_level: rd.level,
        planned_ok: false,
        planner_error: None,
        plan_barrier_count: 0,
        plan_exposed_valuable: vec![],
        plan_exposed_containers: vec![],
        plan_controller_attackable: false,
        live_barrier_count: 0,
        live_exposed_valuable: vec![],
        live_exposed_containers: vec![],
        live_controller_attackable: false,
        live_barriers_on_plan: 0,
        live_barriers_off_plan: 0,
        classification: String::new(),
    };

    // Planned barrier tile set (filled when the plan succeeds) for the
    // stale-plan subset test below.
    let mut plan_barrier_set: HashSet<(u8, u8)> = HashSet::new();

    // Controller position for attackController checks.
    let controller_xy: Option<(u8, u8)> = rd
        .planner_objects
        .iter()
        .filter_map(|o| o.as_object())
        .find(|o| o.get("type").and_then(Value::as_str) == Some("controller"))
        .and_then(|o| {
            Some((
                o.get("x")?.as_i64()? as u8,
                o.get("y")?.as_i64()? as u8,
            ))
        });

    // ---------- PLAN analysis ----------
    let mut plan_ascii = String::new();
    match plan_res {
        Ok(Some(plan)) => {
            finding.planned_ok = true;
            let mut barriers: HashSet<(u8, u8)> = HashSet::new();
            for st in [StructureType::Rampart, StructureType::Wall] {
                for loc in plan.get_locations(st) {
                    barriers.insert((loc.x(), loc.y()));
                }
            }
            finding.plan_barrier_count = barriers.len();
            plan_barrier_set = barriers.clone();

            // Gather planned valuable + container tiles.
            let mut valuable: Vec<(u8, u8, String)> = Vec::new();
            let mut containers: Vec<(u8, u8)> = Vec::new();
            // Enumerate via visualize-equivalent: iterate all structure types.
            for st in ALL_PLAN_TYPES {
                let cat = category_for_plan(st);
                match cat {
                    Category::Valuable => {
                        for loc in plan.get_locations(st) {
                            valuable.push((loc.x(), loc.y(), format!("{st:?}")));
                        }
                    }
                    Category::Container => {
                        for loc in plan.get_locations(st) {
                            containers.push((loc.x(), loc.y()));
                        }
                    }
                    _ => {}
                }
            }

            let reached = reachable_from_exits(&terrain, &barriers);
            for (x, y, ts) in &valuable {
                if reached[idx(*x, *y)] {
                    finding.plan_exposed_valuable.push(Exposure {
                        type_str: ts.clone(),
                        x: *x,
                        y: *y,
                    });
                }
            }
            for (x, y) in &containers {
                if reached[idx(*x, *y)] {
                    finding
                        .plan_exposed_containers
                        .push(Exposure { type_str: "container".into(), x: *x, y: *y });
                }
            }
            if let Some((cx, cy)) = controller_xy {
                // attackController is possible if a hostile can stand adjacent
                // to the controller (the controller tile itself is unwalkable).
                finding.plan_controller_attackable = neighbor_reachable(&reached, cx, cy);
            }

            plan_ascii = render_ascii(
                &terrain,
                &barriers,
                &finding.plan_exposed_valuable,
                &finding.plan_exposed_containers,
                controller_xy,
                &reached,
            );
        }
        Ok(None) => {
            finding.planner_error = Some("planner timed out".into());
        }
        Err(e) => {
            finding.planner_error = Some(e);
        }
    }

    // ---------- LIVE analysis ----------
    let mut live_barriers: HashSet<(u8, u8)> = HashSet::new();
    for s in &rd.live {
        if s.cat == Category::Barrier {
            live_barriers.insert((s.x, s.y));
        }
    }
    finding.live_barrier_count = live_barriers.len();
    if finding.planned_ok {
        for b in &live_barriers {
            if plan_barrier_set.contains(b) {
                finding.live_barriers_on_plan += 1;
            } else {
                finding.live_barriers_off_plan += 1;
            }
        }
    }
    let live_reached = reachable_from_exits(&terrain, &live_barriers);
    for s in &rd.live {
        match s.cat {
            Category::Valuable => {
                if live_reached[idx(s.x, s.y)] {
                    finding.live_exposed_valuable.push(Exposure {
                        type_str: s.type_str.clone(),
                        x: s.x,
                        y: s.y,
                    });
                }
            }
            Category::Container => {
                if live_reached[idx(s.x, s.y)] {
                    finding.live_exposed_containers.push(Exposure {
                        type_str: "container".into(),
                        x: s.x,
                        y: s.y,
                    });
                }
            }
            _ => {}
        }
    }
    if let Some((cx, cy)) = controller_xy {
        finding.live_controller_attackable = neighbor_reachable(&live_reached, cx, cy);
    }
    let live_ascii = render_ascii(
        &terrain,
        &live_barriers,
        &finding.live_exposed_valuable,
        &finding.live_exposed_containers,
        controller_xy,
        &live_reached,
    );

    // ---------- classification ----------
    finding.classification = if !finding.planned_ok {
        format!(
            "PLANNER FAILED ({})",
            finding.planner_error.as_deref().unwrap_or("unknown")
        )
    } else if !finding.plan_exposed_valuable.is_empty() || finding.plan_controller_attackable {
        "PLAN DEFECT: core structure(s)/controller reachable in the planned layout".into()
    } else if !finding.plan_exposed_containers.is_empty() {
        "EXPECTED LIMITATION: only best-effort mining containers exposed in the plan".into()
    } else if !finding.live_exposed_valuable.is_empty() || finding.live_controller_attackable {
        "LIVE INCOMPLETE: plan seals the core but live ramparts not fully built yet".into()
    } else {
        "OK: plan and live perimeter both seal the core".into()
    };

    // Write per-room ASCII.
    let txt = format!(
        "Room {}/{} (RCL {})\nclassification: {}\n\n== PLAN ==\n{}\n== LIVE ==\n{}\n\nLegend: # natural wall  r rampart  w wall  X exposed-valuable  o exposed-container  * protected-valuable  C controller  s source  m mineral  , swamp  . plain  (lowercase reachable-from-exit shading: ':' reachable plain)\n",
        rd.shard, rd.name, rd.level, finding.classification, plan_ascii, live_ascii
    );
    std::fs::write(out.join(format!("{}_{}.txt", rd.shard, rd.name)), txt)?;

    Ok(finding)
}

const ALL_PLAN_TYPES: [StructureType; 15] = [
    StructureType::Spawn,
    StructureType::Extension,
    StructureType::Tower,
    StructureType::Storage,
    StructureType::Terminal,
    StructureType::Link,
    StructureType::Lab,
    StructureType::Factory,
    StructureType::Nuker,
    StructureType::Observer,
    StructureType::PowerSpawn,
    StructureType::Extractor,
    StructureType::Container,
    StructureType::Rampart,
    StructureType::Wall,
];

#[allow(clippy::too_many_arguments)]
fn render_ascii(
    terrain: &FastRoomTerrain,
    barriers: &HashSet<(u8, u8)>,
    exposed_valuable: &[Exposure],
    exposed_containers: &[Exposure],
    controller: Option<(u8, u8)>,
    reached: &[bool],
) -> String {
    let exv: HashSet<(u8, u8)> = exposed_valuable.iter().map(|e| (e.x, e.y)).collect();
    let exc: HashSet<(u8, u8)> = exposed_containers.iter().map(|e| (e.x, e.y)).collect();
    let mut s = String::with_capacity((W + 1) * H);
    for y in 0..H as u8 {
        for x in 0..W as u8 {
            let c = if exv.contains(&(x, y)) {
                'X'
            } else if exc.contains(&(x, y)) {
                'o'
            } else if Some((x, y)) == controller {
                'C'
            } else if barriers.contains(&(x, y)) {
                'r'
            } else if terrain.is_wall(x, y) {
                '#'
            } else if reached[idx(x, y)] {
                ':' // reachable open tile (enemy can stand here)
            } else if terrain.get_xy(x, y).contains(TerrainFlags::SWAMP) {
                ','
            } else {
                '.'
            };
            s.push(c);
        }
        s.push('\n');
    }
    s
}
