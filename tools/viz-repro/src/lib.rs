use wasm_bindgen::prelude::*;
use screeps::local::{Position, RoomCoordinate, RoomName};
use screeps::objects::{CircleStyle, LineStyle, PolyStyle, RectStyle, TextStyle, Visual};
use screeps::{MapTextStyle, MapVisualShape};

fn pos(x: u8, y: u8) -> Position {
    let c = |v| RoomCoordinate::new(v).unwrap();
    Position::new(c(x), c(y), "W5N5".parse::<RoomName>().unwrap())
}

#[wasm_bindgen]
pub fn map_circle() -> JsValue {
    let shape = MapVisualShape::circle(pos(25, 25), CircleStyle::default().fill("#1f6feb").radius(8.0).opacity(0.35));
    serde_wasm_bindgen::to_value(&shape).unwrap()
}

#[wasm_bindgen]
pub fn map_text() -> JsValue {
    let shape = MapVisualShape::text(pos(25, 25), "?".to_string(), MapTextStyle::default().color("#c4b5fd").font_size(10.0).opacity(0.8));
    serde_wasm_bindgen::to_value(&shape).unwrap()
}

#[wasm_bindgen]
pub fn map_line() -> JsValue {
    let shape = MapVisualShape::line(pos(10, 10), pos(40, 40), LineStyle::default().width(1.0).color("#3fb950"));
    serde_wasm_bindgen::to_value(&shape).unwrap()
}

#[wasm_bindgen]
pub fn map_poly() -> JsValue {
    let pts = vec![(&pos(1,1)).into(), (&pos(2,2)).into()];
    let shape = MapVisualShape::poly(pts, PolyStyle::default().stroke("#fff"));
    serde_wasm_bindgen::to_value(&shape).unwrap()
}

#[wasm_bindgen]
pub fn map_rect() -> JsValue {
    let shape = MapVisualShape::rect(pos(5, 5), 10, 10, RectStyle::default().fill("#ff0000"));
    serde_wasm_bindgen::to_value(&shape).unwrap()
}

#[wasm_bindgen]
pub fn room_circle() -> JsValue {
    let v = Visual::circle(25.0, 25.0, Some(CircleStyle::default().fill("#fff")));
    serde_wasm_bindgen::to_value(&v).unwrap()
}

#[wasm_bindgen]
pub fn room_text_nan() -> JsValue {
    // NaN in a style field slips past the bot's coords_ok guard
    let v = Visual::text(10.0, 10.0, "hi".into(), Some(TextStyle::default().font(f32::NAN)));
    serde_wasm_bindgen::to_value(&v).unwrap()
}

#[wasm_bindgen]
pub fn room_poly() -> JsValue {
    let v = Visual::poly(vec![(1.0, 1.0), (2.0, 2.0)], Some(PolyStyle::default()));
    serde_wasm_bindgen::to_value(&v).unwrap()
}
