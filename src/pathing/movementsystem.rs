use crate::creep::*;
use screeps::*;
use serde::*;
use specs::*;
use specs::prelude::*;
use crate::room::data::*;
use crate::entitymappingsystem::*;
use screeps_rover::*;

#[derive(Component, Serialize, Deserialize)]
pub struct CreepMovementData {
    data: Option<CachedMovementData>,
}

#[derive(SystemData)]
pub struct MovementUpdateSystemData<'a> {
    movement: WriteExpect<'a, MovementData<Entity>>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
    cost_matrix: WriteExpect<'a, CostMatrixSystem>,
}

impl<'a> MovementSystemExternal<Entity> for MovementUpdateSystemData<'a> {
    fn get_creep(&self, entity: Entity) -> Result<Creep, MovementError> {
        let creep_owner = self.creep_owner.get(entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        Ok(creep)
    }

    fn get_room_weight(&self, from_room_name: RoomName, to_room_name: RoomName, current_room_name: RoomName, room_options: &RoomOptions) -> Option<f64> {
        if !can_traverse_between_rooms(from_room_name, to_room_name) {
            return Some(f64::INFINITY);
        }

        let target_room_entity = self.mapping.get_room(&to_room_name)?;
        let target_room_data = self.room_data.get(target_room_entity)?;

        let is_current_room = to_room_name == current_room_name;

        if let Some(dynamic_visibility_data) = target_room_data.get_dynamic_visibility_data() {
            if !is_current_room {
                if !room_options.allow_hostile() {
                    if dynamic_visibility_data.source_keeper() || dynamic_visibility_data.owner().hostile() {
                        return Some(f64::INFINITY);
                    }
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

        MovementSystem::process_inbuilt(&mut data, movement_data);
    }
}
