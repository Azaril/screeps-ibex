use crate::visualize::*;
use screeps::*;
use std::collections::hash_map::*;
use std::collections::HashMap;

pub const SPAWN_QUEUE_POS: (f32, f32) = (35.0, 5.0);
pub const JOBS_POS: (f32, f32) = (35.0, 25.0);
pub const OPERATIONS_POS: (f32, f32) = (5.0, 5.0);
pub const MISSION_POS: (f32, f32) = (5.0, 25.0);
pub const ROOM_DATA_POS: (f32, f32) = (25.0, 5.0);

pub struct RoomUI<'a> {
    room_state: &'a mut RoomUIState,
    room_visualizer: &'a mut RoomVisualizer,
}

impl<'a> RoomUI<'a> {
    pub fn visualizer(&mut self) -> &mut RoomVisualizer {
        self.room_visualizer
    }

    pub fn missions(&mut self) -> ListVisualizer {
        self.room_state.missions.visualize(&mut self.room_visualizer)
    }

    pub fn spawn_queue(&mut self) -> ListVisualizer {
        self.room_state.spawn_queue.visualize(&mut self.room_visualizer)
    }

    pub fn jobs(&mut self) -> ListVisualizer {
        self.room_state.jobs.visualize(&mut self.room_visualizer)
    }
}

pub struct RoomUIState {
    missions: ListVisualizerState,
    spawn_queue: ListVisualizerState,
    jobs: ListVisualizerState,
}

impl RoomUIState {
    pub fn new() -> RoomUIState {
        let missions_text_style = TextStyle::default().font(0.5).align(TextAlign::Left);
        let spawn_queue_text_style = TextStyle::default().font(0.5).align(TextAlign::Left);
        let jobs_text_style = TextStyle::default().font(0.5).align(TextAlign::Left);

        RoomUIState {
            missions: ListVisualizerState::new(MISSION_POS, (0.0, 1.0), Some(missions_text_style)),
            spawn_queue: ListVisualizerState::new(SPAWN_QUEUE_POS, (0.0, 1.0), Some(spawn_queue_text_style)),
            jobs: ListVisualizerState::new(JOBS_POS, (0.0, 1.0), Some(jobs_text_style)),
        }
    }
}

pub struct GlobalUI<'a> {
    global_state: &'a mut GlobalUIState,
    global_visualizer: &'a mut RoomVisualizer,
}

impl<'a> GlobalUI<'a> {
    pub fn visualizer(&mut self) -> &mut RoomVisualizer {
        self.global_visualizer
    }

    pub fn operations(&mut self) -> ListVisualizer {
        self.global_state.operations.visualize(&mut self.global_visualizer)
    }
}

pub struct GlobalUIState {
    operations: ListVisualizerState,
}

impl GlobalUIState {
    pub fn new() -> GlobalUIState {
        let opereations_text_style = TextStyle::default().font(0.5).align(TextAlign::Left);
        GlobalUIState {
            operations: ListVisualizerState::new(OPERATIONS_POS, (0.0, 1.0), Some(opereations_text_style)),
        }
    }
}

pub struct UISystem {
    global_state: Option<GlobalUIState>,
    room_states: HashMap<RoomName, RoomUIState>,
}

impl Default for UISystem {
    fn default() -> UISystem {
        UISystem::new()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl UISystem {
    pub fn new() -> UISystem {
        UISystem {
            global_state: None,
            room_states: HashMap::new(),
        }
    }

    pub fn with_global<T>(&mut self, visualizer: &mut Visualizer, callback: T)
    where
        T: Fn(&mut GlobalUI),
    {
        let mut global_visualizer = visualizer.global();
        let global_initialized = self.global_state.is_some();

        if !global_initialized {
            self.global_state = Some(GlobalUIState::new())
        }

        let mut global_ui = GlobalUI {
            global_state: &mut self.global_state.as_mut().unwrap(),
            global_visualizer: &mut global_visualizer,
        };

        if !global_initialized {
            Self::initialize_global(&mut global_ui);
        }

        callback(&mut global_ui);
    }

    pub fn with_room<T>(&mut self, room: RoomName, visualizer: &mut Visualizer, callback: T)
    where
        T: FnOnce(&mut RoomUI),
    {
        let mut room_visualizer = visualizer.get_room(room);
        let room_state_entry = self.room_states.entry(room);
        let room_initialized = match &room_state_entry {
            Entry::Occupied(_) => true,
            Entry::Vacant(_) => false,
        };

        let mut room_state = room_state_entry.or_insert_with(RoomUIState::new);

        let mut room_ui = RoomUI {
            room_state: &mut room_state,
            room_visualizer: &mut room_visualizer,
        };

        if !room_initialized {
            Self::initialize_room(room, &mut room_ui);
        }

        callback(&mut room_ui);
    }

    fn initialize_global(global_ui: &mut GlobalUI) {
        global_ui.operations().add_text("Operations".to_string(), None);
    }

    fn initialize_room(room_name: RoomName, room_ui: &mut RoomUI) {
        room_ui.missions().add_text("Missions".to_string(), None);

        room_ui.jobs().add_text("Jobs".to_string(), None);

        if let Some(room) = game::rooms::get(room_name) {
            room_ui.spawn_queue().add_text(
                format!(
                    "Spawn Queue - Energy {} / {}",
                    room.energy_available(),
                    room.energy_capacity_available()
                ),
                None,
            );
        } else {
            room_ui.spawn_queue().add_text("Spawn Queue - Energy ? / ?".to_string(), None);
        }
    }
}
