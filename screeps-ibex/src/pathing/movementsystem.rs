use crate::creep::*;
use crate::entitymappingsystem::*;
use crate::room::data::*;
use crate::visualize::Visualizer;
use screeps::*;
use screeps_rover::screeps_impl::{ScreepsCostMatrixDataSource, ScreepsPathfinder};
use screeps_rover::*;
use serde::*;
use shrinkwraprs::*;
use specs::prelude::*;
use specs::*;

#[derive(Shrinkwrap, Component, Serialize, Deserialize, Clone, Default)]
#[shrinkwrap(mutable)]
#[serde(transparent)]
pub struct CreepRoverData(pub CreepMovementData);

#[derive(SystemData)]
pub struct MovementUpdateSystemData<'a> {
    entities: Entities<'a>,
    movement: WriteExpect<'a, MovementData<Entity>>,
    movement_results: WriteExpect<'a, MovementResults<Entity>>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    creep_movement_data: WriteStorage<'a, CreepRoverData>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
    cost_matrix_cache: WriteExpect<'a, CostMatrixCache>,
    visualizer: Option<Write<'a, Visualizer>>,
}

/// Movement visualizer that pushes intents to the screeps-ibex room
/// visualizer system, which batches and flushes all visuals at end of tick.
struct IbexMovementVisualizer<'a> {
    visualizer: &'a mut Visualizer,
}

impl<'a> MovementVisualizer for IbexMovementVisualizer<'a> {
    fn visualize_path(&mut self, creep_pos: Position, path: &[Position]) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let points: Vec<(f32, f32)> = path.iter().map(|p| (p.x().u8() as f32, p.y().u8() as f32)).collect();
        let style = PolyStyle::default().stroke("blue").stroke_width(0.2).opacity(0.5);
        room_vis.poly(points, Some(style));
    }

    fn visualize_anchor(&mut self, creep_pos: Position, anchor_pos: Position) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;

        let circle_style = CircleStyle::default()
            .fill("#ff8800")
            .radius(0.15)
            .opacity(0.5)
            .stroke("#ff8800")
            .stroke_width(0.02);
        room_vis.circle(cx, cy, Some(circle_style));

        let ax = anchor_pos.x().u8() as f32;
        let ay = anchor_pos.y().u8() as f32;
        if (ax - cx).abs() > 0.01 || (ay - cy).abs() > 0.01 {
            let line_style = LineStyle::default().color("#ff8800").opacity(0.25);
            room_vis.line((cx, cy), (ax, ay), Some(line_style));
        }
    }

    fn visualize_immovable(&mut self, creep_pos: Position) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;
        let d = 0.15;
        let style = LineStyle::default().color("#ff4444").opacity(0.6);
        room_vis.line((cx - d, cy - d), (cx + d, cy + d), Some(style.clone()));
        room_vis.line((cx - d, cy + d), (cx + d, cy - d), Some(style));
    }

    fn visualize_stuck(&mut self, creep_pos: Position, ticks: u16) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;

        let circle_style = CircleStyle::default()
            .fill("#ffcc00")
            .radius(0.2)
            .opacity(0.6)
            .stroke("#ffcc00")
            .stroke_width(0.03);
        room_vis.circle(cx, cy, Some(circle_style));

        let text_style = TextStyle::default().color("#ffcc00").font(0.4).stroke("#000000").stroke_width(0.03);
        room_vis.text(cx, cy + 0.55, format!("{}", ticks), Some(text_style));
    }

    fn visualize_failed(&mut self, creep_pos: Position) {
        let room = creep_pos.room_name();
        let room_vis = self.visualizer.get_room(room);
        let cx = creep_pos.x().u8() as f32;
        let cy = creep_pos.y().u8() as f32;

        let circle_style = CircleStyle::default()
            .fill("#ff0000")
            .radius(0.2)
            .opacity(0.7)
            .stroke("#ff0000")
            .stroke_width(0.03);
        room_vis.circle(cx, cy, Some(circle_style));
    }
}

struct MovementSystemExternalProvider<'a, 'b> {
    entities: &'b Entities<'a>,
    creep_owner: &'b ReadStorage<'a, CreepOwner>,
    creep_movement_data: &'b mut WriteStorage<'a, CreepRoverData>,
    room_data: &'b ReadStorage<'a, RoomData>,
    mapping: &'b Read<'a, EntityMappingData>,
}

impl<'a, 'b> MovementSystemExternal<Entity> for MovementSystemExternalProvider<'a, 'b> {
    type Creep = screeps::Creep;

    fn get_creep(&self, entity: Entity) -> Result<screeps::Creep, MovementError> {
        let creep_owner = self.creep_owner.get(entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        Ok(creep)
    }

    fn get_creep_movement_data(&mut self, entity: Entity) -> Result<&mut CreepMovementData, MovementError> {
        if !self.creep_movement_data.contains(entity) {
            let _ = self.creep_movement_data.insert(entity, CreepRoverData::default());
        }

        self.creep_movement_data
            .get_mut(entity)
            .map(|m| &mut m.0)
            .ok_or("Failed to get creep movement data".to_owned())
    }

    fn get_room_cost(&self, from_room_name: RoomName, to_room_name: RoomName, room_options: &RoomOptions) -> Option<f64> {
        if !can_traverse_between_rooms(from_room_name, to_room_name) {
            return None;
        }

        let dynamic_visibility_data = self
            .mapping
            .get_room(&to_room_name)
            .and_then(|target_room_entity| self.room_data.get(target_room_entity))
            .and_then(|target_room_data| target_room_data.get_dynamic_visibility_data());

        if let Some(dynamic_visibility_data) = dynamic_visibility_data {
            let is_hostile = dynamic_visibility_data.source_keeper()
                || dynamic_visibility_data.owner().hostile()
                || dynamic_visibility_data.reservation().hostile()
                || dynamic_visibility_data.hostile_creeps()
                || dynamic_visibility_data.hostile_structures();

            if is_hostile {
                match room_options.hostile_behavior() {
                    HostileBehavior::Allow => {}
                    HostileBehavior::HighCost => return Some(10.0),
                    HostileBehavior::Deny => return None,
                }
            }

            if dynamic_visibility_data.owner().mine()
                || dynamic_visibility_data.owner().friendly()
                || dynamic_visibility_data.reservation().mine()
                || dynamic_visibility_data.reservation().friendly()
            {
                return Some(1.0);
            } else {
                return Some(2.0);
            }
        }

        Some(2.0)
    }

    fn get_entity_position(&self, entity: Entity) -> Option<Position> {
        let creep_owner = self.creep_owner.get(entity)?;
        let creep = creep_owner.id().resolve()?;
        Some(HasPosition::pos(&creep))
    }
}

pub struct MovementUpdateSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for MovementUpdateSystem {
    type SystemData = MovementUpdateSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let movement_data = std::mem::replace(&mut *data.movement, MovementData::new());

        let mut external = MovementSystemExternalProvider {
            entities: &data.entities,
            creep_owner: &data.creep_owner,
            creep_movement_data: &mut data.creep_movement_data,
            room_data: &data.room_data,
            mapping: &data.mapping,
        };

        let mut pathfinder = ScreepsPathfinder;
        let mut ibex_visualizer = data.visualizer.as_deref_mut().map(|v| IbexMovementVisualizer { visualizer: v });

        let mut cost_matrix_system = CostMatrixSystem::new(&mut data.cost_matrix_cache, Box::new(ScreepsCostMatrixDataSource));

        let mut system = MovementSystem::new(
            &mut cost_matrix_system,
            &mut pathfinder,
            ibex_visualizer.as_mut().map(|v| v as &mut dyn MovementVisualizer),
        );

        let pathing_features = crate::features::features().pathing;
        system.set_reuse_path_length(pathing_features.reuse_path_length);
        system.set_max_shove_depth(pathing_features.max_shove_depth);
        system.set_friendly_creep_distance(pathing_features.friendly_creep_distance);

        let results = system.process(&mut external, movement_data);

        *data.movement_results = results;
    }
}
