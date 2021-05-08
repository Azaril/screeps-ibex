use super::data::*;
use super::missionsystem::*;
use crate::room::roomplansystem::*;
use crate::serialize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(ConvertSaveload)]
pub struct ConstructionMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ConstructionMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = ConstructionMission::new(owner, room_data);

        builder
            .with(MissionData::Construction(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> ConstructionMission {
        ConstructionMission {
            owner: owner.into(),
            room_data,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for ConstructionMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe_state(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> String {
        "Construction".to_string()
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms().get(room_data.name).ok_or("Expected room")?;

        let request_plan = if let Some(room_plan_data) = system_data.room_plan_data.get(self.room_data) {
            if let Some(plan) = room_plan_data.plan() {
                if game::time() % 10 == 0 {
                    if crate::features::construction::execute() {
                        let construction_sites = room_data.get_construction_sites().ok_or("Expected construction sites")?;

                        const MAX_CONSTRUCTION_SITES: i32 = 2;

                        let max_placement = MAX_CONSTRUCTION_SITES - (construction_sites.len() as i32);

                        if max_placement > 0 {
                            plan.execute(&room, max_placement as u32);
                        }
                    }

                    if crate::features::construction::cleanup() {
                        let structures = room_data.get_structures().ok_or("Expected structures")?;

                        plan.cleanup(structures.all());
                    }
                }

                false
            } else {
                crate::features::construction::allow_replan()
            }
        } else {
            true
        };

        if request_plan || crate::features::construction::force_plan() {
            system_data.room_plan_queue.request(RoomPlanRequest::new(self.room_data, 1.0));
        }

        Ok(MissionResult::Running)
    }
}
