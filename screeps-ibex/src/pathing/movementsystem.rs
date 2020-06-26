use crate::creep::*;
use crate::entitymappingsystem::*;
use crate::room::data::*;
use screeps::*;
use screeps_rover::*;
use serde::*;
use specs::prelude::*;
use specs::*;

#[derive(Component, Serialize, Deserialize)]
pub struct CreepMovementData {
    data: Option<CachedMovementData>,
}

#[derive(SystemData)]
pub struct MovementUpdateSystemData<'a> {
    entities: Entities<'a>,
    movement: WriteExpect<'a, MovementData<Entity>>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
    cost_matrix: WriteExpect<'a, CostMatrixSystem>,
}

struct MovementSystemExternalProvider<'a, 'b> {
    entities: &'b Entities<'a>,
    creep_owner: &'b ReadStorage<'a, CreepOwner>,
    room_data: &'b ReadStorage<'a, RoomData>,
    mapping: &'b Read<'a, EntityMappingData>,
}

impl<'a, 'b> MovementSystemExternal<Entity> for MovementSystemExternalProvider<'a, 'b> {
    fn get_creep(&self, entity: Entity) -> Result<Creep, MovementError> {
        let creep_owner = self.creep_owner.get(entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        Ok(creep)
    }

    fn get_room_cost(
        &self,
        from_room_name: RoomName,
        to_room_name: RoomName,
        room_options: &RoomOptions,
    ) -> Option<f64> {
        if !can_traverse_between_rooms(from_room_name, to_room_name) {
            return Some(f64::INFINITY);
        }

        let target_room_entity = self.mapping.get_room(&to_room_name)?;
        let target_room_data = self.room_data.get(target_room_entity)?;

        if let Some(dynamic_visibility_data) = target_room_data.get_dynamic_visibility_data() {
            if !room_options.allow_hostile() {
                if dynamic_visibility_data.source_keeper() || dynamic_visibility_data.owner().hostile() {
                    return Some(f64::INFINITY);
                }

                if dynamic_visibility_data.updated_within(2000) {
                    if dynamic_visibility_data.hostile_creeps() {
                        return Some(f64::INFINITY);
                    }
                }

                if dynamic_visibility_data.updated_within(10000) && dynamic_visibility_data.hostile_structures() {
                    return Some(f64::INFINITY);                    
                }
            }

            if dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly() {
                Some(3.0)
            } else if dynamic_visibility_data.reservation().mine() || dynamic_visibility_data.reservation().friendly() {
                Some(2.0)
            } else {
                Some(1.0)
            }
        } else {
            Some(2.0)
        }
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
            room_data: &data.room_data,
            mapping: &data.mapping,
        };

        let mut system = MovementSystem::new(&mut *data.cost_matrix);

        if crate::features::pathing::visualize() {
            system.set_default_visualization_style(PolyStyle::default());
        }

        system.set_reuse_path_length(10);

        if crate::features::pathing::custom() {
            system.process(&mut external, movement_data);
        } else {
            system.process_inbuilt(&mut external, movement_data);
        }
    }
}
