use crate::creep::*;
use crate::entitymappingsystem::*;
use crate::room::data::*;
use screeps::*;
use screeps_rover::*;
use serde::*;
use specs::prelude::*;
use specs::*;
use shrinkwraprs::*;

#[derive(Shrinkwrap, Component, Serialize, Deserialize, Clone)]
#[derive(Default)]
#[shrinkwrap(mutable)]
#[serde(transparent)]
pub struct CreepRoverData(pub CreepMovementData);

#[derive(SystemData)]
pub struct MovementUpdateSystemData<'a> {
    entities: Entities<'a>,
    movement: WriteExpect<'a, MovementData<Entity>>,
    creep_owner: ReadStorage<'a, CreepOwner>,
    creep_movement_data: WriteStorage<'a, CreepRoverData>,
    room_data: ReadStorage<'a, RoomData>,
    mapping: Read<'a, EntityMappingData>,
    cost_matrix: WriteExpect<'a, CostMatrixSystem>,
}

struct MovementSystemExternalProvider<'a, 'b> {
    entities: &'b Entities<'a>,
    creep_owner: &'b ReadStorage<'a, CreepOwner>,
    creep_movement_data: &'b mut WriteStorage<'a, CreepRoverData>,
    room_data: &'b ReadStorage<'a, RoomData>,
    mapping: &'b Read<'a, EntityMappingData>,
}

impl<'a, 'b> MovementSystemExternal<Entity> for MovementSystemExternalProvider<'a, 'b> {
    fn get_creep(&self, entity: Entity) -> Result<Creep, MovementError> {
        let creep_owner = self.creep_owner.get(entity).ok_or("Expected creep owner")?;
        let creep = creep_owner.id().resolve().ok_or("Expected creep")?;

        Ok(creep)
    }

    fn get_creep_movement_data(&mut self, entity: Entity) -> Result<&mut CreepMovementData, MovementError> {
        if !self.creep_movement_data.contains(entity) {
            let _ = self.creep_movement_data.insert(entity, CreepRoverData::default());
        }

        self.creep_movement_data.get_mut(entity).map(|m| &mut m.0).ok_or("Failed to get creep movement data".to_owned())
    }

    fn get_room_cost(
        &self,
        from_room_name: RoomName,
        to_room_name: RoomName,
        room_options: &RoomOptions,
    ) -> Option<f64> {
        if !can_traverse_between_rooms(from_room_name, to_room_name) {
            return None;
        }

        let target_room_entity = self.mapping.get_room(&to_room_name)?;
        let target_room_data = self.room_data.get(target_room_entity)?;

        if let Some(dynamic_visibility_data) = target_room_data.get_dynamic_visibility_data() {
            let is_hostile = dynamic_visibility_data.source_keeper() || 
                dynamic_visibility_data.owner().hostile() || 
                dynamic_visibility_data.reservation().hostile() ||
                dynamic_visibility_data.hostile_creeps() ||
                dynamic_visibility_data.hostile_structures();

            if is_hostile {
                match room_options.hostile_behavior() {
                    HostileBehavior::Allow => {},
                    HostileBehavior::HighCost => return Some(10.0),
                    HostileBehavior::Deny => return None,
                }
            }

            if dynamic_visibility_data.owner().mine() || dynamic_visibility_data.owner().friendly() {
                Some(1.0)
            } else if dynamic_visibility_data.reservation().mine() || dynamic_visibility_data.reservation().friendly() {
                Some(1.0)
            } else {
                Some(2.0)
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
            creep_movement_data: &mut data.creep_movement_data,
            room_data: &data.room_data,
            mapping: &data.mapping,
        };

        let mut system = MovementSystem::new(&mut *data.cost_matrix);

        if crate::features::pathing::visualize() {
            system.set_default_visualization_style(PolyStyle::default());
        }

        system.set_reuse_path_length(5);

        if crate::features::pathing::custom() {
            system.process(&mut external, movement_data);
        } else {
            system.process_inbuilt(&mut external, movement_data);
        }
    }
}
