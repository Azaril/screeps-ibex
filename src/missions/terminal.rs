use super::data::*;
use super::missionsystem::*;
use crate::remoteobjectid::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[derive(Clone, ConvertSaveload)]
pub struct TerminalMission {
    room_data: Entity,
}

#[cfg_attr(feature = "time", timing)]
impl TerminalMission {
    pub fn build<B>(builder: B, room_data: Entity) -> B
    where
        B: Builder + MarkedBuilder,
    {
        let mission = TerminalMission::new(room_data);

        builder
            .with(MissionData::Terminal(mission))
            .marked::<::serialize::SerializeMarker>()
    }

    pub fn new(room_data: Entity) -> TerminalMission {
        TerminalMission { room_data }
    }
}

#[cfg_attr(feature = "time", timing)]
impl Mission for TerminalMission {
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
            let storage_resource_types = storage.store_types();

            let all_resource_types = storage_resource_types.iter().chain(terminal_storage_types.iter()).unique();

            for resource_type in all_resource_types {
                let current_storage_amount = storage.store_used_capacity(Some(*resource_type));
                let current_terminal_amount = terminal.store_used_capacity(Some(*resource_type));

                let desired_storage_amount = match resource_type {
                    ResourceType::Energy => 500_000,
                    _ => 10_000,
                };

                let desired_terminal_amount = match resource_type {
                    ResourceType::Energy => 50_000,
                    _ => 10_000,
                };

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
                        );

                        runtime_data.transfer_queue.request_withdraw(transfer_request);
                    }

                    transfer_amount
                } else {
                    0
                };

                //
                // Actively transfer any resources that are in excess of the desired terminal amount and are not already
                // being made avaiable due to storage shortage.
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
                        );

                        runtime_data.transfer_queue.request_withdraw(transfer_request);
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
