use super::data::*;
use super::missionsystem::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::ordersystem::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
use std::collections::HashSet;
use log::*;

#[derive(ConvertSaveload)]
pub struct TerminalMission {
    owner: EntityOption<Entity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TerminalMission {
    pub fn build<B>(builder: B, owner: Option<Entity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TerminalMission::new(owner, room_data);

        builder
            .with(MissionData::Terminal(EntityRefCell::new(mission)))
            .marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<Entity>, room_data: Entity) -> TerminalMission {
        TerminalMission {
            owner: owner.into(),
            room_data,
        }
    }

    pub fn get_desired_reserve_terminal_amount(resource: ResourceType) -> u32 {
        match resource {
            ResourceType::Energy => OrderQueue::maximum_transfer_energy(),
            _ => 0,
        }
    }

    pub fn get_desired_storage_amount(resource: ResourceType) -> u32 {
        match resource {
            ResourceType::Energy => 150_000,
            _ => 10_000,
        }
    }

    pub fn get_desired_transfer_outgoing_terminal_amount(resource: ResourceType) -> u32 {
        match resource {
            ResourceType::Energy => 50_000,
            _ => 0,
        }
    }

    pub fn get_desired_passive_terminal_amount(resource: ResourceType) -> u32 {
        match resource {
            ResourceType::Energy => 10_000,
            _ => 10_000,
        }
    }

    pub fn get_desired_active_terminal_amount(resource: ResourceType) -> u32 {
        match resource {
            ResourceType::Energy => 10_000,
            _ => 5_000,
        }
    }

    pub fn get_minimum_terminal_transfer_amount(resource: ResourceType) -> u32 {
        match resource {
            ResourceType::Energy => 1_000,
            _ => 100,
        }
    }

    fn can_purchase_resource(resource: ResourceType) -> bool {
        match resource {
            ResourceType::Energy => true,
            _ => false,
        }
    }

    fn get_resource_thresholds(resource: ResourceType) -> ResourceThresholds {
        let desired_storage_amount = Self::get_desired_storage_amount(resource);

        let desired_reserve_terminal_amount = Self::get_desired_reserve_terminal_amount(resource);
        let desired_transfer_outgoing_terminal_amount = Self::get_desired_transfer_outgoing_terminal_amount(resource);
        let desired_passive_terminal_amount = Self::get_desired_passive_terminal_amount(resource);
        let desired_active_terminal_amount = Self::get_desired_active_terminal_amount(resource);

        let a = desired_reserve_terminal_amount;
        let b = a + desired_transfer_outgoing_terminal_amount;
        let c = b + desired_passive_terminal_amount;
        let d = c + desired_active_terminal_amount;

        ResourceThresholds {
            desired_storage_amount,

            desired_reserve_terminal_amount,
            desired_transfer_outgoing_terminal_amount,
            desired_passive_terminal_amount,
            desired_active_terminal_amount,

            terminal_reserve_threshold: 0..=a,
            terminal_transfer_outgoing_threshold: a..=b,
            terminal_passive_threshold: b..=c,
            terminal_active_threshold: c..=d,
        }
    }
}

struct ResourceThresholds {
    desired_storage_amount: u32,

    desired_reserve_terminal_amount: u32,
    desired_transfer_outgoing_terminal_amount: u32,
    desired_passive_terminal_amount: u32,
    desired_active_terminal_amount: u32,

    terminal_reserve_threshold: std::ops::RangeInclusive<u32>,
    terminal_transfer_outgoing_threshold: std::ops::RangeInclusive<u32>,
    terminal_passive_threshold: std::ops::RangeInclusive<u32>,
    terminal_active_threshold: std::ops::RangeInclusive<u32>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for TerminalMission {
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
        "Terminal".to_string()
    }

    fn pre_run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;
        let terminal = room.terminal().ok_or("Expected terminal")?;

        if let Some(storage) = room.storage() {
            let current_terminal_energy = terminal.store_of(ResourceType::Energy);
            let available_transfer_energy = Self::get_desired_reserve_terminal_amount(ResourceType::Energy).min(current_terminal_energy);

            let storage_resource_types = storage.store_types();
            let terminal_storage_types = terminal.store_types();

            let known_resource_types: HashSet<ResourceType> = storage_resource_types
                .into_iter()
                .chain(terminal_storage_types.into_iter())
                .chain(std::iter::once(ResourceType::Energy))
                .collect();

            //TODO: Include resources that are requested by transport system but don't exist in the room.

            //
            // If there are sufficient resources in the terminal and storage, request selling them.
            //

            for resource_type in known_resource_types {
                let current_storage_amount = storage.store_used_capacity(Some(resource_type));
                let current_terminal_amount = terminal.store_used_capacity(Some(resource_type));
                let current_total_amount = current_storage_amount + current_terminal_amount;

                let thresholds = Self::get_resource_thresholds(resource_type);

                //
                // Request transfer of resources in.
                //

                let total_reserve_amount = thresholds.desired_storage_amount + thresholds.terminal_reserve_threshold.end();

                if current_total_amount < total_reserve_amount {
                    let transfer_amount = total_reserve_amount - current_total_amount;
                    let terminal_free_amount = terminal.store_free_capacity(Some(resource_type)).max(0) as u32;
                    let transfer_amount = transfer_amount.min(terminal_free_amount);

                    if transfer_amount > 0 {
                        if Self::can_purchase_resource(resource_type) {
                            //TODO: Only purchase when transfer is not available.
                            //TODO: Need to correctly figure out how much
                            let purchase_amount = transfer_amount / 4;
    
                            if purchase_amount > 0 {
                                system_data
                                    .order_queue
                                    .request_passive_purchase(room_data.name, resource_type, purchase_amount);
                            }
                        }

                        if transfer_amount > Self::get_minimum_terminal_transfer_amount(resource_type) {
                            let deposit_request = TransferDepositRequest::new(
                                TransferTarget::Terminal(terminal.remote_id()), 
                                Some(resource_type), 
                                TransferPriority::Low, 
                                transfer_amount, 
                                TransferType::Terminal);
                            
                            system_data
                                .transfer_queue
                                .request_deposit(deposit_request);
                        }
                    }
                } else {
                    let effective_terminal_amount = current_total_amount - thresholds.desired_storage_amount.min(current_total_amount);

                    if effective_terminal_amount >= *thresholds.terminal_transfer_outgoing_threshold.start() {
                        let transfer_amount = effective_terminal_amount - thresholds.terminal_transfer_outgoing_threshold.start();
                        let transfer_amount =
                        transfer_amount.min(thresholds.terminal_transfer_outgoing_threshold.end() - thresholds.terminal_transfer_outgoing_threshold.start());

                        if transfer_amount > 0 && transfer_amount >= Self::get_minimum_terminal_transfer_amount(resource_type) {
                            let withdraw_request = TransferWithdrawRequest::new(
                                TransferTarget::Terminal(terminal.remote_id()), 
                                resource_type, 
                                TransferPriority::None, 
                                transfer_amount, 
                                TransferType::Terminal);
                            
                            system_data
                                .transfer_queue
                                .request_withdraw(withdraw_request);
                        }
                    }
                    
                    if effective_terminal_amount >= *thresholds.terminal_passive_threshold.start() {
                        let passive_amount = effective_terminal_amount - thresholds.terminal_passive_threshold.start();
                        let passive_amount =
                            passive_amount.min(thresholds.terminal_passive_threshold.end() - thresholds.terminal_passive_threshold.start());

                        if passive_amount > 0 {
                            system_data
                                .order_queue
                                .request_passive_sale(room_data.name, resource_type, passive_amount);
                        }
                    }

                    if effective_terminal_amount >= *thresholds.terminal_active_threshold.start() {
                        let active_amount = effective_terminal_amount - thresholds.terminal_active_threshold.start();
                        let active_amount =
                            active_amount.min(thresholds.terminal_active_threshold.end() - thresholds.terminal_active_threshold.start());

                        if active_amount > 0 {
                            system_data.order_queue.request_active_sale(
                                room_data.name,
                                resource_type,
                                active_amount,
                                available_transfer_energy,
                            );
                        }
                    }
                }
            }
        }

        system_data.transfer_queue.register_generator(
            room_data.name,
            TransferTypeFlags::HAUL,
            Box::new(move |_system, transfer, room_name| {
                let room = game::rooms::get(room_name).ok_or("Expected room")?;
                let terminal = room.terminal().ok_or("Expected terminal")?;

                let terminal_storage_types = terminal.store_types();
                let terminal_id = terminal.remote_id();

                if let Some(storage) = room.storage() {
                    let storage_resource_types = storage.store_types();

                    let all_resource_types = storage_resource_types
                        .into_iter()
                        .chain(terminal_storage_types.into_iter())
                        .chain(std::iter::once(ResourceType::Energy))
                        .unique();

                    for resource_type in all_resource_types {
                        let current_storage_amount = storage.store_used_capacity(Some(resource_type));
                        let current_terminal_amount = terminal.store_used_capacity(Some(resource_type));

                        let thresholds = Self::get_resource_thresholds(resource_type);

                        //
                        // Ensure a reserve amount of the resource is held in the terminal
                        //

                        if current_terminal_amount < *thresholds.terminal_reserve_threshold.end() {
                            let max_transfer = thresholds.terminal_reserve_threshold.end() - thresholds.terminal_reserve_threshold.start();
                            let transfer_amount = (*thresholds.terminal_reserve_threshold.end() - current_terminal_amount).min(max_transfer);

                            let transfer_request = TransferDepositRequest::new(
                                TransferTarget::Terminal(terminal_id),
                                Some(resource_type),
                                TransferPriority::Medium,
                                transfer_amount,
                                TransferType::Haul,
                            );

                            transfer.request_deposit(transfer_request);
                        }

                        //
                        // If there is excess resources in storage and a shortage in the terminal, request transfer of
                        // those resources.
                        //

                        if current_storage_amount > thresholds.desired_storage_amount {
                            if current_terminal_amount < *thresholds.terminal_transfer_outgoing_threshold.end() {
                                let max_transfer = thresholds.terminal_transfer_outgoing_threshold.end() - thresholds.terminal_transfer_outgoing_threshold.start();
                                let transfer_amount = (*thresholds.terminal_transfer_outgoing_threshold.end() - current_terminal_amount).min(max_transfer);
    
                                let transfer_request = TransferDepositRequest::new(
                                    TransferTarget::Terminal(terminal_id),
                                    Some(resource_type),
                                    TransferPriority::Low,
                                    transfer_amount,
                                    TransferType::Haul,
                                );
    
                                transfer.request_deposit(transfer_request);
                            }

                            if current_terminal_amount < *thresholds.terminal_passive_threshold.end() {
                                let max_transfer = thresholds.terminal_passive_threshold.end() - thresholds.terminal_passive_threshold.start();
                                let transfer_amount = (*thresholds.terminal_passive_threshold.end() - current_terminal_amount).min(max_transfer);
    
                                let transfer_request = TransferDepositRequest::new(
                                    TransferTarget::Terminal(terminal_id),
                                    Some(resource_type),
                                    TransferPriority::Low,
                                    transfer_amount,
                                    TransferType::Haul,
                                );
    
                                transfer.request_deposit(transfer_request);
                            }

                            if current_terminal_amount < *thresholds.terminal_active_threshold.end() {
                                let max_transfer = thresholds.terminal_active_threshold.end() - thresholds.terminal_active_threshold.start();
                                let transfer_amount = (*thresholds.terminal_active_threshold.end() - current_terminal_amount).min(max_transfer);
    
                                let transfer_request = TransferDepositRequest::new(
                                    TransferTarget::Terminal(terminal_id),
                                    Some(resource_type),
                                    TransferPriority::Low,
                                    transfer_amount,
                                    TransferType::Haul,
                                );
    
                                transfer.request_deposit(transfer_request);
                            }
                        }

                        //
                        // Make available any resources that are in the terminal and there is not sufficient amount in storage.
                        //

                        //TODO: This should likely prevent sellling, not just make them available. Factor our amount storage needs in calculation above.
                        let made_available_amount = if current_storage_amount < thresholds.desired_storage_amount {
                            if current_terminal_amount > *thresholds.terminal_reserve_threshold.end() {
                                let available_terminal_amount = current_terminal_amount - *thresholds.terminal_reserve_threshold.end();
                                let transfer_amount =
                                    (thresholds.desired_storage_amount - current_storage_amount).min(available_terminal_amount);

                                if transfer_amount > 0 {
                                    let transfer_request = TransferWithdrawRequest::new(
                                        TransferTarget::Terminal(terminal_id),
                                        resource_type,
                                        TransferPriority::None,
                                        transfer_amount,
                                        TransferType::Haul,
                                    );

                                    transfer.request_withdraw(transfer_request);
                                }

                                transfer_amount
                            } else {
                                0
                            }
                        } else {
                            0
                        };

                        //
                        // Actively transfer any resources that are in excess of the desired terminal amount (reserve, active and passive)
                        // and are not already being made avaiable due to storage shortage.
                        //

                        if current_terminal_amount > *thresholds.terminal_active_threshold.end() {
                            let terminal_excess = current_terminal_amount - *thresholds.terminal_active_threshold.end();
                            let transfer_amount = (terminal_excess as i32) - (made_available_amount as i32);

                            if transfer_amount > 0 {
                                let transfer_request = TransferWithdrawRequest::new(
                                    TransferTarget::Terminal(terminal_id),
                                    resource_type,
                                    TransferPriority::Medium,
                                    transfer_amount as u32,
                                    TransferType::Haul,
                                );

                                transfer.request_withdraw(transfer_request);
                            }
                        }
                    }
                }

                Ok(())
            }),
        );

        Ok(())
    }

    fn run_mission(&mut self, system_data: &mut MissionExecutionSystemData, _mission_entity: Entity) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let terminal = room.terminal().ok_or("Expected terminal")?;

        let transfer_queue = &mut system_data.transfer_queue;

        if terminal.cooldown() == 0 {
            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Terminal Transfer",
                room_data: &*system_data.room_data,
            };

            let thresholds = Self::get_resource_thresholds(ResourceType::Energy);
            let current_terminal_energy = terminal.store_of(ResourceType::Energy);

            let available_transfer_energy = if current_terminal_energy >= *thresholds.terminal_reserve_threshold.start() {
                let max_amount = *thresholds.terminal_reserve_threshold.end() - *thresholds.terminal_reserve_threshold.start();
                (current_terminal_energy - *thresholds.terminal_reserve_threshold.start()).min(max_amount)
            } else {
                0
            };

            //TODO: Potentially use active priority pairs to iterate here. Currently relies on there never being a None -> None priority request.
            let best_transfer = ALL_TRANSFER_PRIORITIES
                .iter()
                .filter_map(|priority| {
                    transfer_queue.get_terminal_delivery_from_target(
                        &transfer_queue_data,
                        &TransferTarget::Terminal(terminal.remote_id()),
                        TransferPriorityFlags::ACTIVE,
                        priority.into(),
                        TransferType::Terminal,
                        available_transfer_energy,
                        TransferCapacity::Infinite,
                    )
                })
                .next();

            if let Some((pickup, delivery)) = best_transfer {
                transfer_queue.register_pickup(&pickup, TransferType::Terminal);
                transfer_queue.register_delivery(&delivery, TransferType::Terminal);

                if let Some((transfer_resource, transfer_amount)) = delivery
                    .resources()
                    .iter()
                    .map(|(resource, entries)| (resource, entries.iter().map(|e| e.amount()).sum::<u32>()))
                    .max_by_key(|(_, amount)| *amount) {

                    info!("Terminal transfer: {} -> {} - Resource: {:?} - Amount: {}", room_data.name, delivery.target().pos().room_name(), transfer_resource, transfer_amount);

                    terminal.send(*transfer_resource, transfer_amount, delivery.target().pos().room_name(), None);
                }
            }
        }

        Ok(MissionResult::Running)
    }
}
