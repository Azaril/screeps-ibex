use crate::findnearest::*;
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
    Terrain,
    Source,
    Container(RoomItemData),
    Road,
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
                RoomItem::Terrain => {}
                RoomItem::Source => {}
                RoomItem::Container(data) => {
                    if room_level >= data.required_rcl {
                        room.create_construction_site(
                            &RoomPosition::new(loc.x() as u32, loc.y() as u32, room_name),
                            StructureType::Container,
                        );
                    }
                }
                RoomItem::Road => {}
            }
        }
    }

    pub fn visualize(&self, room: &Room) {
        let circle = |x: i32, y: i32, fill: &str, opacity: f32| {
            js! { @{room.as_ref()}.visual.circle(@{x}, @{y}, { fill: @{fill}, opacity: @{opacity} }); }
        };

        for (loc, entry) in self.state.iter() {
            match entry {
                RoomItem::Empty => {}
                RoomItem::Terrain => {
                    circle(loc.x() as i32, loc.y() as i32, "grey", 0.5);
                }
                RoomItem::Source => {
                    circle(loc.x() as i32, loc.y() as i32, "green", 1.0);
                }
                RoomItem::Container(_) => {
                    circle(loc.x() as i32, loc.y() as i32, "blue", 1.0);
                }
                RoomItem::Road => {
                    circle(loc.x() as i32, loc.y() as i32, "yellow", 1.0);
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

        //Self::add_terrain(&self.room, &mut state);
        Self::add_sources(&self.room, &mut state);
        Self::add_containers(&self.room, &mut state);

        Plan { state }
    }

    /*
    fn add_terrain(room: &Room, state: &mut PlanState) {
        let terrain = game::map::get_room_terrain(room.name());

        let raw_terrain = terrain.get_raw_buffer();

        for y in 0..50 {
            for x in 0..50 {
                let entry = raw_terrain[(y as usize) * 50 + (x as usize)];

                let wall = entry & TERRAIN_MASK_WALL;

                if wall != 0 {
                    state.insert(Location::from_coords(x, y), RoomItem::Terrain);
                }
            }
        }
    }
    */

    fn add_sources(room: &Room, state: &mut PlanState) {
        let sources = room.find(find::SOURCES);

        for source in sources {
            let pos = source.pos();

            state.insert(Location::from_coords(pos.x(), pos.y()), RoomItem::Source);
        }
    }

    fn add_containers(room: &Room, state: &mut PlanState) {
        let spawns = room.find(find::MY_SPAWNS);

        for source in room.find(find::SOURCES) {
            let nearest_spawn_path = spawns.iter().cloned().find_nearest_path_to(
                source.pos(),
                PathFinderHelpers::same_room_ignore_creeps_and_structures,
            );

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = (last_step.x as i32) - last_step.dx;
                    let pos_y = (last_step.y as i32) - last_step.dy;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem::Container(RoomItemData { required_rcl: 2 }),
                    );
                }
            }
        }

        if let Some(controller) = room.controller() {
            let nearest_spawn_path = spawns.iter().cloned().find_nearest_path_to(
                controller.pos(),
                PathFinderHelpers::same_room_ignore_creeps_and_structures,
            );

            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = (last_step.x as i32) - last_step.dx;
                    let pos_y = (last_step.y as i32) - last_step.dy;

                    state.insert(
                        Location::from_coords(pos_x as u32, pos_y as u32),
                        RoomItem::Container(RoomItemData { required_rcl: 2 }),
                    );
                }
            }
        }
    }
}
