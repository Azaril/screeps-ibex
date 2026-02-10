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
        screeps::RoomVisual::new(room_name).draw_multi(&self.visuals);
    }
}

impl screeps_visual::render::VisualBackend for RoomVisualizer {
    fn circle(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        fill: Option<&str>,
        stroke: Option<&str>,
        stroke_width: f32,
        opacity: f32,
    ) {
        let mut style = CircleStyle::default().radius(radius).opacity(opacity);
        if let Some(f) = fill {
            style = style.fill(f);
        }
        if let Some(s) = stroke {
            style = style.stroke(s).stroke_width(stroke_width);
        }
        self.visuals.push(Visual::circle(x, y, Some(style)));
    }

    fn rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<&str>,
        stroke: Option<&str>,
        stroke_width: f32,
        opacity: f32,
    ) {
        let mut style = RectStyle::default().opacity(opacity);
        if let Some(f) = fill {
            style = style.fill(f);
        }
        if let Some(s) = stroke {
            style = style.stroke(s).stroke_width(stroke_width);
        }
        self.visuals.push(Visual::rect(x, y, w, h, Some(style)));
    }

    fn poly(
        &mut self,
        points: &[(f32, f32)],
        fill: Option<&str>,
        stroke: Option<&str>,
        stroke_width: f32,
        opacity: f32,
    ) {
        let mut style = PolyStyle::default().opacity(opacity);
        if let Some(f) = fill {
            style = style.fill(f);
        }
        if let Some(s) = stroke {
            style = style.stroke(s).stroke_width(stroke_width);
        }
        self.visuals
            .push(Visual::poly(points.to_vec(), Some(style)));
    }

    fn line(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        color: Option<&str>,
        width: f32,
        opacity: f32,
    ) {
        let mut style = LineStyle::default().width(width).opacity(opacity);
        if let Some(c) = color {
            style = style.color(c);
        }
        self.visuals.push(Visual::line(from, to, Some(style)));
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
pub struct ApplyVisualsSystemData<'a> {
    visualizer: Option<Write<'a, Visualizer>>,
}

/// Flushes the Visualizer resource to the game (e.g. console::add_visual).
/// Named to avoid confusion with "visualization" / RenderSystem.
pub struct ApplyVisualsSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for ApplyVisualsSystem {
    type SystemData = ApplyVisualsSystemData<'a>;

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
