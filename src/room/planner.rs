use screeps::*;
use serde::*;
use crate::findnearest::*;

#[derive(Copy, Clone, Serialize, Deserialize)]
struct RoomItemData {
    required_rcl: u32
}


#[derive(Copy, Clone, Serialize, Deserialize)]
enum RoomItem {
    Empty,
    Terrain,
    Source,
    Container(RoomItemData),
    Road
}

type PlanState = [[RoomItem; 50]; 50];

pub struct Plan {
    room: RoomName,
    state: PlanState,
}

impl Plan {
    pub fn execute(&self) {
        if let Some(room) = game::rooms::get(self.room) {
            let room_name = room.name();
            let room_level = room.controller().map(|c| c.level()).unwrap_or(0);

            for x in 0..50 {
                for y in 0..50 {
                    let entry = self.get_entry(x, y);

                    match entry {
                        RoomItem::Empty => {},
                        RoomItem::Terrain => {},
                        RoomItem::Source => {},
                        RoomItem::Container(data) => {
                            if room_level >= data.required_rcl {
                                room.create_construction_site(&RoomPosition::new(x as u32, y as u32, room_name), StructureType::Container);
                            }
                        },
                        RoomItem::Road => {}
                    }
                } 
            }
        }
    }

    pub fn visualize(&self) {
        let room = game::rooms::get(self.room);

        let circle = |x: i32, y: i32, fill: &str, opacity: f32| { js! { @{room.as_ref()}.visual.circle(@{x}, @{y}, { fill: @{fill}, opacity: @{opacity} }); } };

        for x in 0..50 {
            for y in 0..50 {
                let entry = self.get_entry(x, y);

                match entry {
                    RoomItem::Empty => {},
                    RoomItem::Terrain => {
                        circle(x as i32, y as i32, "grey", 0.5);
                    },
                    RoomItem::Source => {
                        circle(x as i32, y as i32, "green", 1.0);
                    },
                    RoomItem::Container(_) => {
                        circle(x as i32, y as i32, "blue", 1.0);
                    },
                    RoomItem::Road => {}
                }
            } 
        }
    }

    fn get_entry(&self, x: usize, y: usize) -> &RoomItem {
        &self.state[x][y]
    }
}

pub struct Planner<'a> {
    room: &'a Room
}

impl<'a> Planner<'a> {
    pub fn new(room: &Room) -> Planner {
        Planner {
            room
        }
    }

    pub fn plan(&self) -> Plan {
        let mut state = [[RoomItem::Empty; 50]; 50];

        Self::add_terrain(&self.room, &mut state);
        Self::add_sources(&self.room, &mut state);
        Self::add_containers(&self.room, &mut state);

        Plan {
            room: self.room.name(),
            state
        }
    }

    fn add_terrain(room: &Room, state: &mut PlanState) {
        let terrain = game::map::get_room_terrain(room.name()); 

        let raw_terrain = terrain.get_raw_buffer();

        for y in 0..50 {
            for x in 0..50 {
                let entry = raw_terrain[y * 50 + x];

                let wall = entry & TERRAIN_MASK_WALL;

                if wall != 0 {
                    state[x][y] = RoomItem::Terrain
                }
            }
        }
    }

    fn add_sources(room: &Room, state: &mut PlanState) {
        let sources = room.find(find::SOURCES);

        for source in sources {
            let pos = source.pos();

            state[pos.x() as usize][pos.y() as usize] = RoomItem::Source;
        }
    }

    fn add_containers(room: &Room, state: &mut PlanState) {
        let spawns = room.find(find::MY_SPAWNS);
        let sources = room.find(find::SOURCES);

        for source in sources {
            let nearest_spawn_path = spawns
                .iter()
                .cloned()
                .find_nearest_path_to(source.pos(), PathFinderHelpers::same_room_ignore_creeps_and_structures);
            
            if let Some(Path::Vectorized(path)) = nearest_spawn_path {
                if let Some(last_step) = path.last() {
                    let pos_x = (last_step.x as i32) - last_step.dx;
                    let pos_y = (last_step.y as i32) - last_step.dy;

                    state[pos_x as usize][pos_y as usize] = RoomItem::Container(RoomItemData{ required_rcl: 2 });
                }
            }
        }
    }
}