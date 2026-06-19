//! SVG replay scrubber for a [`CombatRecording`] (P2.H4).
//!
//! Renders the captured per-tick frames as a horizontal **filmstrip** of mini-maps — write the
//! string to a `.svg` and scrub left→right through ticks to see the engagement unfold (creeps as
//! owner-coloured circles whose radius tracks HP; structures as squares). The minimal text scrubber
//! is the engine's [`CombatRecording::render`]; the aggregated scoring + sim-vs-server parity is the
//! H5 `screeps-combat-eval` policy layer. (Towers aren't in the frame model, so they don't draw;
//! creeps + structures do.)

use screeps_combat_engine::{CombatRecording, PlayerId};
use std::fmt::Write;

const CELL: u32 = 4; // px per room tile
const ROOM: u32 = 50; // tiles per side
const PAD: u32 = 8;
const GAP: u32 = 10; // px between frames
const LABEL_H: u32 = 12;

fn owner_fill(owner: PlayerId) -> &'static str {
    match owner {
        0 => "#3b82f6", // us — blue
        _ => "#ef4444", // foe — red
    }
}

/// Render the recording as an SVG filmstrip (one mini-map per tick, left→right). Deterministic —
/// the recording's frames + rows are already id-sorted, so output never depends on map iteration.
pub fn to_svg(rec: &CombatRecording) -> String {
    let frame_w = ROOM * CELL;
    let frame_h = ROOM * CELL;
    let n = rec.frames.len().max(1) as u32;
    let total_w = PAD + n * (frame_w + GAP);
    let total_h = PAD + LABEL_H + frame_h + PAD;

    let mut s = String::new();
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{total_w}\" height=\"{total_h}\" \
         viewBox=\"0 0 {total_w} {total_h}\" font-family=\"monospace\" font-size=\"9\">"
    );
    for (i, f) in rec.frames.iter().enumerate() {
        let ox = PAD + i as u32 * (frame_w + GAP);
        let oy = PAD + LABEL_H;
        let _ = write!(s, "<g transform=\"translate({ox},{oy})\">");
        let _ = write!(
            s,
            "<rect x=\"0\" y=\"0\" width=\"{frame_w}\" height=\"{frame_h}\" fill=\"#0b1020\" stroke=\"#334155\"/>"
        );
        let _ = write!(s, "<text x=\"0\" y=\"-3\" fill=\"#cbd5e1\">t{}</text>", f.tick);
        // Structures (spawns/ramparts/walls) as grey tiles.
        for st in &f.structures {
            let x = st.x as u32 * CELL;
            let y = st.y as u32 * CELL;
            let _ = write!(s, "<rect x=\"{x}\" y=\"{y}\" width=\"{CELL}\" height=\"{CELL}\" fill=\"#94a3b8\"/>");
        }
        // Creeps as circles: radius ∝ HP fraction, colour by owner.
        for c in &f.creeps {
            let cx = c.x as u32 * CELL + CELL / 2;
            let cy = c.y as u32 * CELL + CELL / 2;
            let frac = if c.hits_max > 0 { c.hits as f64 / c.hits_max as f64 } else { 0.0 };
            let r = (1.0 + frac * CELL as f64).round() as u32;
            let _ = write!(s, "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"{}\"/>", owner_fill(c.owner));
        }
        let _ = write!(s, "</g>");
    }
    let _ = write!(s, "</svg>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{Part, Position, RoomCoordinate};
    use screeps_combat_engine::{record_tick, CombatAction, CombatRecording, CombatWorld, Intents, SimBody, SimCreep};

    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), "W1N1".parse().unwrap())
    }

    #[test]
    fn renders_a_recording_as_an_svg_filmstrip() {
        // Capture a tiny 3-tick engagement (a ranged attacker chipping a target) and render it.
        let mut world = CombatWorld {
            creeps: vec![
                SimCreep { id: 1, owner: 0, pos: pos(24, 25), body: SimBody::unboosted(&[Part::RangedAttack, Part::RangedAttack]), fatigue: 0 },
                SimCreep { id: 2, owner: 1, pos: pos(25, 25), body: SimBody::unboosted(&[Part::Move]), fatigue: 0 },
            ],
            ..Default::default()
        };
        let mut rec = CombatRecording::new();
        for _ in 0..3 {
            let mut i = Intents::new();
            i.set(1, vec![CombatAction::RangedAttack(2)]);
            record_tick(&mut rec, &mut world, &i);
        }
        let svg = to_svg(&rec);
        assert!(svg.starts_with("<svg"), "is an svg document");
        assert!(svg.ends_with("</svg>"));
        assert_eq!(svg.matches("<g transform").count(), 3, "one frame group per recorded tick");
        assert!(svg.contains("<circle"), "creeps render as circles");
        assert!(svg.contains("#3b82f6") && svg.contains("#ef4444"), "both owners coloured");
        assert!(svg.contains(">t0<") && svg.contains(">t2<"), "tick labels present");
    }

    #[test]
    fn empty_recording_is_still_valid_svg() {
        let svg = to_svg(&CombatRecording::new());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
    }
}
