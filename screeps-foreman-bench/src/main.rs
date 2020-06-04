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

    info!("Loading map data...");
    let map_data = load_map_data("resources/map-mmo-shard3.json")?;
    info!("Finished loading map data");

    info!("Getting room features...");
    let room = map_data.get_room("E34S31")?;
    info!("Finished getting room features");

    info!("Planning...");
    let plan = evaluate_plan(&room)?.ok_or("Failed to create plan for room")?;
    info!("Plan complete");

    let mut img: RgbImage = ImageBuffer::new(500, 500);

    let terrain = room.get_terrain()?;
    render_terrain(&mut img, &terrain, 10);

    render_plan(&mut img, &plan, 10);

    img.save("output.png").map_err(|err| format!("Failed to save image: {}", err))?;

    Ok(())
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
        let color = Rgb([255, 0, 0]);

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
    description: String,
    rooms: Vec<RoomData>
}

impl MapData {
    fn get_room(&self, room_name: &str) -> Result<&RoomData, String> {
        self
            .rooms
            .iter()
            .find(|room| room.room == room_name)
            .ok_or("Failed to find room".to_owned())
    }
}

fn load_map_data<P>(path: P) -> Result<MapData, String> where P: AsRef<Path>  {
    let mut file = File::open(path).map_err(|err| format!("Failed to open map file: {}", err))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|err| format!("Failed to read string to buffer: {}", err))?;

    let data: MapData = serde_json::from_str(&contents).map_err(|err| format!("Failed to load json: {}", err))?;

    Ok(data)
}

fn evaluate_plan(room: &RoomData) -> Result<Option<Plan>, String> {
    let mut data_source = RoomDataPlannerDataSource {
        terrain: room.get_terrain()?,
        controllers: room.get_controllers(),
        sources: room.get_sources(),
        minerals: room.get_minerals()
    };

    let planner = Planner::new(screeps_foreman::scoring::score_state);

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

    let evaluate_result = planner.evaluate(screeps_foreman::layout::ALL_ROOT_NODES, &mut data_source, &mut running_state, || true)?;

    let plan = match evaluate_result {
        PlanEvaluationResult::Complete(plan) => {
            if plan.is_some() {
                info!("Evalaute complete - planned room layout.");
            } else {
                info!("Evalaute complete - failed to find room layout.");
            }
            
            Ok(plan)
        },
        PlanEvaluationResult::Running() => {
            Err("Expected to run to completion".to_owned())
        }
    }?;

    Ok(plan)
}