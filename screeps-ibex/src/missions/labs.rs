use super::data::*;
use super::missionsystem::*;
use crate::serialize::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use crate::jobs::utility::waitbehavior::*;
use crate::room::data::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use std::collections::HashMap;
use log::*;

#[derive(Clone, ConvertSaveload)]
pub struct LabsMissionContext {
    room_data: Entity,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum LabsState {
        Idle {
            phantom: std::marker::PhantomData<Entity>
        },
        Wait {
            ticks: u32
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &LabsMissionContext) -> String {
            format!("Labs - {}", self.status_description())
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        * => fn gather_data(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut LabsMissionContext) -> Result<Option<LabsState>, String>;
    }
);

impl Idle {
    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut LabsMissionContext,
    ) -> Result<Option<LabsState>, String> {
        if let Some((resource_type, amount)) = Self::get_target_reaction(system_data, state_context)? {
            let room_data = system_data
                .room_data
                .get(state_context.room_data)
                .ok_or("Expected room data")?;
                
            info!("Selected reaction - Room: {} Resource: {:?} - Amount: {}", room_data.name, resource_type, amount);

            Ok(Some(LabsState::wait(10)))
        } else {
            Ok(Some(LabsState::wait(10)))
        }
    }

    fn desired_resources() -> &'static [(ResourceType, u32)] {
        &[
            //
            // Tier 1 boosts
            //

            (ResourceType::UtriumHydride, 1000),
            (ResourceType::UtriumOxide, 1000),
            (ResourceType::KeaniumHydride, 1000),
            (ResourceType::KeaniumOxide, 1000),
            (ResourceType::LemergiumHydride, 1000),
            (ResourceType::LemergiumOxide, 1000),
            (ResourceType::ZynthiumHydride, 1000),
            (ResourceType::ZynthiumOxide, 1000),
            (ResourceType::GhodiumHydride, 1000),
            (ResourceType::GhodiumOxide, 1000),
        ]
    }

    fn get_target_reaction(system_data: &mut MissionExecutionSystemData, state_context: &mut LabsMissionContext) -> Result<Option<(ResourceType, u32)>, String> {
        let room_data = system_data
            .room_data
            .get(state_context.room_data)
            .ok_or("Expected room data")?;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Labs Idle",
            room_data: &*system_data.room_data,
        };

        let mut available_resources = system_data.transfer_queue.get_available_withdrawl_totals(&transfer_queue_data, &[room_data.name], TransferType::Haul);

        let mut all_available_reactions: HashMap<ResourceType, u32> = HashMap::new();

        let mut target_resources = Self::desired_resources().to_vec();

        while let Some((target_resource, desired_amount)) = target_resources.pop() {
            let needed_amount = {
                let available_amount = available_resources.entry(target_resource).or_insert(0);

                let needed_amount = desired_amount as i32 - *available_amount as i32;

                *available_amount -= desired_amount.min(*available_amount);

                needed_amount
            };

            if needed_amount > 0 {
                let needed_amount = needed_amount as u32;

                if let Some(resource_components) = target_resource.reaction_components() {
                    let component_available_resources: Vec<_> = resource_components
                        .iter()
                        .map(|component_resource| {
                            (*component_resource, *available_resources.get(component_resource).unwrap_or(&0))
                        })
                        .collect();

                    let available_reactions = component_available_resources
                        .iter()
                        .map(|(_, available_amount)| available_amount / LAB_REACTION_AMOUNT)
                        .min()
                        .unwrap_or(0)
                        .min(needed_amount);

                    if available_reactions > 0 {
                        all_available_reactions
                            .entry(target_resource)
                            .and_modify(|e| *e += available_reactions)
                            .or_insert(available_reactions);

                        for (resource, _) in component_available_resources.iter() {
                            let used_amount = available_reactions * LAB_REACTION_AMOUNT;

                            available_resources.entry(*resource).and_modify(|e| *e -= (*e).min(used_amount));
                        }
                    }

                    for (resource, available_amount) in component_available_resources.iter() {
                        if *available_amount < needed_amount {
                            target_resources.push((*resource, needed_amount - available_amount));
                        }
                    }
                }
            }
        }

        let best_reaction = all_available_reactions
            .iter()
            .max_by_key(|(_, amount)| *amount)
            .map(|(resource_type, amount)| (*resource_type, *amount));

        Ok(best_reaction)
    }
}

impl Wait {  
    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut LabsMissionContext,
    ) -> Result<Option<LabsState>, String> {
        Ok(tick_wait(&mut self.ticks, || LabsState::idle(std::marker::PhantomData)))
    }
}

#[derive(ConvertSaveload)]
pub struct LabsMission {
    owner: EntityOption<Entity>,
    context: LabsMissionContext,
    state: LabsState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl LabsMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = LabsMission::new(owner, room_data);

        builder
            .with(MissionData::Labs(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> LabsMission {
        LabsMission {
            owner: owner.into(),
            context: LabsMissionContext {
                room_data
            },
            state: LabsState::idle(std::marker::PhantomData),
        }
    }

    pub fn can_run(room_data: &RoomData) -> bool {
        room_data
            .get_structures()
            .map(|structures| !structures.labs().is_empty())
            .unwrap_or(false)
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for LabsMission {
    fn get_owner(&self) -> &Option<Entity> {
        &self.owner
    }

    fn owner_complete(&mut self, owner: Entity) {
        assert!(Some(owner) == *self.owner);

        self.owner.take();
    }

    fn get_room(&self) -> Entity {
        self.context.room_data
    }

    fn describe_state(&self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> String {
        self.state.describe_state(system_data, mission_entity, &self.context)
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<(), String> {
        self.state.gather_data(system_data, mission_entity);

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity) -> Result<MissionResult, String> {
        while let Some(tick_result) = self.state.tick(system_data, mission_entity, &mut self.context)? {
            self.state = tick_result
        }

        self.state.visualize(system_data, mission_entity);

        Ok(MissionResult::Running)
    }
}
