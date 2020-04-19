use super::data::*;
use super::missionsystem::*;
use crate::ownership::*;
use crate::remoteobjectid::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[derive(Clone, ConvertSaveload)]
pub struct TerminalMission {
    owner: EntityOption<OperationOrMissionEntity>,
    room_data: Entity,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl TerminalMission {
    pub fn build<B>(builder: B, owner: Option<OperationOrMissionEntity>, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TerminalMission::new(owner, room_data);

        builder.with(MissionData::Terminal(mission)).marked::<SerializeMarker>()
    }

    pub fn new(owner: Option<OperationOrMissionEntity>, room_data: Entity) -> TerminalMission {
        TerminalMission {
            owner: owner.into(),
            room_data,
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Mission for TerminalMission {
    fn get_owner(&self) -> &Option<OperationOrMissionEntity> {
        &self.owner
    }

    fn get_room(&self) -> Entity {
        self.room_data
    }

    fn describe(&mut self, system_data: &MissionExecutionSystemData, describe_data: &mut MissionDescribeData) {
        if let Some(room_data) = system_data.room_data.get(self.room_data) {
            describe_data.ui.with_room(room_data.name, describe_data.visualizer, |room_ui| {
                room_ui.missions().add_text("Terminal".to_string(), None);
            })
        }
    }

    fn pre_run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<(), String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let terminal = room.terminal().ok_or("Expected terminal")?;
        let terminal_storage_types = terminal.store_types();
        let terminal_id = terminal.remote_id();

        if let Some(storage) = room.storage() {
            //
            // Transfer energy needed for purchase/sale to the terminal.
            //
            let energy_reserve = runtime_data.order_queue.maximum_transfer_energy();
            let current_terminal_energy = terminal.store_used_capacity(Some(ResourceType::Energy));
            let available_transfer_energy = energy_reserve.min(current_terminal_energy);

            if current_terminal_energy < energy_reserve {
                let transfer_amount = energy_reserve - current_terminal_energy;

                let transfer_request = TransferDepositRequest::new(
                    TransferTarget::Terminal(terminal_id),
                    Some(ResourceType::Energy),
                    TransferPriority::Medium,
                    transfer_amount,
                    TransferType::Haul,
                );

                runtime_data.transfer_queue.request_deposit(transfer_request);
            }

            let storage_resource_types = storage.store_types();

            let all_resource_types = storage_resource_types.iter().chain(terminal_storage_types.iter()).unique();

            for resource_type in all_resource_types {
                let current_storage_amount = storage.store_used_capacity(Some(*resource_type));
                let mut current_terminal_amount = terminal.store_used_capacity(Some(*resource_type));

                if *resource_type == ResourceType::Energy {
                    current_terminal_amount -= energy_reserve.min(current_terminal_amount);
                }

                let desired_storage_amount = match resource_type {
                    ResourceType::Energy => 150_000,
                    _ => 10_000,
                };

                let desired_passive_terminal_amount = match resource_type {
                    ResourceType::Energy => 10_000,
                    _ => 10_000,
                };

                let desired_active_terminal_amount = match resource_type {
                    ResourceType::Energy => 10_000,
                    _ => 5_000,
                };

                let desired_terminal_amount = desired_passive_terminal_amount + desired_active_terminal_amount;

                //
                // If there is excess resources in storage and a shortage in the terminal, request transfer of
                // those resources.
                //

                if current_storage_amount > desired_storage_amount && current_terminal_amount < desired_terminal_amount {
                    let storage_excess = current_storage_amount - desired_storage_amount;
                    let terminal_shortage = desired_terminal_amount - current_terminal_amount;

                    let transfer_amount = storage_excess.min(terminal_shortage);

                    if transfer_amount > 0 {
                        let transfer_request = TransferDepositRequest::new(
                            TransferTarget::Terminal(terminal_id),
                            Some(*resource_type),
                            TransferPriority::Medium,
                            transfer_amount,
                            TransferType::Haul,
                        );

                        runtime_data.transfer_queue.request_deposit(transfer_request);
                    }
                }

                //
                // Make available any resources that are in the terminal and there is not sufficient amount in storage.
                //

                let made_available_amount = if current_storage_amount < desired_storage_amount {
                    let transfer_amount = (desired_storage_amount - current_storage_amount).min(current_terminal_amount);

                    if transfer_amount > 0 {
                        let transfer_request = TransferWithdrawRequest::new(
                            TransferTarget::Terminal(terminal_id),
                            *resource_type,
                            TransferPriority::None,
                            transfer_amount,
                            TransferType::Haul,
                        );

                        runtime_data.transfer_queue.request_withdraw(transfer_request);
                    }

                    transfer_amount
                } else {
                    0
                };

                //
                // Actively transfer any resources that are in excess of the desired terminal amount (active and passive)
                // and are not already being made avaiable due to storage shortage.
                //

                if current_terminal_amount > desired_terminal_amount {
                    let terminal_excess = current_terminal_amount - desired_terminal_amount;
                    let transfer_amount = (terminal_excess as i32) - (made_available_amount as i32);

                    if transfer_amount > 0 {
                        let transfer_request = TransferWithdrawRequest::new(
                            TransferTarget::Terminal(terminal_id),
                            *resource_type,
                            TransferPriority::Medium,
                            transfer_amount as u32,
                            TransferType::Haul,
                        );

                        runtime_data.transfer_queue.request_withdraw(transfer_request);
                    }
                }

                //
                // If there are sufficient resources in the terminal and storage, request selling them.
                //

                if current_storage_amount >= desired_storage_amount {
                    let passive_amount = current_terminal_amount.min(desired_passive_terminal_amount);

                    if passive_amount > 0 {
                        runtime_data
                            .order_queue
                            .request_passive_sale(room_data.name, *resource_type, passive_amount);
                    }

                    let active_amount = current_terminal_amount - passive_amount;

                    if active_amount > 0 {
                        runtime_data.order_queue.request_active_sale(
                            room_data.name,
                            *resource_type,
                            active_amount,
                            available_transfer_energy,
                        );
                    }
                }
            }
        }

        Ok(())
    }

    fn run_mission(
        &mut self,
        system_data: &MissionExecutionSystemData,
        _runtime_data: &mut MissionExecutionRuntimeData,
    ) -> Result<MissionResult, String> {
        let room_data = system_data.room_data.get(self.room_data).ok_or("Expected room data")?;
        let room = game::rooms::get(room_data.name).ok_or("Expected room")?;

        let _terminal = room.terminal().ok_or("Expected terminal");

        Ok(MissionResult::Running)
    }
}
