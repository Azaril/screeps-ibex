use screeps_foreman::planner::*;
use screeps_foreman::visual::*;
use screeps_foreman::location::*;
use screeps_foreman::*;
use log::*;
use std::fs::File;
use std::path::Path;
use std::io::Read;
use serde::*;
use std::convert::*;
use image::*;
use std::time::*;
use std::collections::HashMap;
#[cfg(not(feature = "profile"))]
use rayon::prelude::*;

struct RoomDataPlannerDataSource {
    terrain: FastRoomTerrain,
    controllers: Vec<PlanLocation>,
    sources: Vec<PlanLocation>,
    minerals: Vec<PlanLocation>,
}

impl PlannerRoomDataSource for RoomDataPlannerDataSource {
    fn get_terrain(&mut self) -> &FastRoomTerrain {
        &self.terrain
    }

    fn get_controllers(&mut self) -> &[PlanLocation] {
        &self.controllers
    }

    fn get_sources(&mut self) -> &[PlanLocation] {
        &self.sources
    }

    fn get_minerals(&mut self) -> &[PlanLocation] {
        &self.minerals
    }
}

fn main() -> Result<(), String> {
    env_logger::init();

    std::fs::create_dir_all("output").map_err(|err| format!("Failed to create output folder: {}", err))?;

    info!("Loading map data...");
    let shard = "shard2";    
    let map_data = load_map_data(format!("resources/map-mmo-{}.json", shard))?;
    info!("Finished loading map data");

    /*
    let rooms = map_data
        .get_rooms()
        .iter()
        .filter(|room_data| room_data.get_sources().len() == 2 && room_data.get_controllers().len() == 1)
        .take(1000)
        .collect::<Vec<_>>();
    */

    let rooms = vec![
        //map_data.get_room("E33S31")?,
        //map_data.get_room("E34S31")?,
        //map_data.get_room("E35S31")?,
        
        //map_data.get_room("E33S31")?,
        //map_data.get_room("E11N11")?,

        //map_data.get_room("E11N1")?,
        //map_data.get_room("E29S11")?,

        map_data.get_room("E21S44")?,
    ];

    let maximum_seconds = Some(60.0);
    //let maximum_seconds = Some(2.0);

    //let maximum_batch_seconds = None;
    let maximum_batch_seconds = Some(5.0);

    
    #[cfg(not(feature = "profile"))]
    let room_iter = rooms.par_iter();

    #[cfg(feature = "profile")]
    let room_iter = rooms.iter();

    let room_results: Vec<_> = room_iter
        .map(|room| {
            let res = run_room(shard, &room, maximum_seconds, maximum_batch_seconds);

            (room, res)
        })
        .collect();

    for (room, result) in room_results {
        match result {
            Ok(()) => {
                info!("Succesfully ran room planning: {}", room.name());
            },
            Err(err) => {
                error!("Failed running room planning: {} - Error: {}", room.name(), err);
            }
        }
    }

    Ok(())
}

fn run_room(shard: &str, room: &RoomData, maximum_seconds: Option<f32>, maximum_batch_seconds: Option<f32>) -> Result<(), String> {
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

    let plan_result = evaluate_plan(&room, maximum_seconds, maximum_batch_seconds);
    
    let duration = epoch.elapsed().as_secs_f32();

    info!("Planning complete - Duration: {}", duration);

    #[cfg(feature = "profile")]
    {
        info!("Gathering trace...");
        let trace = screeps_timing::stop_trace();
        info!("Done gathering trace");

        let trace_name = format!("output/{}_trace.json", room_name);
        let trace_file = &File::create(trace_name).map_err(|err| format!("Failed to crate trace file: {}", err))?;

        info!("Serializing trace to disk...");
        serde_json::to_writer(trace_file, &trace).map_err(|err| format!("Failed to serialize json: {}", err))?;
        info!("Done serializing");
    }

    let plan = plan_result?.ok_or("Failed to create plan for room")?;

    let mut img: RgbImage = ImageBuffer::new(500, 500);

    let terrain = room.get_terrain()?;
    render_terrain(&mut img, &terrain, 10);

    render_plan(&mut img, &plan, 10);

    let output_img_name = format!("output/{}.png", room_name);
    img.save(output_img_name).map_err(|err| format!("Failed to save image: {}", err))?;

    let serialized_plan = serialize_plan(shard, &room, &plan)?;

    let output_plan_name = format!("output/{}_plan.json", room_name);
    let output_plan_file = &File::create(output_plan_name).map_err(|err| format!("Failed to crate plan file: {}", err))?;

    serde_json::to_writer(output_plan_file, &serialized_plan).map_err(|err| format!("Failed to write plan json: {}", err))?;

    Ok(())
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

impl From<StructureType> for RoomPlannerStructure {
    fn from(data: StructureType) -> Self {
        match data {
            StructureType::Spawn => RoomPlannerStructure::Spawn,
            StructureType::Extension => RoomPlannerStructure::Extension,
            StructureType::Road => RoomPlannerStructure::Road,
            StructureType::Wall => RoomPlannerStructure::Wall,
            StructureType::Rampart => RoomPlannerStructure::Rampart,
            StructureType::KeeperLair => RoomPlannerStructure::KeeperLair,
            StructureType::Portal => RoomPlannerStructure::Portal,
            StructureType::Controller => RoomPlannerStructure::Controller,
            StructureType::Link => RoomPlannerStructure::Link,
            StructureType::Storage => RoomPlannerStructure::Storage,
            StructureType::Tower => RoomPlannerStructure::Tower,
            StructureType::Observer => RoomPlannerStructure::Observer,
            StructureType::PowerBank => RoomPlannerStructure::PowerBank,
            StructureType::PowerSpawn => RoomPlannerStructure::PowerSpawn,
            StructureType::Extractor => RoomPlannerStructure::Extractor,
            StructureType::Lab => RoomPlannerStructure::Lab,
            StructureType::Terminal => RoomPlannerStructure::Terminal,
            StructureType::Container => RoomPlannerStructure::Container,
            StructureType::Nuker => RoomPlannerStructure::Nuker,
            StructureType::Factory => RoomPlannerStructure::Factory,
            StructureType::InvaderCore => RoomPlannerStructure::InvaderCore,
        }
    }
}

#[derive(Serialize)]
struct RoomPlannerPosition {
    x: u8,
    y: u8
}

impl From<Location> for RoomPlannerPosition {
    fn from(data: Location) -> Self {
        Self {
            x: data.x(),
            y: data.y()
        }
    }
}

#[derive(Serialize, Default)]
struct RoomPlannerEntry {
    pos: Vec<RoomPlannerPosition>
}

#[derive(Serialize)]
struct RoomPlannerData {
    name: String,
    shard: String,
    rcl: u32,
    buildings: HashMap<RoomPlannerStructure, RoomPlannerEntry>
}

impl RoomVisualizer for RoomPlannerData {
    fn render(&mut self, location: Location, structure_type: StructureType) { 
        let entry = self.buildings
            .entry(structure_type.into())
            .or_insert_with(RoomPlannerEntry::default);

        entry.pos.push(location.into());
    }
}

fn serialize_plan(shard: &str, room_data: &RoomData, plan: &Plan) -> Result<RoomPlannerData, String> {
    let mut data = RoomPlannerData {
        name: room_data.name().to_owned(),
        shard: shard.to_owned(),
        rcl: 8,
        buildings: HashMap::new()
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

            fill_region(img, (x as u32) * pixel_size, (y as u32) * pixel_size, pixel_size, pixel_size, color);
        }
    }
}

struct ImgVisualizer<'a> {
    img: &'a mut RgbImage,
    pixel_size: u32
}

impl<'a> RoomVisualizer for ImgVisualizer<'a> {
    fn render(&mut self, location: Location, structure: StructureType) {
        let color = match structure {
            StructureType::Road => Rgb([50, 0, 50]),
            StructureType::Rampart => Rgb([0, 255, 0]),
            StructureType::Spawn => Rgb([255, 255, 0]),
            StructureType::Storage => Rgb([0, 255, 255]),
            _ => Rgb([255, 0, 0]),
        };

        fill_region(&mut self.img, location.x() as u32 * self.pixel_size, location.y() as u32 * self.pixel_size, self.pixel_size, self.pixel_size, color);
    }
}

fn render_plan(img: &mut RgbImage, plan: &Plan, pixel_size: u32) {
    let mut visualizer = ImgVisualizer {
        img,
        pixel_size
    };

    plan.visualize(&mut visualizer)
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
                let val = mask.to_digit(16).ok_or_else(|| E::custom("Expected hex digit character".to_owned()))? as u8;

                buffer.push(val);
            }

            Ok(buffer)
        }
    }
    
    deserializer.deserialize_any(JsonStringVisitor)
}

#[derive(Deserialize)]
struct RoomData {
    room: String,
    #[serde(deserialize_with = "terrain_string_to_vec")]
    terrain: Vec<u8>,
    objects: Vec<serde_json::Value>
}

impl RoomData {
    fn name(&self) -> &str {
        &self.room
    }

    fn get_terrain(&self) -> Result<FastRoomTerrain, String> {
        if self.terrain.len() != 50 * 50 {
            return Err("Terrain was not expected 50 x 50 layout".to_owned());
        }

        let terrain = FastRoomTerrain::new(self.terrain.clone());

        Ok(terrain)
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
    rooms: Vec<RoomData>
}

impl MapData {
    #[allow(dead_code)]
    fn get_room(&self, room_name: &str) -> Result<&RoomData, String> {
        self
            .rooms
            .iter()
            .find(|room| room.name() == room_name)
            .ok_or("Failed to find room".to_owned())
    }

    #[allow(dead_code)]
    fn get_rooms(&self) -> &[RoomData] {
        &self.rooms
    }
}

fn load_map_data<P>(path: P) -> Result<MapData, String> where P: AsRef<Path>  {
    let mut file = File::open(path).map_err(|err| format!("Failed to open map file: {}", err))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|err| format!("Failed to read string to buffer: {}", err))?;

    let data: MapData = serde_json::from_str(&contents).map_err(|err| format!("Failed to load json: {}", err))?;

    Ok(data)
}

fn evaluate_plan(room: &RoomData, max_seconds: Option<f32>, max_batch_seconds: Option<f32>) -> Result<Option<Plan>, String> {
    let mut data_source = RoomDataPlannerDataSource {
        terrain: room.get_terrain()?,
        controllers: room.get_controllers(),
        sources: room.get_sources(),
        minerals: room.get_minerals()
    };

    let planner = Planner::new(screeps_foreman::scoring::score_state);

    let epoch = Instant::now();

    let seed_result = planner.seed(screeps_foreman::layout::ALL_ROOT_NODES, &mut data_source)?;

    let mut running_state = match seed_result {
        PlanSeedResult::Complete(plan) => {
            info!("Seeding complete - plan complete");

            return Ok(plan);
        }
        PlanSeedResult::Running(run_state) => {
            info!("Seeding complete - pending evaluation");

            run_state
        }
    };

    info!("Starting evaluating...");

    let plan = loop {
        let batch_epoch = Instant::now();

        let evaluate_result = planner.evaluate(
            screeps_foreman::layout::ALL_ROOT_NODES, 
            &mut data_source, 
            &mut running_state, 
            || {
                let elapsed = epoch.elapsed().as_secs_f32();

                if max_seconds.map(|max| elapsed >= max).unwrap_or(false) {
                    return false;
                }

                let batch_elapsed = batch_epoch.elapsed().as_secs_f32();

                if max_batch_seconds.map(|max| batch_elapsed >= max).unwrap_or(false) {
                    return false;
                }

                true
            }
        )?;
    
        match evaluate_result {
            PlanEvaluationResult::Complete(plan) => {
                if plan.is_some() {
                    info!("Evaluate complete - planned room layout.");
                } else {
                    info!("Evaluate complete - failed to find room layout.");
                }
                
                break Ok(plan)
            },
            PlanEvaluationResult::Running() => {
                if max_seconds.map(|max| epoch.elapsed().as_secs_f32() >= max).unwrap_or(false) {
                    break Err("Exceeded maximum duration for planning".to_owned());
                }
            }
        };
    }?;

    Ok(plan)
}