use super::data::*;
use super::operationsystem::*;
use crate::missions::colony::*;
use crate::missions::data::*;
use crate::serialize::*;
use log::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct ColonyOperation {
    owner: EntityOption<Entity>,
    last_run: Option<u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl ColonyOperation {
    pub fn build<B>(builder: B, owner: Option<Entity>) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let operation = ColonyOperation::new(owner);

        builder.with(OperationData::Colony(operation)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>) -> ColonyOperation {
        ColonyOperation {
            owner: owner.into(),
            last_run: None,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Operation for ColonyOperation {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn describe(&mut self, _system_data: &mut OperationExecutionSystemData, describe_data: &mut OperationDescribeData) {
        describe_data.ui.with_global(describe_data.visualizer, |global_ui| {
            global_ui.operations().add_text("Colony".to_string(), None);
        })
    }

    fn run_operation(
        &mut self,
        system_data: &mut OperationExecutionSystemData,
        runtime_data: &mut OperationExecutionRuntimeData,
    ) -> Result<OperationResult, ()> {
        let should_run = self.last_run.map(|t| game::time() - t >= 50).unwrap_or(true);

        if !should_run {
            return Ok(OperationResult::Running);
        }

        self.last_run = Some(game::time());

        for (entity, room_data) in (&*system_data.entities, &mut *system_data.room_data).join() {
            let needs_colony = ColonyMission::can_run(&room_data);

            if needs_colony {
                //
                // Query if any missions running on the room currently fufill the colony role.
                //

                let mission_data = system_data.mission_data;

                //TODO: wiarchbe: Use trait instead of match.
                let has_colony_mission = room_data
                    .get_missions()
                    .iter()
                    .any(|mission_entity| mission_data.get(*mission_entity).as_mission_type::<ColonyMission>().is_some());

                //
                // Spawn a new mission to fill the colony role if missing.
                //

                if !has_colony_mission {
                    info!("Starting colony mission for spawning room. Room: {}", room_data.name);

                    let mission_entity = ColonyMission::build(
                        system_data.updater.create_entity(system_data.entities),
                        Some(runtime_data.entity),
                        entity,
                    )
                    .build();

                    room_data.add_mission(mission_entity);
                }
            }
        }

        Ok(OperationResult::Running)
    }
}
