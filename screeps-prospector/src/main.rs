//! screeps-prospector CLI — operator-facing entry point.
//!
//! Thin wrappers over the library (`ops`/`cache`/`score`/`place` + the
//! shared `screeps_rest_api` client):
//! `scan`/`fetch` talk to the server, `score`/`recommend` run entirely
//! offline against the cache, `place`/`auto` write to the server behind
//! the confirmation gates (recommend-first; explicit `--yes`; `auto` is
//! refused outright against official servers and `place` there
//! additionally needs `--i-understand-this-is-mmo`).

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use screeps_prospector::cache::{default_cache_path, seed_from, RoomCache};
use screeps_prospector::config::{ProspectorConfig, DEFAULT_SERVER_NAME};
use screeps_prospector::ops;
use screeps_prospector::place::{gate_auto, gate_place, PlacementRequest, DEFAULT_SPAWN_NAME};
use screeps_prospector::score::{
    self, HeuristicWeights, PlanProfile, RecommendOptions, RecommendResult, Stage1Result,
    DEFAULT_FINALISTS, DEFAULT_PLAN_TIMEOUT_SECS,
};
use screeps_rest_api::{enumerate_room_names, Client, DEFAULT_MIN_DELAY_MS};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "screeps-prospector",
    about = "Spawn-site selection: scan/fetch/score rooms, recommend and place spawns",
    version
)]
struct Cli {
    /// Path to the credentials file (default: ../.screeps.yaml — the
    /// crate is invoked from its own directory; fixed-path rule)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Server entry name in .screeps.yaml
    #[arg(long, global = true, default_value = DEFAULT_SERVER_NAME)]
    server_name: String,

    /// Shard name (required for screeps.com, omit for private servers)
    #[arg(long, global = true)]
    shard: Option<String>,

    /// Cache file (default: cache/<shard-or-server>.json)
    #[arg(long, global = true)]
    cache_file: Option<PathBuf>,

    /// Courtesy minimum delay between API calls, in milliseconds.
    /// The default is sized for screeps.com's 120 requests/minute
    /// global token limit; lower it freely against a private server.
    #[arg(long, global = true, default_value_t = DEFAULT_MIN_DELAY_MS)]
    min_delay_ms: u64,

    #[command(subcommand)]
    command: Command,
}

/// Stage-1 heuristic weights (shared by `score`, `recommend`, `auto`).
/// Defaults come from the library's `HeuristicWeights::default()`.
#[derive(Args)]
struct WeightArgs {
    /// Weight: source count (2 strongly preferred)
    #[arg(long, default_value_t = HeuristicWeights::default().sources)]
    w_sources: f32,
    /// Weight: controller presence
    #[arg(long, default_value_t = HeuristicWeights::default().controller)]
    w_controller: f32,
    /// Weight: mineral presence/type
    #[arg(long, default_value_t = HeuristicWeights::default().mineral)]
    w_mineral: f32,
    /// Weight: 1 - swamp fraction
    #[arg(long, default_value_t = HeuristicWeights::default().swamp)]
    w_swamp: f32,
    /// Weight: 1 - wall fraction
    #[arg(long, default_value_t = HeuristicWeights::default().walls)]
    w_walls: f32,
    /// Weight: exit count/distribution (defensibility)
    #[arg(long, default_value_t = HeuristicWeights::default().exits)]
    w_exits: f32,
}

impl WeightArgs {
    fn to_weights(&self) -> HeuristicWeights {
        HeuristicWeights {
            sources: self.w_sources,
            controller: self.w_controller,
            mineral: self.w_mineral,
            swamp: self.w_swamp,
            walls: self.w_walls,
            exits: self.w_exits,
        }
    }
}

/// Stage-2 planning knobs (shared by `recommend` and `auto`).
#[derive(Args)]
struct PlanArgs {
    /// Number of stage-1 finalists carried into foreman planning
    #[arg(long = "top", default_value_t = DEFAULT_FINALISTS)]
    top: usize,
    /// Wall-clock budget per finalist plan, in seconds
    #[arg(long, default_value_t = DEFAULT_PLAN_TIMEOUT_SECS)]
    plan_timeout_secs: u64,
    /// Foreman layer stack: `full` (the real 23-layer stack) or
    /// `reduced` (hub-only; fast, for iteration — not for real placement)
    #[arg(long, default_value = "full")]
    plan_profile: PlanProfile,
}

impl PlanArgs {
    fn to_options(&self, weights: HeuristicWeights) -> RecommendOptions {
        RecommendOptions {
            weights,
            finalists: self.top,
            profile: self.plan_profile,
            plan_timeout: Duration::from_secs(self.plan_timeout_secs),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Discover rooms open for spawning (batched map-stats) and record
    /// their status in the cache. Resumable: progress is saved after
    /// every batch, and re-runs skip rooms with a fresh cached status
    Scan {
        /// Comma-separated room names (e.g. W5N5,W6N5)
        #[arg(long)]
        rooms: Option<String>,
        /// Enumerate the whole map via world-size instead (an MMO shard
        /// is ~15k rooms => ~15 batched map-stats calls)
        #[arg(long)]
        all: bool,
        /// Skip rooms whose cached status is fresher than this many
        /// seconds (0 = rescan everything)
        #[arg(long, default_value_t = 3600)]
        status_ttl_secs: u64,
    },
    /// Fetch terrain + planner objects into the cache (terrain is
    /// immutable: cached rooms are never refetched)
    Fetch {
        /// Comma-separated room names
        #[arg(long, conflicts_with = "all_open")]
        rooms: Option<String>,
        /// Every room the cache currently flags open (run `scan` first).
        /// This is also the default when --rooms is omitted.
        #[arg(long)]
        all_open: bool,
        /// Re-fetch room status when older than this many seconds
        #[arg(long, default_value_t = 3600)]
        status_ttl_secs: u64,
    },
    /// Rank cached rooms by the stage-1 heuristics (offline — no API
    /// calls; rooms missing data are fetch-listed)
    Score {
        /// Comma-separated room names (default: the cache's open rooms,
        /// or every cached room when no scan statuses exist)
        #[arg(long, conflicts_with = "all")]
        rooms: Option<String>,
        /// Score every cached room
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        weights: WeightArgs,
    },
    /// Full offline pipeline: stage-1 heuristics -> foreman planning for
    /// the finalists -> ranked recommendations with plan-derived spawn
    /// tiles (no API calls)
    Recommend {
        /// Comma-separated room names (default: the cache's open rooms,
        /// or every cached room when no scan statuses exist)
        #[arg(long, conflicts_with = "all")]
        rooms: Option<String>,
        /// Consider every cached room
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        weights: WeightArgs,
        #[command(flatten)]
        plan: PlanArgs,
    },
    /// Place a spawn via the REST API. Prints exactly what it will do,
    /// refuses without --yes; official servers (token auth/screeps.com)
    /// additionally require --i-understand-this-is-mmo
    Place {
        #[arg(long)]
        room: String,
        #[arg(long)]
        x: u32,
        #[arg(long)]
        y: u32,
        /// Spawn name
        #[arg(long, default_value = DEFAULT_SPAWN_NAME)]
        name: String,
        /// Confirm the placement
        #[arg(long)]
        yes: bool,
        /// Required acknowledgement when the server entry is official
        #[arg(long = "i-understand-this-is-mmo")]
        i_understand_this_is_mmo: bool,
    },
    /// Private-server end-to-end: scan -> fetch -> recommend -> place
    /// the best room's plan-derived spawn tile. REFUSED outright against
    /// official servers (recommend-only there)
    Auto {
        /// Confirm the placement
        #[arg(long)]
        yes: bool,
        /// Spawn name
        #[arg(long, default_value = DEFAULT_SPAWN_NAME)]
        name: String,
        /// Re-fetch room status when older than this many seconds
        #[arg(long, default_value_t = 3600)]
        status_ttl_secs: u64,
        #[command(flatten)]
        weights: WeightArgs,
        #[command(flatten)]
        plan: PlanArgs,
    },
    /// Inspect or seed the room cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand)]
enum CacheAction {
    /// Show the cache path and room/terrain/status counts
    #[command(alias = "info")]
    Stats,
    /// OPTIONAL import: copy an existing map JSON into the cache (the
    /// source file is never modified). Not a setup step — the happy
    /// path is `scan` + `fetch` against the live server (P0.P8); seed
    /// exists for niche cases (sharing scans between machines,
    /// importing offline/bench exports).
    Seed {
        /// Source map JSON (explicit — no default; repo-relative
        /// defaults break for external users and at crate extraction)
        #[arg(long)]
        from: PathBuf,
        /// Overwrite an existing cache file
        #[arg(long)]
        force: bool,
    },
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_room_list(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn resolve_cache_path(cli: &Cli) -> PathBuf {
    cli.cache_file
        .clone()
        .unwrap_or_else(|| default_cache_path(&cli.server_name, cli.shard.as_deref()))
}

fn load_or_new_cache(path: &Path, cli: &Cli) -> Result<RoomCache> {
    if path.exists() {
        RoomCache::load(path)
    } else {
        Ok(RoomCache {
            description: format!(
                "screeps-prospector:{}{}",
                cli.server_name,
                cli.shard
                    .as_deref()
                    .map(|s| format!(":{s}"))
                    .unwrap_or_default()
            ),
            rooms: Vec::new(),
        })
    }
}

fn load_cache_or_explain(path: &Path) -> Result<RoomCache> {
    if !path.exists() {
        bail!(
            "no cache at {} (run `scan`+`fetch`, or `cache seed` for offline use; \
             if you scanned with --shard, pass the same one — cache files are \
             per-shard)",
            path.display()
        );
    }
    RoomCache::load(path)
}

/// Load the credentials file (read here, at RUNTIME — never in tests;
/// Workstream-P offline constraint).
fn load_config(cli: &Cli) -> Result<ProspectorConfig> {
    ProspectorConfig::load(cli.config.as_deref(), &cli.server_name)
}

/// Build the REST client from an already-loaded config and sign in
/// (a no-op for token auth). screeps.com targets additionally get
/// their `--shard` checked against `/api/game/shards/info` BEFORE any
/// quota-bearing call: a missing shard fails map-stats with a bare
/// "invalid shard" (verified live 2026-06-10) after the call is
/// already spent, and the validation error can list the actual
/// choices — which change over time (shardX joined shard0..shard3).
///
/// The gate keys on the HOST, not on `cfg.is_official()`: official
/// classification covers any token-auth entry (quota caution), but a
/// token-auth PRIVATE server is legitimately shardless and has no
/// shards/info route — it must keep connecting exactly as before.
async fn connect_with(cfg: ProspectorConfig, cli: &Cli) -> Result<Client> {
    let base_url = cfg.base_url.clone();
    let screeps_com = base_url.contains("screeps.com");
    let client = Client::new(
        cfg.base_url,
        cli.shard.clone(),
        cfg.auth,
        Duration::from_millis(cli.min_delay_ms),
    )?;
    client
        .sign_in()
        .await
        .with_context(|| format!("signing in to {base_url}"))?;
    if screeps_com {
        match client.shards_info().await {
            Ok(info) => {
                let names: Vec<String> = info.shards.into_iter().map(|s| s.name).collect();
                ops::validate_shard_choice(cli.shard.as_deref(), &names)?;
            }
            // screeps.com flavors without the endpoint (e.g. season
            // variants): can't list, but still refuse to run shardless
            // — that fails later with worse errors.
            Err(err) => match cli.shard.as_deref() {
                Some(shard) => {
                    tracing::warn!("could not validate --shard {shard}: {err}");
                }
                None => bail!(
                    "screeps.com is sharded and the shard list could not be \
                     fetched ({err}); pass --shard explicitly (e.g. shard3)"
                ),
            },
        }
    }
    Ok(client)
}

// ---- table printing ----

fn print_stage1_table(stage1: &Stage1Result) {
    if stage1.ranked.is_empty() {
        println!("no scoreable rooms (see the fetch list below)");
    } else {
        println!(
            "{:<10} {:>6} {:>4} {:>5} {:>4} {:>7} {:>6} {:>6} {:>6}  note",
            "room", "total", "src", "ctrl", "min", "swamp%", "wall%", "exits", "sides"
        );
        for score in &stage1.ranked {
            println!(
                "{:<10} {:>6.3} {:>4} {:>5} {:>4} {:>7.1} {:>6.1} {:>6} {:>6}  {}",
                score.room,
                score.total,
                score.source_count,
                if score.has_controller { "yes" } else { "no" },
                score.mineral_type.as_deref().unwrap_or("-"),
                score.metrics.swamp_fraction * 100.0,
                score.metrics.wall_fraction * 100.0,
                score.metrics.exit_tiles,
                score.metrics.exit_sides,
                score.disqualified.as_deref().unwrap_or("")
            );
        }
    }
    print_fetch_list(stage1);
}

fn print_fetch_list(stage1: &Stage1Result) {
    if stage1.needs_fetch.is_empty() {
        return;
    }
    println!();
    println!("missing data ({} rooms):", stage1.needs_fetch.len());
    for needs in &stage1.needs_fetch {
        println!("  {:<10} {}", needs.room, needs.reason);
    }
    let rooms: Vec<&str> = stage1.needs_fetch.iter().map(|n| n.room.as_str()).collect();
    println!("  -> fetch --rooms {}", rooms.join(","));
}

/// Human ETA: seconds -> "Ns" / "Nm Ss" / "Nh Mm".
fn fmt_eta(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// "60/hour" / "240/day" — the quota the ETA reasoning cites.
fn fmt_quota(quota: &screeps_rest_api::EndpointQuota) -> String {
    let period = match quota.period.as_secs() {
        3600 => "hour".to_owned(),
        86_400 => "day".to_owned(),
        secs => format!("{secs}s"),
    };
    format!("{}/{period}", quota.limit)
}

/// MMO quota reasoning, printed BEFORE any scan network call (official
/// servers only): call count, the per-endpoint quota driving the ETA,
/// and the region-scoping suggestion for long runs.
fn print_scan_plan(plan: &ops::ScanPlan, official: bool) {
    if !official {
        return;
    }
    println!(
        "MMO quota plan: {} room(s) to scan ({} fresh in cache, skipped) -> {} map-stats call(s) \
         of up to {} rooms",
        plan.rooms_to_scan,
        plan.skipped_fresh,
        plan.calls,
        ops::MAP_STATS_CHUNK
    );
    println!(
        "  map-stats quota on screeps.com: {} => ETA <= {} (sustained pacing; faster while the \
         hourly window is unspent)",
        fmt_quota(&plan.quota),
        fmt_eta(plan.eta_secs)
    );
    if plan.eta_secs > 600 {
        println!(
            "  tip: scope --rooms to the region you want to settle; progress persists after \
             every batch, so an interrupted run resumes where it stopped"
        );
    }
}

/// MMO quota reasoning for fetch: room-terrain (360/hour) dominates.
fn print_fetch_plan(plan: &ops::FetchPlan, official: bool) {
    if !official {
        return;
    }
    println!(
        "MMO quota plan: {} room-terrain call(s) ({}) + {} room-objects call(s) (global \
         120/minute) + {} status batch(es) ({}); {} of {} room(s) already complete, skipped",
        plan.terrain_calls,
        fmt_quota(&plan.terrain_quota),
        plan.object_calls,
        plan.status_calls,
        fmt_quota(&plan.status_quota),
        plan.skipped_complete,
        plan.rooms_total
    );
    println!(
        "  ETA <= {} (sustained pacing; faster while the hourly windows are unspent)",
        fmt_eta(plan.eta_secs)
    );
    if plan.eta_secs > 600 {
        println!(
            "  tip: fetch a region first (--rooms ...); progress persists incrementally, so an \
             interrupted run resumes where it stopped"
        );
    }
}

fn print_recommendations(result: &RecommendResult) {
    if result.recommendations.is_empty() {
        println!("no recommendations (every candidate was rejected or needs a fetch)");
    } else {
        println!(
            "{:<4} {:<10} {:>9} {:>10} {:>8} {:>7}",
            "rank", "room", "heuristic", "plan", "spawn", "secs"
        );
        for (index, rec) in result.recommendations.iter().enumerate() {
            println!(
                "{:<4} {:<10} {:>9.3} {:>10.4} {:>8} {:>7.1}",
                index + 1,
                rec.room,
                rec.heuristic.total,
                rec.plan_score.total,
                format!("({},{})", rec.spawn.0, rec.spawn.1),
                rec.plan_seconds,
            );
        }
    }
    if !result.rejected.is_empty() {
        println!();
        println!("rejected ({} rooms):", result.rejected.len());
        for rejected in &result.rejected {
            println!("  {:<10} {}", rejected.room, rejected.reason);
        }
    }
    print_fetch_list(&result.stage1);
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // screeps_rest_api included by default: its warns carry
                // the rate-limit backoff/resume notices — invisible
                // backoff looks like a hang.
                .unwrap_or_else(|_| "screeps_prospector=info,screeps_rest_api=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Command::Scan {
            rooms,
            all,
            status_ttl_secs,
        } => {
            let cache_path = resolve_cache_path(&cli);
            let mut cache = load_or_new_cache(&cache_path, &cli)?;
            let cfg = load_config(&cli)?;
            let official = cfg.is_official();
            let client = connect_with(cfg, &cli).await?;
            let room_names = match (rooms, all) {
                (Some(list), _) => parse_room_list(list),
                (None, true) => {
                    let world = client.world_size().await?;
                    enumerate_room_names(world.width, world.height)
                }
                (None, false) => bail!(
                    "pass --rooms W1N1,W2N1,... or --all (enumerates the whole map \
                     via world-size; ~15k rooms on an MMO shard => ~15 batched calls)"
                ),
            };
            // ETA up front, before the first map-stats call.
            let plan = ops::plan_scan(&cache, &room_names, now_unix(), *status_ttl_secs);
            print_scan_plan(&plan, official);
            let summary = ops::scan_rooms_resumable(
                &client,
                &mut cache,
                &room_names,
                now_unix(),
                Some(&cache_path),
                *status_ttl_secs,
            )
            .await?;
            cache.save(&cache_path)?;
            println!(
                "scanned {} rooms ({} skipped, status fresh): {} open for spawning -> {}",
                summary.scanned,
                summary.skipped_fresh,
                summary.open,
                cache_path.display()
            );
            if summary.open == 0 && summary.scanned >= 10 {
                println!(
                    "note: nothing came back open — every scanned room is owned, \
                     reserved, out-of-borders/nonexistent, or unclaimable \
                     (highway/source-keeper). If that's unexpected, check --shard."
                );
            }
        }
        Command::Fetch {
            rooms,
            all_open: _,
            status_ttl_secs,
        } => {
            let cache_path = resolve_cache_path(&cli);
            let mut cache = load_or_new_cache(&cache_path, &cli)?;
            let room_names = match rooms {
                Some(list) => parse_room_list(list),
                // --all-open and the bare default are the same thing:
                // every room the cache currently flags open.
                None => cache.open_rooms().map(|r| r.room.clone()).collect(),
            };
            if room_names.is_empty() {
                bail!(
                    "no rooms to fetch: pass --rooms, or run `scan` first so the \
                     cache knows which rooms are open (if you scanned with \
                     --shard, pass the same one — cache files are per-shard)"
                );
            }
            let cfg = load_config(&cli)?;
            let official = cfg.is_official();
            let client = connect_with(cfg, &cli).await?;
            // ETA up front, before the first call.
            let plan = ops::plan_fetch(
                &cache,
                &room_names,
                *status_ttl_secs,
                now_unix(),
                cli.min_delay_ms,
            );
            print_fetch_plan(&plan, official);
            let summary = ops::fetch_rooms_resumable(
                &client,
                &mut cache,
                &room_names,
                *status_ttl_secs,
                now_unix(),
                Some(&cache_path),
            )
            .await?;
            cache.save(&cache_path)?;
            println!(
                "fetched {} rooms ({} terrains fetched, {} already cached, {} complete rooms skipped, {} statuses refreshed) -> {}",
                summary.fetched_objects,
                summary.fetched_terrain,
                summary.skipped_terrain,
                summary.skipped_complete,
                summary.refreshed_status,
                cache_path.display()
            );
        }
        Command::Score {
            rooms,
            all,
            weights,
        } => {
            let cache = load_cache_or_explain(&resolve_cache_path(&cli))?;
            let explicit = rooms.as_deref().map(parse_room_list);
            let (room_names, selection) = score::select_rooms(&cache, explicit, *all);
            if room_names.is_empty() {
                bail!("nothing to score: {selection}");
            }
            println!("scoring {} rooms ({selection})", room_names.len());
            println!();
            let result = score::stage1(&cache, &room_names, &weights.to_weights());
            print_stage1_table(&result);
        }
        Command::Recommend {
            rooms,
            all,
            weights,
            plan,
        } => {
            let cache = load_cache_or_explain(&resolve_cache_path(&cli))?;
            let explicit = rooms.as_deref().map(parse_room_list);
            let (room_names, selection) = score::select_rooms(&cache, explicit, *all);
            if room_names.is_empty() {
                bail!("nothing to recommend: {selection}");
            }
            let options = plan.to_options(weights.to_weights());
            println!(
                "recommending over {} rooms ({selection}); planning top {} with the {:?} profile",
                room_names.len(),
                options.finalists,
                options.profile
            );
            println!();
            let result = score::recommend(&cache, &room_names, &options);
            print_recommendations(&result);
            if let Some(best) = result.recommendations.first() {
                println!();
                println!(
                    "best: {} — place with `place --room {} --x {} --y {} --yes`",
                    best.room, best.room, best.spawn.0, best.spawn.1
                );
            }
        }
        Command::Place {
            room,
            x,
            y,
            name,
            yes,
            i_understand_this_is_mmo,
        } => {
            let cfg = load_config(&cli)?;
            let request = PlacementRequest {
                server_name: cfg.server_name.clone(),
                base_url: cfg.base_url.clone(),
                shard: cli.shard.clone(),
                official: cfg.is_official(),
                room: room.clone(),
                x: *x,
                y: *y,
                name: name.clone(),
            };
            // Print exactly what would happen BEFORE any gate or call.
            println!("{}", request.describe());
            gate_place(request.official, *yes, *i_understand_this_is_mmo)?;
            let client = connect_with(cfg, &cli).await?;
            client.place_spawn(room, *x, *y, name).await?;
            println!("placed spawn '{name}' in {room} at ({x}, {y})");
        }
        Command::Auto {
            yes,
            name,
            status_ttl_secs,
            weights,
            plan,
        } => {
            let cfg = load_config(&cli)?;
            // The gate runs FIRST: a refused `auto` performs zero
            // network I/O, and no flag combination unlocks it on
            // official servers.
            gate_auto(cfg.is_official(), *yes)?;

            let cache_path = resolve_cache_path(&cli);
            let mut cache = load_or_new_cache(&cache_path, &cli)?;
            let server_name = cfg.server_name.clone();
            let base_url = cfg.base_url.clone();
            let client = connect_with(cfg, &cli).await?;

            // 0. The account must not already have a spawn.
            let world = client.world_status().await?;
            if world.status == "normal" {
                bail!(
                    "world-status is 'normal' — this account already has a spawn; \
                     respawn first if you really want to move"
                );
            }
            println!("world-status: {} — proceeding", world.status);

            // 1. Scan the whole map for open rooms (TTL 0: `auto` is a
            // fresh end-to-end decision, never a resume).
            let world_size = client.world_size().await?;
            let room_names = enumerate_room_names(world_size.width, world_size.height);
            let scan = ops::scan_rooms_resumable(
                &client,
                &mut cache,
                &room_names,
                now_unix(),
                Some(&cache_path),
                0,
            )
            .await?;
            cache.save(&cache_path)?;
            println!("scan: {} rooms, {} open", scan.scanned, scan.open);
            let open: Vec<String> = cache.open_rooms().map(|r| r.room.clone()).collect();
            if open.is_empty() {
                bail!("no rooms are open for spawning on this server");
            }

            // 2. Fetch terrain + objects for the open rooms.
            let fetch = ops::fetch_rooms_resumable(
                &client,
                &mut cache,
                &open,
                *status_ttl_secs,
                now_unix(),
                Some(&cache_path),
            )
            .await?;
            cache.save(&cache_path)?;
            println!(
                "fetch: {} terrains fetched, {} already cached",
                fetch.fetched_terrain, fetch.skipped_terrain
            );

            // 3. Recommend (offline, over the cache just built).
            let options = plan.to_options(weights.to_weights());
            let result = score::recommend(&cache, &open, &options);
            println!();
            print_recommendations(&result);
            let best = result
                .recommendations
                .first()
                .context("no room produced a viable plan — nothing to place")?;

            // 4. Place the best room's plan-derived spawn tile.
            let request = PlacementRequest {
                server_name,
                base_url,
                shard: cli.shard.clone(),
                official: false, // gate_auto already refused official servers
                room: best.room.clone(),
                x: best.spawn.0 as u32,
                y: best.spawn.1 as u32,
                name: name.clone(),
            };
            println!();
            println!("{}", request.describe());
            client
                .place_spawn(&request.room, request.x, request.y, &request.name)
                .await?;
            println!(
                "placed spawn '{}' in {} at ({}, {})",
                request.name, request.room, request.x, request.y
            );
        }
        Command::Cache { action } => match action {
            CacheAction::Stats => {
                let cache_path = resolve_cache_path(&cli);
                if !cache_path.exists() {
                    bail!(
                        "no cache at {} (run `scan`, `fetch`, or `cache seed`)",
                        cache_path.display()
                    );
                }
                let cache = RoomCache::load(&cache_path)?;
                let with_terrain = cache.rooms.iter().filter(|r| r.has_terrain()).count();
                let with_status = cache
                    .rooms
                    .iter()
                    .filter(|r| r.spawn_status.is_some())
                    .count();
                let open = cache.open_rooms().count();
                println!("cache:        {}", cache_path.display());
                println!("description:  {}", cache.description);
                println!("rooms:        {}", cache.rooms.len());
                println!("with terrain: {with_terrain}");
                println!("with status:  {with_status} ({open} open)");
            }
            CacheAction::Seed { from, force } => {
                let cache_path = resolve_cache_path(&cli);
                let rooms = seed_from(from, &cache_path, *force)?;
                println!(
                    "seeded {} rooms: {} -> {} (source untouched)",
                    rooms,
                    from.display(),
                    cache_path.display()
                );
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's self-check: catches conflicting flags/ids at test time.
    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// Scan's resume TTL parses with its documented default and 0
    /// (= full rescan) stays accepted.
    #[test]
    fn scan_status_ttl_parses_with_default() {
        let cli = Cli::try_parse_from(["screeps-prospector", "scan", "--all"]).unwrap();
        let Command::Scan {
            status_ttl_secs, ..
        } = cli.command
        else {
            panic!("expected scan");
        };
        assert_eq!(status_ttl_secs, 3600);
        let cli = Cli::try_parse_from([
            "screeps-prospector",
            "scan",
            "--all",
            "--status-ttl-secs",
            "0",
        ])
        .unwrap();
        let Command::Scan {
            status_ttl_secs, ..
        } = cli.command
        else {
            panic!("expected scan");
        };
        assert_eq!(status_ttl_secs, 0);
    }

    /// The ETA strings the quota plan prints.
    #[test]
    fn eta_and_quota_formatting() {
        assert_eq!(fmt_eta(45), "45s");
        assert_eq!(fmt_eta(840), "14m 00s");
        assert_eq!(fmt_eta(3600 + 120), "1h 02m");
        let map_stats = screeps_rest_api::endpoint_quota("POST", "/api/game/map-stats").unwrap();
        assert_eq!(fmt_quota(&map_stats), "60/hour");
        let code = screeps_rest_api::endpoint_quota("POST", "/api/user/code").unwrap();
        assert_eq!(fmt_quota(&code), "240/day");
    }

    #[test]
    fn room_list_parsing() {
        assert_eq!(
            parse_room_list(" W1N1, W2N1 ,,E0S0 "),
            vec!["W1N1".to_owned(), "W2N1".to_owned(), "E0S0".to_owned()]
        );
    }

    /// The CLI weight defaults track the library defaults (one source
    /// of truth — `HeuristicWeights::default()`).
    #[test]
    fn weight_args_default_to_library_weights() {
        let cli = Cli::try_parse_from(["screeps-prospector", "score"]).unwrap();
        let Command::Score { weights, .. } = cli.command else {
            panic!("expected score");
        };
        assert_eq!(weights.to_weights(), HeuristicWeights::default());
    }

    /// `--plan-profile` parses through `PlanProfile::from_str` and
    /// rejects junk at clap level.
    #[test]
    fn plan_profile_parses() {
        let cli = Cli::try_parse_from([
            "screeps-prospector",
            "recommend",
            "--plan-profile",
            "reduced",
        ])
        .unwrap();
        let Command::Recommend { plan, .. } = cli.command else {
            panic!("expected recommend");
        };
        assert_eq!(plan.plan_profile, PlanProfile::Reduced);
        assert_eq!(plan.top, DEFAULT_FINALISTS);
        assert!(
            Cli::try_parse_from(["screeps-prospector", "recommend", "--plan-profile", "warp"])
                .is_err()
        );
    }

    /// `place` parses its named-flag form with the safety flags off by
    /// default (the gates themselves are unit-tested in `place.rs`).
    #[test]
    fn place_args_parse_with_safety_flags_defaulting_off() {
        let cli = Cli::try_parse_from([
            "screeps-prospector",
            "place",
            "--room",
            "W9N9",
            "--x",
            "24",
            "--y",
            "21",
        ])
        .unwrap();
        let Command::Place {
            room,
            x,
            y,
            name,
            yes,
            i_understand_this_is_mmo,
        } = cli.command
        else {
            panic!("expected place");
        };
        assert_eq!((room.as_str(), x, y), ("W9N9", 24, 21));
        assert_eq!(name, DEFAULT_SPAWN_NAME);
        assert!(!yes && !i_understand_this_is_mmo);
    }
}
