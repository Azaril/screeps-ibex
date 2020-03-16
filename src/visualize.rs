use screeps::*;
use serde::*;
use specs::prelude::*;
use std::collections::HashMap;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CircleStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opacity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke_width: Option<f32>,
}

impl CircleStyle {
    pub fn radius(mut self, val: f32) -> CircleStyle {
        self.radius = Some(val);
        self
    }

    pub fn fill(mut self, val: &str) -> CircleStyle {
        self.fill = Some(val.to_string());
        self
    }

    pub fn opacity(mut self, val: f32) -> CircleStyle {
        self.opacity = Some(val);
        self
    }

    pub fn stroke(mut self, val: &str) -> CircleStyle {
        self.stroke = Some(val.to_string());
        self
    }

    pub fn stroke_width(mut self, val: f32) -> CircleStyle {
        self.stroke_width = Some(val);
        self
    }
}

#[derive(Serialize)]
pub struct CircleData {
    x: f32,
    y: f32,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    style: Option<CircleStyle>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LineDrawStyle {
    Solid,
    Dashed,
    Dotted,
}

impl Default for LineDrawStyle {
    fn default() -> LineDrawStyle {
        LineDrawStyle::Solid
    }
}

impl LineDrawStyle {
    pub fn is_solid(&self) -> bool {
        match self {
            LineDrawStyle::Solid => true,
            _ => false,
        }
    }
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LineStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opacity: Option<f32>,
    #[serde(skip_serializing_if = "LineDrawStyle::is_solid")]
    line_style: LineDrawStyle,
}

impl LineStyle {
    pub fn width(mut self, val: f32) -> LineStyle {
        self.width = Some(val);
        self
    }

    pub fn color(mut self, val: &str) -> LineStyle {
        self.color = Some(val.to_string());
        self
    }

    pub fn opacity(mut self, val: f32) -> LineStyle {
        self.opacity = Some(val);
        self
    }

    pub fn line_style(mut self, val: LineDrawStyle) -> LineStyle {
        self.line_style = val;
        self
    }
}

#[derive(Serialize)]
pub struct LineData {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    style: Option<LineStyle>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RectStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opacity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke_width: Option<f32>,
    #[serde(skip_serializing_if = "LineDrawStyle::is_solid")]
    line_style: LineDrawStyle,
}

impl RectStyle {
    pub fn fill(mut self, val: &str) -> RectStyle {
        self.fill = Some(val.to_string());
        self
    }

    pub fn opacity(mut self, val: f32) -> RectStyle {
        self.opacity = Some(val);
        self
    }

    pub fn stroke(mut self, val: &str) -> RectStyle {
        self.stroke = Some(val.to_string());
        self
    }

    pub fn stroke_width(mut self, val: f32) -> RectStyle {
        self.stroke_width = Some(val);
        self
    }

    pub fn line_style(mut self, val: LineDrawStyle) -> RectStyle {
        self.line_style = val;
        self
    }
}

#[derive(Serialize)]
pub struct RectData {
    x: f32,
    y: f32,
    #[serde(rename = "w")]
    width: f32,
    #[serde(rename = "h")]
    height: f32,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    style: Option<RectStyle>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PolyStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opacity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke_width: Option<f32>,
    #[serde(skip_serializing_if = "LineDrawStyle::is_solid")]
    line_style: LineDrawStyle,
}

impl PolyStyle {
    pub fn fill(mut self, val: &str) -> PolyStyle {
        self.fill = Some(val.to_string());
        self
    }

    pub fn opacity(mut self, val: f32) -> PolyStyle {
        self.opacity = Some(val);
        self
    }

    pub fn stroke(mut self, val: &str) -> PolyStyle {
        self.stroke = Some(val.to_string());
        self
    }

    pub fn stroke_width(mut self, val: f32) -> PolyStyle {
        self.stroke_width = Some(val);
        self
    }

    pub fn line_style(mut self, val: LineDrawStyle) -> PolyStyle {
        self.line_style = val;
        self
    }
}

#[derive(Serialize)]
pub struct PolyData {
    points: Vec<(f32, f32)>,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    style: Option<PolyStyle>,
}

#[derive(Serialize, Clone)]
#[serde(untagged)]
enum FontStyle {
    Size(f32),
    Custom(String),
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub enum TextAlign {
    Center,
    Left,
    Right,
}

impl Default for TextAlign {
    fn default() -> TextAlign {
        TextAlign::Center
    }
}

impl TextAlign {
    pub fn is_center(&self) -> bool {
        match self {
            TextAlign::Center => true,
            _ => false,
        }
    }
}

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    font: Option<FontStyle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    background_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    background_padding: Option<f32>,
    #[serde(skip_serializing_if = "TextAlign::is_center")]
    align: TextAlign,
    #[serde(skip_serializing_if = "Option::is_none")]
    opacity: Option<f32>,
}

impl TextStyle {
    pub fn color(mut self, val: &str) -> TextStyle {
        self.color = Some(val.to_string());
        self
    }

    pub fn font(mut self, val: f32) -> TextStyle {
        self.font = Some(FontStyle::Size(val));
        self
    }

    pub fn custom_font(mut self, val: &str) -> TextStyle {
        self.font = Some(FontStyle::Custom(val.to_string()));
        self
    }

    pub fn stroke(mut self, val: &str) -> TextStyle {
        self.stroke = Some(val.to_string());
        self
    }

    pub fn stroke_width(mut self, val: f32) -> TextStyle {
        self.opacity = Some(val);
        self
    }

    pub fn background_color(mut self, val: &str) -> TextStyle {
        self.background_color = Some(val.to_string());
        self
    }

    pub fn background_padding(mut self, val: f32) -> TextStyle {
        self.opacity = Some(val);
        self
    }

    pub fn align(mut self, val: TextAlign) -> TextStyle {
        self.align = val;
        self
    }

    pub fn opacity(mut self, val: f32) -> TextStyle {
        self.opacity = Some(val);
        self
    }
}

#[derive(Serialize)]
pub struct TextData {
    text: String,
    x: f32,
    y: f32,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    style: Option<TextStyle>,
}

#[derive(Serialize)]
#[serde(tag = "t")]
enum Visual {
    #[serde(rename = "c")]
    Circle(CircleData),
    #[serde(rename = "l")]
    Line(LineData),
    #[serde(rename = "r")]
    Rect(RectData),
    #[serde(rename = "p")]
    Poly(PolyData),
    #[serde(rename = "t")]
    Text(TextData),
}

pub struct RoomVisualizer {
    visuals: Vec<Visual>,
}

impl RoomVisualizer {
    pub fn new() -> RoomVisualizer {
        RoomVisualizer { visuals: vec![] }
    }

    pub fn circle(&mut self, x: f32, y: f32, style: Option<CircleStyle>) {
        self.visuals.push(Visual::Circle(CircleData { x, y, style }));
    }

    pub fn line(&mut self, from: (f32, f32), to: (f32, f32), style: Option<LineStyle>) {
        self.visuals.push(Visual::Line(LineData {
            x1: from.0,
            y1: from.1,
            x2: to.0,
            y2: to.1,
            style,
        }));
    }

    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, style: Option<RectStyle>) {
        self.visuals.push(Visual::Rect(RectData {
            x,
            y,
            width,
            height,
            style,
        }));
    }

    pub fn poly(&mut self, points: Vec<(f32, f32)>, style: Option<PolyStyle>) {
        self.visuals.push(Visual::Poly(PolyData { points, style }));
    }

    pub fn text(&mut self, x: f32, y: f32, text: String, style: Option<TextStyle>) {
        self.visuals.push(Visual::Text(TextData { x, y, text, style }));
    }

    pub fn apply(&self, room: Option<RoomName>) {
        if !self.visuals.is_empty() {
            let data = serde_json::to_string(&self.visuals).unwrap();

            use stdweb::*;

            //TODO: Really horrible hack here that it's needed to JSON.parse and then pass in to the visual function as
            //      JSON.stringify in console.addVisual prevents direct string concatenation. (Double string escaping happens.)
            js! {
                JSON.parse(@{data}).forEach(function(v) { console.addVisual(@{room}, v); });
            };
        }
    }
}

pub struct Visualizer {
    global: RoomVisualizer,
    rooms: HashMap<RoomName, RoomVisualizer>,
}

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
}
