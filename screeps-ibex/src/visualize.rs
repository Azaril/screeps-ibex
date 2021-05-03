use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

pub struct RoomVisualizer {
    visuals: Vec<Visual>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RoomVisualizer {
    pub fn new() -> RoomVisualizer {
        RoomVisualizer { visuals: vec![] }
    }

    pub fn circle(&mut self, x: f32, y: f32, style: Option<CircleStyle>) {
        self.visuals.push(Visual::circle(x, y, style));
    }

    pub fn line(&mut self, from: (f32, f32), to: (f32, f32), style: Option<LineStyle>) {
        self.visuals.push(Visual::line(from, to, style));
    }

    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, style: Option<RectStyle>) {
        self.visuals.push(Visual::rect(x, y, width, height, style));
    }

    pub fn poly(&mut self, points: Vec<(f32, f32)>, style: Option<PolyStyle>) {
        self.visuals.push(Visual::poly(points, style));
    }

    pub fn text(&mut self, x: f32, y: f32, text: String, style: Option<TextStyle>) {
        self.visuals.push(Visual::text(x, y, text, style));
    }

    pub fn apply(&self, room_name: Option<RoomName>) {
        Self::draw_multi_fast(room_name, &self.visuals);
    }

    fn draw_multi_fast(room_name: Option<RoomName>, visuals: &[Visual]) {
        RoomVisual::new(room_name).draw_multi(visuals);
    }
}

pub struct Visualizer {
    global: RoomVisualizer,
    rooms: HashMap<RoomName, RoomVisualizer>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Visualizer {
    pub fn new() -> Visualizer {
        Visualizer {
            global: RoomVisualizer::new(),
            rooms: HashMap::new(),
        }
    }

    pub fn global(&mut self) -> &mut RoomVisualizer {
        &mut self.global
    }

    pub fn get_room(&mut self, room: RoomName) -> &mut RoomVisualizer {
        self.rooms.entry(room).or_insert_with(RoomVisualizer::new)
    }
}

impl Default for Visualizer {
    fn default() -> Visualizer {
        Visualizer::new()
    }
}

#[derive(SystemData)]
pub struct VisualizerSystemData<'a> {
    visualizer: Option<Write<'a, Visualizer>>,
}

pub struct VisualizerSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for VisualizerSystem {
    type SystemData = VisualizerSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if let Some(visualizer) = &mut data.visualizer {
            visualizer.global.apply(None);

            for (room, room_visualizer) in &visualizer.rooms {
                room_visualizer.apply(Some(*room));
            }

            visualizer.rooms.clear();
        }
    }
}

pub struct ListVisualizerState {
    pos: (f32, f32),
    pos_offset: (f32, f32),
    style: Option<TextStyle>,
}

impl ListVisualizerState {
    pub fn visualize<'a>(&mut self, visualizer: &'a mut RoomVisualizer) -> ListVisualizer<'a, '_> {
        ListVisualizer { visualizer, state: self }
    }
}

impl ListVisualizerState {
    pub fn new(pos: (f32, f32), pos_offset: (f32, f32), style: Option<TextStyle>) -> ListVisualizerState {
        ListVisualizerState { pos, pos_offset, style }
    }

    pub fn get_default_style(&self) -> TextStyle {
        self.style.clone().unwrap_or_default()
    }
}

pub struct ListVisualizer<'a, 'b> {
    visualizer: &'a mut RoomVisualizer,
    state: &'b mut ListVisualizerState,
}

impl<'a, 'b> ListVisualizer<'a, 'b> {
    pub fn add_text(&mut self, text: String, style: Option<TextStyle>) {
        let visualizer = &mut self.visualizer;
        let state = &mut self.state;

        visualizer.text(state.pos.0, state.pos.1, text, style.or_else(|| state.style.clone()));

        state.pos.0 += state.pos_offset.0;
        state.pos.1 += state.pos_offset.1;
    }

    pub fn get_default_style(&self) -> TextStyle {
        self.state.get_default_style()
    }
}
