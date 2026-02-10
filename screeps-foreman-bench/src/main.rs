use clap::{Parser, Subcommand};
use screeps_foreman::location::*;
use screeps_foreman::plan::*;
use screeps_foreman::planner::*;
use screeps_foreman::room_data::*;
use screeps_foreman::visual::*;
use log::*;
use std::fs::File;
use std::path::Path;
use std::io::Read;
use serde::*;
use image::*;
use std::time::*;
use std::collections::HashMap;
#[cfg(not(feature = "profile"))]
use rayon::prelude::*;

/// Offline room planner and visualization tool for screeps-foreman.
///
/// Loads map data from a JSON file, runs the room planning pipeline,
/// and outputs PNG images, plan JSON, and score reports.
#[derive(Parser)]
#[command(name = "screeps-foreman-bench")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Plan a single room and output results.
    Plan {
        /// Path to the map data JSON file (e.g. resources/map-mmo-shard3.json).
        #[arg(short, long)]
        map: String,

        /// Room name to plan (e.g. W3S52).
        #[arg(short, long)]
        room: String,

        /// Shard name, used in output metadata.
        #[arg(short, long, default_value = "shard1")]
        shard: String,

        /// Output directory for generated files.
        #[arg(short, long, default_value = "output")]
        output: String,
    },

    /// Plan multiple rooms and rank them by score.
    Compare {
        /// Path to the map data JSON file.
        #[arg(short, long)]
        map: String,

        /// Room names to compare (comma-separated, e.g. W3S52,E11N11).
        /// If omitted, plans all rooms with 2 sources and a controller.
        #[arg(short, long)]
        rooms: Option<String>,

        /// Shard name, used in output metadata.
        #[arg(short, long, default_value = "shard1")]
        shard: String,

        /// Maximum number of rooms to plan (when --rooms is omitted).
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Output directory for generated files.
        #[arg(short, long, default_value = "output")]
        output: String,
    },

    /// List all rooms in a map data file.
    ListRooms {
        /// Path to the map data JSON file.
        #[arg(short, long)]
        map: String,

        /// Only list rooms with this many sources.
        #[arg(long)]
        sources: Option<usize>,

        /// Only list rooms that have a controller.
        #[arg(long)]
        has_controller: bool,
    },
}

struct RoomDataPlannerDataSource {
    terrain: FastRoomTerrain,
    controllers: Vec<PlanLocation>,
    sources: Vec<PlanLocation>,
    minerals: Vec<PlanLocation>,
}

impl PlannerRoomDataSource for RoomDataPlannerDataSource {
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

fn main() -> Result<(), String> {
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Plan { map, room, shard, output } => {
            cmd_plan(&map, &room, &shard, &output)
        }
        Commands::Compare { map, rooms, shard, limit, output } => {
            cmd_compare(&map, rooms.as_deref(), &shard, limit, &output)
        }
        Commands::ListRooms { map, sources, has_controller } => {
            cmd_list_rooms(&map, sources, has_controller)
        }
    }
}

fn cmd_plan(map_path: &str, room_name: &str, shard: &str, output_dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(output_dir)
        .map_err(|err| format!("Failed to create output folder '{}': {}", output_dir, err))?;

    info!("Loading map data from {}...", map_path);
    let map_data = load_map_data(map_path)?;
    info!("Finished loading map data ({} rooms)", map_data.rooms.len());

    let room = map_data.get_room(room_name)?;
    run_room(shard, room, output_dir)?;

    println!("Done. Output written to {}/", output_dir);
    Ok(())
}

fn cmd_compare(
    map_path: &str,
    rooms_arg: Option<&str>,
    shard: &str,
    limit: usize,
    output_dir: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(output_dir)
        .map_err(|err| format!("Failed to create output folder '{}': {}", output_dir, err))?;

    info!("Loading map data from {}...", map_path);
    let map_data = load_map_data(map_path)?;
    info!("Finished loading map data ({} rooms)", map_data.rooms.len());

    let rooms: Vec<&BenchRoomData> = if let Some(names) = rooms_arg {
        names
            .split(',')
            .map(|n| map_data.get_room(n.trim()))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        map_data
            .rooms
            .iter()
            .filter(|r| r.get_sources().len() == 2 && r.get_controllers().len() == 1)
            .take(limit)
            .collect()
    };

    println!("Planning {} rooms...", rooms.len());

    #[cfg(not(feature = "profile"))]
    let room_iter = rooms.par_iter();

    #[cfg(feature = "profile")]
    let room_iter = rooms.iter();

    let mut results: Vec<_> = room_iter
        .filter_map(|room| {
            match run_room(shard, room, output_dir) {
                Ok(plan) => Some((room.name().to_owned(), plan)),
                Err(err) => {
                    error!("Failed planning {}: {}", room.name(), err);
                    None
                }
            }
        })
        .collect();

    // Sort by total score descending
    results.sort_by(|a, b| {
        b.1.score
            .total
            .partial_cmp(&a.1.score.total)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    println!();
    println!("{:<10} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "Room", "Total", "SrcDst", "CtrlD", "Hub", "Tower", "ExtEff", "Upkeep");
    println!("{}", "-".repeat(73));
    for (name, plan) in &results {
        let s = &plan.score;
        println!("{:<10} {:>7.4} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3}",
            name, s.total, s.source_distance, s.controller_distance,
            s.hub_quality, s.tower_coverage, s.extension_efficiency, s.upkeep_cost);
    }

    println!();
    println!("Done. Output written to {}/", output_dir);
    Ok(())
}

fn cmd_list_rooms(map_path: &str, sources_filter: Option<usize>, has_controller: bool) -> Result<(), String> {
    let map_data = load_map_data(map_path)?;

    let mut count = 0;
    for room in &map_data.rooms {
        let nsources = room.get_sources().len();
        let ncontrollers = room.get_controllers().len();

        if let Some(required) = sources_filter {
            if nsources != required {
                continue;
            }
        }
        if has_controller && ncontrollers == 0 {
            continue;
        }

        println!("{:<10} sources={} controllers={} minerals={}",
            room.name(), nsources, ncontrollers, room.get_minerals().len());
        count += 1;
    }

    println!();
    println!("{} rooms listed.", count);
    Ok(())
}

fn run_room(shard: &str, room: &BenchRoomData, output_dir: &str) -> Result<Plan, String> {
    let room_name = room.name();

    info!("Planning: {}", room_name);

    let epoch = Instant::now();

    #[cfg(feature = "profile")]
    {
        let epoch = epoch.clone();

        screeps_timing::start_trace(Box::new(move || {
            let elapsed = epoch.elapsed();
            elapsed.as_micros() as u64
        }))
    }

    let data_source = RoomDataPlannerDataSource {
        terrain: room.get_terrain()?,
        controllers: room.get_controllers(),
        sources: room.get_sources(),
        minerals: room.get_minerals(),
    };

    let plan = plan_room(&data_source).map_err(|e| format!("Planning failed: {}", e))?;

    let duration = epoch.elapsed().as_secs_f32();
    info!("Planning complete - Duration: {:.3}s", duration);
    info!(
        "Plan score: {:.4} (source_dist={:.3}, ctrl_dist={:.3}, hub_quality={:.3}, tower_cov={:.3})",
        plan.score.total,
        plan.score.source_distance,
        plan.score.controller_distance,
        plan.score.hub_quality,
        plan.score.tower_coverage,
    );

    #[cfg(feature = "profile")]
    {
        info!("Gathering trace...");
        let trace = screeps_timing::stop_trace();
        info!("Done gathering trace");

        let trace_name = format!("{}/{}_trace.json", output_dir, room_name);
        let trace_file =
            &File::create(trace_name).map_err(|err| format!("Failed to create trace file: {}", err))?;

        info!("Serializing trace to disk...");
        serde_json::to_writer(trace_file, &trace)
            .map_err(|err| format!("Failed to serialize json: {}", err))?;
        info!("Done serializing");
    }

    let mut img: RgbImage = ImageBuffer::new(500, 500);

    let terrain = room.get_terrain()?;
    render_terrain(&mut img, &terrain, 10);
    render_plan(&mut img, &plan, 10);

    let output_img_name = format!("{}/{}.png", output_dir, room_name);
    img.save(output_img_name).map_err(|err| format!("Failed to save image: {}", err))?;

    let serialized_plan = serialize_plan(shard, room, &plan)?;
    let output_plan_name = format!("{}/{}_plan.json", output_dir, room_name);
    let output_plan_file =
        &File::create(output_plan_name).map_err(|err| format!("Failed to create plan file: {}", err))?;
    serde_json::to_writer(output_plan_file, &serialized_plan)
        .map_err(|err| format!("Failed to write plan json: {}", err))?;

    // Report serialized pipeline state size
    let plan_json = serde_json::to_string(&plan).map_err(|e| format!("Failed to serialize plan: {}", e))?;
    info!("Serialized plan size: {} bytes", plan_json.len());

    Ok(plan)
}

#[derive(Serialize, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
enum RoomPlannerStructure {
    Spawn = 0,
    Extension = 1,
    Road = 2,
    Wall = 3,
    Rampart = 4,
    KeeperLair = 5,
    Portal = 6,
    Controller = 7,
    Link = 8,
    Storage = 9,
    Tower = 10,
    Observer = 11,
    PowerBank = 12,
    PowerSpawn = 13,
    Extractor = 14,
    Lab = 15,
    Terminal = 16,
    Container = 17,
    Nuker = 18,
    Factory = 19,
    InvaderCore = 20,
}

impl From<screeps_foreman::shim::StructureType> for RoomPlannerStructure {
    fn from(data: screeps_foreman::shim::StructureType) -> Self {
        match data {
            screeps_foreman::shim::StructureType::Spawn => RoomPlannerStructure::Spawn,
            screeps_foreman::shim::StructureType::Extension => RoomPlannerStructure::Extension,
            screeps_foreman::shim::StructureType::Road => RoomPlannerStructure::Road,
            screeps_foreman::shim::StructureType::Wall => RoomPlannerStructure::Wall,
            screeps_foreman::shim::StructureType::Rampart => RoomPlannerStructure::Rampart,
            screeps_foreman::shim::StructureType::KeeperLair => RoomPlannerStructure::KeeperLair,
            screeps_foreman::shim::StructureType::Portal => RoomPlannerStructure::Portal,
            screeps_foreman::shim::StructureType::Controller => RoomPlannerStructure::Controller,
            screeps_foreman::shim::StructureType::Link => RoomPlannerStructure::Link,
            screeps_foreman::shim::StructureType::Storage => RoomPlannerStructure::Storage,
            screeps_foreman::shim::StructureType::Tower => RoomPlannerStructure::Tower,
            screeps_foreman::shim::StructureType::Observer => RoomPlannerStructure::Observer,
            screeps_foreman::shim::StructureType::PowerBank => RoomPlannerStructure::PowerBank,
            screeps_foreman::shim::StructureType::PowerSpawn => RoomPlannerStructure::PowerSpawn,
            screeps_foreman::shim::StructureType::Extractor => RoomPlannerStructure::Extractor,
            screeps_foreman::shim::StructureType::Lab => RoomPlannerStructure::Lab,
            screeps_foreman::shim::StructureType::Terminal => RoomPlannerStructure::Terminal,
            screeps_foreman::shim::StructureType::Container => RoomPlannerStructure::Container,
            screeps_foreman::shim::StructureType::Nuker => RoomPlannerStructure::Nuker,
            screeps_foreman::shim::StructureType::Factory => RoomPlannerStructure::Factory,
            screeps_foreman::shim::StructureType::InvaderCore => RoomPlannerStructure::InvaderCore,
        }
    }
}

#[derive(Serialize)]
struct RoomPlannerPosition {
    x: u8,
    y: u8,
}

impl From<Location> for RoomPlannerPosition {
    fn from(data: Location) -> Self {
        Self {
            x: data.x(),
            y: data.y(),
        }
    }
}

#[derive(Serialize, Default)]
struct RoomPlannerEntry {
    pos: Vec<RoomPlannerPosition>,
}

#[derive(Serialize)]
struct RoomPlannerOutputData {
    name: String,
    shard: String,
    rcl: u32,
    buildings: HashMap<RoomPlannerStructure, RoomPlannerEntry>,
}

impl RoomVisualizer for RoomPlannerOutputData {
    fn render(&mut self, location: Location, structure_type: screeps_foreman::shim::StructureType) {
        let entry = self
            .buildings
            .entry(structure_type.into())
            .or_insert_with(RoomPlannerEntry::default);
        entry.pos.push(location.into());
    }
}

fn serialize_plan(
    shard: &str,
    room_data: &BenchRoomData,
    plan: &Plan,
) -> Result<RoomPlannerOutputData, String> {
    let mut data = RoomPlannerOutputData {
        name: room_data.name().to_owned(),
        shard: shard.to_owned(),
        rcl: 8,
        buildings: HashMap::new(),
    };

    plan.visualize(&mut data);

    Ok(data)
}

fn fill_region(img: &mut RgbImage, x: u32, y: u32, width: u32, height: u32, val: image::Rgb<u8>) {
    for x in x..x + width {
        for y in y..y + height {
            img.put_pixel(x, y, val);
        }
    }
}

fn render_terrain(img: &mut RgbImage, terrain: &FastRoomTerrain, pixel_size: u32) {
    for x in 0..50 {
        for y in 0..50 {
            let val = terrain.get_xy(x, y);
            let color = if val.contains(TerrainFlags::WALL) {
                Rgb([0, 0, 0])
            } else if val.contains(TerrainFlags::SWAMP) {
                Rgb([255, 255, 255])
            } else {
                Rgb([127, 127, 127])
            };
            fill_region(
                img,
                (x as u32) * pixel_size,
                (y as u32) * pixel_size,
                pixel_size,
                pixel_size,
                color,
            );
        }
    }
}

struct ImgVisualizer<'a> {
    img: &'a mut RgbImage,
    pixel_size: u32,
}

impl<'a> RoomVisualizer for ImgVisualizer<'a> {
    fn render(&mut self, location: Location, structure: screeps_foreman::shim::StructureType) {
        let color = match structure {
            screeps_foreman::shim::StructureType::Road => Rgb([50, 0, 50]),
            screeps_foreman::shim::StructureType::Rampart => Rgb([0, 255, 0]),
            screeps_foreman::shim::StructureType::Spawn => Rgb([255, 255, 0]),
            screeps_foreman::shim::StructureType::Storage => Rgb([0, 255, 255]),
            screeps_foreman::shim::StructureType::Extension => Rgb([200, 100, 0]),
            screeps_foreman::shim::StructureType::Tower => Rgb([255, 0, 0]),
            screeps_foreman::shim::StructureType::Lab => Rgb([0, 200, 200]),
            screeps_foreman::shim::StructureType::Link => Rgb([255, 165, 0]),
            screeps_foreman::shim::StructureType::Terminal => Rgb([255, 192, 203]),
            screeps_foreman::shim::StructureType::Container => Rgb([0, 0, 255]),
            _ => Rgb([255, 0, 0]),
        };

        fill_region(
            &mut self.img,
            location.x() as u32 * self.pixel_size,
            location.y() as u32 * self.pixel_size,
            self.pixel_size,
            self.pixel_size,
            color,
        );
    }
}

fn render_plan(img: &mut RgbImage, plan: &Plan, pixel_size: u32) {
    let mut visualizer = ImgVisualizer { img, pixel_size };
    plan.visualize(&mut visualizer);
}

fn terrain_string_to_vec<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct JsonStringVisitor;

    impl<'de> de::Visitor<'de> for JsonStringVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string containing terrain data")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            let mut buffer = Vec::with_capacity(v.len());
            for mask in v.chars() {
                let val = mask
                    .to_digit(16)
                    .ok_or_else(|| E::custom("Expected hex digit character".to_owned()))?
                    as u8;
                buffer.push(val);
            }
            Ok(buffer)
        }
    }

    deserializer.deserialize_any(JsonStringVisitor)
}

#[derive(Deserialize)]
struct BenchRoomData {
    room: String,
    #[serde(deserialize_with = "terrain_string_to_vec")]
    terrain: Vec<u8>,
    objects: Vec<serde_json::Value>,
}

impl BenchRoomData {
    fn name(&self) -> &str {
        &self.room
    }

    fn get_terrain(&self) -> Result<FastRoomTerrain, String> {
        if self.terrain.len() != 50 * 50 {
            return Err("Terrain was not expected 50 x 50 layout".to_owned());
        }
        Ok(FastRoomTerrain::new(self.terrain.clone()))
    }

    fn get_object_type(&self, obj_type: &str) -> Vec<PlanLocation> {
        self.objects
            .iter()
            .filter_map(|o| o.as_object())
            .filter(|o| o.get("type").map(|t| t == obj_type).unwrap_or(false))
            .filter_map(|o| {
                let x = o.get("x")?.as_i64()?;
                let y = o.get("y")?.as_i64()?;
                Some(PlanLocation::new(x as i8, y as i8))
            })
            .collect()
    }

    fn get_sources(&self) -> Vec<PlanLocation> {
        self.get_object_type("source")
    }

    fn get_controllers(&self) -> Vec<PlanLocation> {
        self.get_object_type("controller")
    }

    fn get_minerals(&self) -> Vec<PlanLocation> {
        self.get_object_type("mineral")
    }
}

#[derive(Deserialize)]
struct MapData {
    rooms: Vec<BenchRoomData>,
}

impl MapData {
    fn get_room(&self, room_name: &str) -> Result<&BenchRoomData, String> {
        self.rooms
            .iter()
            .find(|room| room.name() == room_name)
            .ok_or_else(|| format!("Room '{}' not found in map data", room_name))
    }
}

fn load_map_data<P>(path: P) -> Result<MapData, String>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    let mut file =
        File::open(path).map_err(|err| format!("Failed to open map file '{}': {}", path.display(), err))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|err| format!("Failed to read map file: {}", err))?;
    let data: MapData =
        serde_json::from_str(&contents).map_err(|err| format!("Failed to parse map JSON: {}", err))?;
    Ok(data)
}
