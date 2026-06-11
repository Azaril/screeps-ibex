use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

/// Per-target visual cap (P1.C6 / IBEX-008): the server enforces a
/// ~500 KB serialized-visual limit per target and THROWS past it —
/// pre-containment that abort skipped `serialize_world` (Field Report
/// H). ~100 bytes/visual ⇒ 4000 stays safely under the limit; overflow
/// drops-with-telemetry instead of throwing.
pub const MAX_VISUALS_PER_TARGET: usize = 4_000;

/// All coordinates finite? Non-finite values corrupt the whole visual
/// payload for the target (IBEX-008's "renderer corrupts all
/// rendering" mode) — droppable at push time.
fn coords_ok(coords: &[f32]) -> bool {
    coords.iter().all(|c| c.is_finite())
}

pub struct RoomVisualizer {
    visuals: Vec<Visual>,
    dropped_non_finite: u32,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl RoomVisualizer {
    pub fn new() -> RoomVisualizer {
        RoomVisualizer {
            visuals: vec![],
            dropped_non_finite: 0,
        }
    }

    pub fn clear(&mut self) {
        self.visuals.clear();
        self.dropped_non_finite = 0;
    }

    pub fn circle(&mut self, x: f32, y: f32, style: Option<CircleStyle>) {
        if !coords_ok(&[x, y]) {
            self.dropped_non_finite += 1;
            return;
        }
        self.visuals.push(Visual::circle(x, y, style));
    }

    pub fn line(&mut self, from: (f32, f32), to: (f32, f32), style: Option<LineStyle>) {
        if !coords_ok(&[from.0, from.1, to.0, to.1]) {
            self.dropped_non_finite += 1;
            return;
        }
        self.visuals.push(Visual::line(from, to, style));
    }

    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, style: Option<RectStyle>) {
        if !coords_ok(&[x, y, width, height]) {
            self.dropped_non_finite += 1;
            return;
        }
        self.visuals.push(Visual::rect(x, y, width, height, style));
    }

    pub fn poly(&mut self, points: Vec<(f32, f32)>, style: Option<PolyStyle>) {
        if !points.iter().all(|(x, y)| coords_ok(&[*x, *y])) {
            self.dropped_non_finite += 1;
            return;
        }
        self.visuals.push(Visual::poly(points, style));
    }

    pub fn text(&mut self, x: f32, y: f32, text: String, style: Option<TextStyle>) {
        if !coords_ok(&[x, y]) {
            self.dropped_non_finite += 1;
            return;
        }
        self.visuals.push(Visual::text(x, y, text, style));
    }

    pub fn apply(&self, room_name: Option<RoomName>) {
        if self.dropped_non_finite > 0 {
            log::warn!(
                "visuals: dropped {} non-finite visual(s) for {:?} (IBEX-008 clamp)",
                self.dropped_non_finite,
                room_name
            );
        }
        let visuals = if self.visuals.len() > MAX_VISUALS_PER_TARGET {
            log::warn!(
                "visuals: {} exceeds the per-target cap {}; truncating (IBEX-008 size guard)",
                self.visuals.len(),
                MAX_VISUALS_PER_TARGET
            );
            &self.visuals[..MAX_VISUALS_PER_TARGET]
        } else {
            &self.visuals[..]
        };
        screeps::RoomVisual::new(room_name).draw_multi(visuals);
    }
}

impl screeps_visual::render::VisualBackend for RoomVisualizer {
    fn circle(&mut self, x: f32, y: f32, radius: f32, fill: Option<&str>, stroke: Option<&str>, stroke_width: f32, opacity: f32) {
        let mut style = CircleStyle::default().radius(radius).opacity(opacity);
        if let Some(f) = fill {
            style = style.fill(f);
        }
        if let Some(s) = stroke {
            style = style.stroke(s).stroke_width(stroke_width);
        }
        // Route through the clamped push (P1.C6) — the backend trait
        // must not bypass the finiteness guard.
        self.circle(x, y, Some(style));
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Option<&str>, stroke: Option<&str>, stroke_width: f32, opacity: f32) {
        let mut style = RectStyle::default().opacity(opacity);
        if let Some(f) = fill {
            style = style.fill(f);
        }
        if let Some(s) = stroke {
            style = style.stroke(s).stroke_width(stroke_width);
        }
        self.rect(x, y, w, h, Some(style));
    }

    fn poly(&mut self, points: &[(f32, f32)], fill: Option<&str>, stroke: Option<&str>, stroke_width: f32, opacity: f32) {
        let mut style = PolyStyle::default().opacity(opacity);
        if let Some(f) = fill {
            style = style.fill(f);
        }
        if let Some(s) = stroke {
            style = style.stroke(s).stroke_width(stroke_width);
        }
        self.poly(points.to_vec(), Some(style));
    }

    fn line(&mut self, from: (f32, f32), to: (f32, f32), color: Option<&str>, width: f32, opacity: f32) {
        let mut style = LineStyle::default().width(width).opacity(opacity);
        if let Some(c) = color {
            style = style.color(c);
        }
        RoomVisualizer::line(self, from, to, Some(style));
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

            visualizer.global.clear();

            for (room, room_visualizer) in &visualizer.rooms {
                room_visualizer.apply(Some(*room));
            }

            visualizer.rooms.clear();
        }
    }
}

#[cfg(test)]
mod visual_guard_tests {
    use super::coords_ok;

    /// P1.C6 / IBEX-008: non-finite coordinates are droppable at push
    /// time — one NaN visual corrupts the whole target's payload.
    #[test]
    fn coord_finiteness_clamp() {
        assert!(coords_ok(&[1.0, 2.5, 49.0]));
        assert!(!coords_ok(&[f32::NAN, 1.0]));
        assert!(!coords_ok(&[1.0, f32::INFINITY]));
        assert!(!coords_ok(&[f32::NEG_INFINITY]));
        assert!(coords_ok(&[]));
    }
}
