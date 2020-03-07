use crate::findnearest::*;
use crate::visualize::*;
use screeps::*;
use serde::*;
use std::collections::*;

#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct RoomItemData {
    required_rcl: u32,
}

#[derive(Copy, Clone, Serialize, Deserialize)]
pub enum RoomItem {
    Empty,
    Structure(StructureType, RoomItemData),
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct Location {
    packed: u16,
}

impl Location {
    fn from_coords(x: u32, y: u32) -> Self {
        Location {
            packed: ((x << 8) | y) as u16,
        }
    }

    fn from_pos(pos: RoomPosition) -> Self {
        Self::from_coords(pos.x(), pos.y())
    }

    #[inline]
    pub fn x(self) -> u8 {
        ((self.packed >> 8) & 0xFF) as u8
    }

    #[inline]
    pub fn y(self) -> u8 {
        (self.packed & 0xFF) as u8
    }

    #[inline]
    pub fn packed_repr(self) -> u16 {
        self.packed
    }

    #[inline]
    pub fn from_packed(packed: u16) -> Self {
        Location { packed }
    }

    pub fn to_room_position(self, room: RoomName) -> RoomPosition {
        RoomPosition::new(self.x() as u32, self.y() as u32, room)
    }
}

impl Serialize for Location {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.packed_repr().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Location {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        u16::deserialize(deserializer).map(Location::from_packed)
    }
}

pub type PlanState = HashMap<Location, RoomItem>;

#[derive(Clone, Serialize, Deserialize)]
pub struct Plan {
    state: PlanState,
}

impl Plan {
    pub fn execute(&self, room: &Room) {
        let room_name = room.name();
        let room_level = room.controller().map(|c| c.level()).unwrap_or(0);

        for (loc, entry) in self.state.iter() {
            match entry {
                RoomItem::Empty => {}
                RoomItem::Structure(structure_type, data) => {
                    if room_level >= data.required_rcl {
                        room.create_construction_site(&RoomPosition::new(loc.x() as u32, loc.y() as u32, room_name), *structure_type);
                    }
                }
            }
        }
    }

    pub fn visualize(&self, visualizer: &mut RoomVisualizer) {
        for (loc, entry) in self.state.iter() {
            match entry {
                RoomItem::Empty => {}
                RoomItem::Structure(StructureType::Spawn, _) => {
                    visualizer.circle(
                        loc.x() as f32,
                        loc.y() as f32,
                        Some(CircleStyle::default().fill("green").opacity(1.0)),
                    );
                }
                RoomItem::Structure(StructureType::Extension, _) => {
                    visualizer.circle(
                        loc.x() as f32,
                        loc.y() as f32,
                        Some(CircleStyle::default().fill("purple").opacity(1.0)),
                    );
                }
                RoomItem::Structure(StructureType::Container, _) => {
                    visualizer.circle(
                        loc.x() as f32,
                        loc.y() as f32,
                        Some(CircleStyle::default().fill("blue").opacity(1.0)),
                    );
                }
                RoomItem::Structure(_, _) => {
                    visualizer.circle(
                        loc.x() as f32,
                        loc.y() as f32,
                        Some(CircleStyle::default().fill("yellow").opacity(1.0)),
                    );
                }
            }
        }
    }
}

pub struct Planner<'a> {
    room: &'a Room,
}

impl<'a> Planner<'a> {
    pub fn new(room: &Room) -> Planner {
        Planner { room }
    }

    pub fn plan(&self) -> Plan {
        let mut state = PlanState::new();

        let terrain = self.room.get_terrain();

        Self::add_spawns(&self.room, &terrain, &mut state);
        Self::add_containers(&self.room, &terrain, &mut state);
        Self::add_extensions(&self.room, &terrain, &mut state);
        Self::add_extractors(&self.room, &terrain, &mut state);

        Plan { state }
    }

    fn in_room_build_bounds(pos: (i32, i32)) -> bool {
        pos.0 > 0 && pos.0 < 49 && pos.1 > 0 && pos.1 < 49
    }

    fn get_nearest_empty_terrain(terrain: &RoomTerrain, start_pos: (u32, u32)) -> Option<(u32, u32)> {
        let expanded = &[(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
        let center = &[(0, 0)];
        let search_pattern = center.iter().chain(expanded.iter());

        for pos in search_pattern {
            let room_pos = ((start_pos.0 as i32 + pos.0), (start_pos.1 as i32 + pos.1));

            if Self::in_room_build_bounds(room_pos) {
                let terrain_data = terrain.get(room_pos.0 as u32, room_pos.1 as u32);

                if terrain_data == Terrain::Plain {
                    return Some((room_pos.0 as u32, room_pos.1 as u32));
                }
            }
        }

        None
    }

    //TODO: Need much better logic for spawn placement.
    fn add_spawns(room: &Room, terrain: &RoomTerrain, state: &mut PlanState) {
        let sources = room.find(find::MY_SPAWNS);

        for source in sources.iter() {
            let pos = source.pos();

            state.insert(
                Location::from_coords(pos.x(), pos.y()),
                RoomItem::Structure(StructureType::Spawn, RoomItemData { required_rcl: 0 }),
            );
        }

        if sources.is_empty() {
            let sources = room.find(find::SOURCES);

            if sources.len() == 2 {
                if let Some(empty_start_pos) = Self::get_nearest_empty_terrain(&terrain, sources[0].pos().into()) {
                    let find_options = FindOptions::new()
                        .max_rooms(1)
                        .ignore_creeps(true)
                        .ignore_destructible_structures(true);

                    let start_pos = RoomPosition::new(empty_start_pos.0, empty_start_pos.1, room.name());
                    let end_pos = sources[1].pos();

                    if let Path::Vectorized(path) = start_pos.find_path_to(&end_pos, find_options) {
                        if !path.is_empty() {
                            let mid_point = &path[path.len() / 2];

                            state.insert(
                                Location::from_coords(mid_point.x, mid_point.y),
                                RoomItem::Structure(StructureType::Spawn, RoomItemData { required_rcl: 0 }),
                            );
                        }
                    }
                }
            }
        }
    }

    fn extension_count_to_rcl(count: u32) -> Option<u32> {
        match count {
            0 => Some(0),
            1..=5 => Some(2),
            6..=10 => Some(3),
            11..=20 => Some(4),
            21..=30 => Some(5),
            31..=40 => Some(6),
            41..=50 => Some(7),
            51..=60 => Some(8),
            _ => None,
        }
    }

    fn add_extensions(_room: &Room, terrain: &RoomTerrain, state: &mut PlanState) {
        let spawn_positions: Vec<Location> = state
            .iter()
            .filter_map(|(pos, entry)| match entry {
                RoomItem::Structure(StructureType::Spawn, _) => Some(pos),
                _ => None,
            })
            .cloned()
            .collect();

        let mut current_extensions = 0;
        let corner_points = [(-1, -1), (-1, 1), (1, 1), (1, -1)];
        let mut rcl = Self::extension_count_to_rcl(current_extensions);

        for spawn_pos in spawn_positions {
            let mut expansion = 1;
            while rcl.is_some() {
                let expanded_corner_points: Vec<(i32, i32)> = corner_points.iter().map(|(x, y)| (x * expansion, y * expansion)).collect();
                for i in 0..expanded_corner_points.len() {
                    let mut current_pos = expanded_corner_points[i % expanded_corner_points.len()];
                    let end_pos = expanded_corner_points[(i + 1) % expanded_corner_points.len()];

                    let step_start = corner_points[i % corner_points.len()];
                    let step_end = corner_points[(i + 1) % corner_points.len()];

                    let delta_x = step_end.0 - step_start.0;
                    let delta_y = step_end.1 - step_start.1;

                    while current_pos != end_pos && rcl.is_some() {
                        let room_pos = ((spawn_pos.x() as i32 + current_pos.0), (spawn_pos.y() as i32 + current_pos.1));

                        let location = Location::from_coords(room_pos.0 as u32, room_pos.1 as u32);

                        if Self::in_room_build_bounds(room_pos) && state.get(&location).is_none() {
                            match terrain.get(room_pos.0 as u32, room_pos.1 as u32) {
                                Terrain::Plain | Terrain::Swamp => {
                                    state.insert(
                                        Location::from_coords(room_pos.0 as u32, room_pos.1 as u32),
                                        RoomItem::Structure(
                                            StructureType::Extension,
                                            RoomItemData {
                                                required_rcl: rcl.unwrap(),
                                            },
                                        ),
                                    );

                                    current_extensions += 1;
                                    rcl = Self::extension_count_to_rcl(current_extensions);
                                }
                                _ => {}
                            }
                        }

                        current_pos.0 += delta_x;
                        current_pos.1 += delta_y;
                    }

                    if rcl.is_none() {
                        break;
                    }
                }

                expansion += 1;
            }
        }
    }

    fn add_containers(room: &Room, _terrain: &RoomTerrain, state: &mut PlanState) {
        let spawn_positions: Vec<Location> = state
            .iter()
            .filter_map(|(pos, entry)| match entry {
                RoomItem::Structure(StructureType::Spawn, _) => Some(pos),
                _ => None,
            })
            .cloned()
            .collect();

        for source in room.find(find::SOURCES) {
            let nearest_spawn_path = spawn_positions
                .iter()
                .map(|p| p.to_room_position(room.name()))
                .find_nearest_path_to(source.pos(), PathFinderHelpers::same_room_ignore_creeps_and_structures_range_1);

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = last_step.x as i32;
                    let pos_y = last_step.y as i32;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem::Structure(StructureType::Container, RoomItemData { required_rcl: 2 }),
                    );
                }
            }
        }

        if let Some(controller) = room.controller() {
            let nearest_spawn_path = spawn_positions
                .iter()
                .map(|p| p.to_room_position(room.name()))
                .find_nearest_path_to(controller.pos(), PathFinderHelpers::same_room_ignore_creeps_and_structures_range_1);

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = last_step.x as i32;
                    let pos_y = last_step.y as i32;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem::Structure(StructureType::Container, RoomItemData { required_rcl: 2 }),
                    );
                }
            }
        }
    }

    fn add_extractors(room: &Room, _terrain: &RoomTerrain, state: &mut PlanState) {
        let spawn_positions: Vec<Location> = state
            .iter()
            .filter_map(|(pos, entry)| match entry {
                RoomItem::Structure(StructureType::Spawn, _) => Some(pos),
                _ => None,
            })
            .cloned()
            .collect();

        for mineral in room.find(find::MINERALS) {
            state.insert(
                Location::from_pos(mineral.pos()),
                RoomItem::Structure(StructureType::Extractor, RoomItemData { required_rcl: 6 }),
            );

            let nearest_spawn_path = spawn_positions
                .iter()
                .map(|p| p.to_room_position(room.name()))
                .find_nearest_path_to(mineral.pos(), PathFinderHelpers::same_room_ignore_creeps_and_structures_range_1);

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = last_step.x as i32;
                    let pos_y = last_step.y as i32;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem::Structure(StructureType::Container, RoomItemData { required_rcl: 6 }),
                    );
                }
            }
        }
    }
}
