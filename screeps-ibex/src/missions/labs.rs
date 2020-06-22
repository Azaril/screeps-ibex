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
use crate::remoteobjectid::*;
use std::marker::PhantomData;

#[derive(Clone, ConvertSaveload)]
pub struct LabsMissionContext {
    room_data: Entity,
}

machine!(
    #[derive(Clone, ConvertSaveload)]
    enum LabsState {
        Idle {
            phantom: PhantomData<Entity>
        },
        Wait {
            ticks: u32
        },
        RunReaction {
            reaction: ResourceType,
            amount: u32,
            input: Vec<(ObjectId<StructureLab>, ResourceType)>,
            output: Vec<ObjectId<StructureLab>>,
        },
        Drain {
            input: Vec<(ObjectId<StructureLab>, ResourceType)>,
            output: Vec<ObjectId<StructureLab>>,
        }
    }

    impl {
        * => fn describe_state(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity, _state_context: &LabsMissionContext) -> String {
            format!("Labs - {}", self.status_description())
        }

        _ => fn status_description(&self) -> String;

        * => fn visualize(&self, _system_data: &MissionExecutionSystemData, _mission_entity: Entity) {}

        Idle, Wait => fn gather_data(&self, _system_data: &mut MissionExecutionSystemData, _mission_entity: Entity, _state_context: &mut LabsMissionContext) {}

        _ => fn tick(&mut self, system_data: &mut MissionExecutionSystemData, mission_entity: Entity, state_context: &mut LabsMissionContext) -> Result<Option<LabsState>, String>;
    }
);

impl Idle {
    fn status_description(&self) -> String {
        format!("Idle")
    }

    fn get_labs(system_data: &mut MissionExecutionSystemData, state_context: &mut LabsMissionContext, input_labs: usize) -> Result<(Vec<ObjectId<StructureLab>>, Vec<ObjectId<StructureLab>>), String> {
        let room_data = system_data
            .room_data
            .get(state_context.room_data)
            .ok_or("Expected room data")?;

        let structures = room_data.get_structures().ok_or("Expected structures")?;

        let labs = structures.labs();
        
        let inputs: Vec<_> = labs
            .iter()
            .filter(|lab| {
                let pos = lab.pos();

                labs.iter().all(|other_lab| other_lab.pos().get_range_to(&pos) <= 2)
            })
            .take(input_labs)
            .map(|l| l.id())
            .collect();

        if inputs.len() != input_labs {
            return Err("Insufficient input labs to run reaction".to_owned());
        }

        let outputs: Vec<_> = labs
            .iter()
            .filter(|lab| !inputs.contains(&lab.id()))
            .map(|l| l.id())
            .collect();

        Ok((inputs, outputs))
    }

    fn tick(
        &mut self,
        system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        state_context: &mut LabsMissionContext,
    ) -> Result<Option<LabsState>, String> {
        if let Some((resource_type, amount)) = Self::get_target_reaction(system_data, state_context)? {               
            let components = resource_type.reaction_components().ok_or("Expected reaction components")?;

            if let Ok((inputs, outputs)) = Self::get_labs(system_data, state_context, components.len()) {
                let room_data = system_data
                    .room_data
                    .get(state_context.room_data)
                    .ok_or("Expected room data")?;

                info!("Selected reaction - Room: {} Resource: {:?} - Amount: {}", room_data.name, resource_type, amount);
                
                let inputs: Vec<_> = inputs
                    .into_iter()
                    .zip(components.iter())
                    .map(|(lab, component)| (lab, *component))
                    .collect();

                return Ok(Some(LabsState::run_reaction(resource_type, amount, inputs, outputs)));
            }
        }

        Ok(Some(LabsState::wait(10)))
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

            //
            // Tier 2 boosts
            //

            (ResourceType::UtriumAcid, 1000),
            (ResourceType::UtriumAlkalide, 1000),
            (ResourceType::KeaniumAcid, 1000),
            (ResourceType::KeaniumAlkalide, 1000),
            (ResourceType::LemergiumAcid, 1000),
            (ResourceType::LemergiumAlkalide, 1000),
            (ResourceType::ZynthiumAcid, 1000),
            (ResourceType::ZynthiumAlkalide, 1000),
            (ResourceType::GhodiumAcid, 1000),
            (ResourceType::GhodiumAlkalide, 1000),    
            
            //
            // Tier 3 boosts
            //

            (ResourceType::CatalyzedUtriumAcid, 1000),
            (ResourceType::CatalyzedUtriumAlkalide, 1000),
            (ResourceType::CatalyzedKeaniumAcid, 1000),
            (ResourceType::CatalyzedKeaniumAlkalide, 1000),
            (ResourceType::CatalyzedLemergiumAcid, 1000),
            (ResourceType::CatalyzedLemergiumAlkalide, 1000),
            (ResourceType::CatalyzedZynthiumAcid, 1000),
            (ResourceType::CatalyzedZynthiumAlkalide, 1000),
            (ResourceType::CatalyzedGhodiumAcid, 1000),
            (ResourceType::CatalyzedGhodiumAlkalide, 1000),    
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
                    //
                    // Compute amount of component resources currently available.
                    //

                    let component_available_resources: Vec<_> = resource_components
                        .iter()
                        .map(|component_resource| {
                            (*component_resource, *available_resources.get(component_resource).unwrap_or(&0))
                        })
                        .collect();

                    //
                    // Compute number of reactions of the current target that can be run.
                    //

                    //TODO: Include any boosts or power creep usage here?
                    let available_reactions = component_available_resources
                        .iter()
                        .map(|(_, available_amount)| available_amount / LAB_REACTION_AMOUNT)
                        .min()
                        .unwrap_or(0)
                        .min(needed_amount / LAB_REACTION_AMOUNT);

                    if available_reactions > 0 {
                        all_available_reactions
                            .entry(target_resource)
                            .and_modify(|e| *e += available_reactions * LAB_REACTION_AMOUNT)
                            .or_insert(available_reactions * LAB_REACTION_AMOUNT);

                        for (resource, _) in component_available_resources.iter() {
                            let used_amount = available_reactions * LAB_REACTION_AMOUNT;

                            available_resources.entry(*resource).and_modify(|e| *e -= (*e).min(used_amount));
                        }
                    }

                    //
                    // Add target for component resources that need to be created.
                    //

                    for (resource, component_available_amount) in component_available_resources.iter() {
                        if *component_available_amount < needed_amount {
                            target_resources.push((*resource, needed_amount - component_available_amount));
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
    fn status_description(&self) -> String {
        format!("Wait - {}", self.ticks)
    }

    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut LabsMissionContext,
    ) -> Result<Option<LabsState>, String> {
        Ok(tick_wait(&mut self.ticks, || LabsState::idle(PhantomData)))
    }
}

impl RunReaction {
    fn status_description(&self) -> String {
        format!("Reaction - {:?} - {:?}", self.reaction, self.amount)
    }

    fn gather_data(&self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity, state_context: &mut LabsMissionContext) {
        if let Some(room_data) = system_data.room_data.get(state_context.room_data) {
            system_data.transfer_queue.register_generator(
                room_data.name,
                TransferTypeFlags::HAUL,
                Self::transfer_generator(state_context.room_data, &self.input, &self.output, self.amount)
            );
        }
    }

    fn transfer_generator(_room_entity: Entity, input: &[(ObjectId<StructureLab>, ResourceType)], output: &[ObjectId<StructureLab>], reaction_amount: u32) -> TransferQueueGenerator {
        let input = input.to_owned();
        let output = output.to_owned();

        Box::new(move |_system, transfer, _room_name| {
            //
            // Inputs
            //

            for (lab, input_resource) in input.iter() {
                let lab = lab.resolve().ok_or("Expected lab")?;

                let current_store = lab.store_types();

                for unwanted_resource in current_store.iter().filter(|r| **r != ResourceType::Energy && *r != input_resource) {
                    let unwanted_amount = lab.store_of(*unwanted_resource);

                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Lab(lab.remote_id()),
                        *unwanted_resource,
                        TransferPriority::High,
                        unwanted_amount,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);
                }

                let needed_amount = reaction_amount;
                let current_resource_amount = lab.store_of(*input_resource);
                let free_capacity = lab.store_free_capacity(Some(*input_resource));
                
                let deposit_amount = (needed_amount as i32 - current_resource_amount as i32).min(free_capacity);

                if deposit_amount > 0 {
                    let transfer_request = TransferDepositRequest::new(
                        TransferTarget::Lab(lab.remote_id()),
                        Some(*input_resource),
                        TransferPriority::High,
                        deposit_amount as u32,
                        TransferType::Haul,
                    );

                    transfer.request_deposit(transfer_request);
                }
            }

            //
            // Outputs
            //

            for lab in output.iter() {
                let lab = lab.resolve().ok_or("Expected lab")?;

                let current_store = lab.store_types();

                for unwanted_resource in current_store.iter().filter(|r| **r != ResourceType::Energy) {
                    let amount = lab.store_of(*unwanted_resource);

                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Lab(lab.remote_id()),
                        *unwanted_resource,
                        TransferPriority::High,
                        amount,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }

            Ok(())
        })
    }

    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut LabsMissionContext,
    ) -> Result<Option<LabsState>, String> {
        if self.amount < LAB_REACTION_AMOUNT {
            return Ok(Some(LabsState::drain(self.input.clone(), self.output.clone())))
        }

        //TODO: Add stuck detection - (i.e. resources go missing).

        let (input_1, input_1_resource) = self.input.get(0).ok_or("Expected first input lab")?;
        let input_1 = input_1.resolve().ok_or("Expected to resolve first input lab")?;
        let mut input_1_resource_amount = input_1.store_of(*input_1_resource);

        let (input_2, input_2_resource) = self.input.get(1).ok_or("Expected second input lab")?;
        let input_2 = input_2.resolve().ok_or("Expected to resolve second input lab")?;
        let mut input_2_resource_amount = input_2.store_of(*input_2_resource);

        for output in self.output.iter() {
            if input_1_resource_amount < LAB_REACTION_AMOUNT || input_2_resource_amount < LAB_REACTION_AMOUNT {
                break;
            }

            let lab = output.resolve().ok_or("Expected lab")?;

            if lab.cooldown() > 0 {
                continue;
            }

            match lab.run_reaction(&input_1, &input_2) {
                ReturnCode::Ok => {
                    self.amount -= LAB_REACTION_AMOUNT;

                    input_1_resource_amount -= LAB_REACTION_AMOUNT;
                    input_2_resource_amount -= LAB_REACTION_AMOUNT;
                },
                err => {
                    error!("Failed to run lab reaction: {:?}", err)
                }
            }

            if self.amount < LAB_REACTION_AMOUNT {
                break;
            }
        }
        
        Ok(None)
    }
}

impl Drain {  
    fn status_description(&self) -> String {
        format!("Drain")
    }
    
    fn gather_data(&self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity, state_context: &mut LabsMissionContext) {
        if let Some(room_data) = system_data.room_data.get(state_context.room_data) {
            system_data.transfer_queue.register_generator(
                room_data.name,
                TransferTypeFlags::HAUL,
                Self::transfer_generator(state_context.room_data, &self.input, &self.output)
            );
        }
    }

    fn transfer_generator(_room_entity: Entity, input: &[(ObjectId<StructureLab>, ResourceType)], output: &[ObjectId<StructureLab>]) -> TransferQueueGenerator {
        let input = input.to_owned();
        let output = output.to_owned();

        Box::new(move |_system, transfer, _room_name| {
            let all_labs = input.iter().map(|(lab, _)| lab).chain(output.iter());

            for lab in all_labs {
                let lab = lab.resolve().ok_or("Expected lab")?;

                let current_store = lab.store_types();

                for unwanted_resource in current_store.iter().filter(|r| **r != ResourceType::Energy) {
                    let amount = lab.store_of(*unwanted_resource);

                    let transfer_request = TransferWithdrawRequest::new(
                        TransferTarget::Lab(lab.remote_id()),
                        *unwanted_resource,
                        TransferPriority::High,
                        amount,
                        TransferType::Haul,
                    );

                    transfer.request_withdraw(transfer_request);
                }
            }

            Ok(())
        })
    }

    fn tick(
        &mut self,
        _system_data: &mut MissionExecutionSystemData,
        _mission_entity: Entity,
        _state_context: &mut LabsMissionContext,
    ) -> Result<Option<LabsState>, String> {
        let all_labs = self.input.iter().map(|(lab, _)| lab).chain(self.output.iter());

        let has_any_resources = all_labs
            .filter_map(|l| l.resolve())
            .flat_map(|l| l.store_types())
            .any(|r| r != ResourceType::Energy);

        if has_any_resources {
            Ok(None)
        } else {
            Ok(Some(LabsState::idle(PhantomData)))
        }
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
            state: LabsState::idle(PhantomData),
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
        self.state.gather_data(system_data, mission_entity, &mut self.context);

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
