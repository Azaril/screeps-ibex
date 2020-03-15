use super::actions::*;
use super::jobsystem::*;
use super::utility::buildbehavior::*;
use super::utility::controllerbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::movebehavior::*;
use super::utility::repair::*;
use super::utility::repairbehavior::*;
use super::utility::waitbehavior::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use crate::transfer::transfersystem::*;
use crate::visualize::*;
use screeps::*;
use serde::{Deserialize, Serialize};
use specs::error::NoError;
use specs::saveload::*;
use specs::*;
use specs_derive::*;

#[cfg(feature = "time")]
use timing_annotate::*;

#[derive(Clone, Serialize, Deserialize)]
pub enum HarvestState {
    Idle(),
    Harvest(RemoteObjectId<Source>, u8),
    Pickup(TransferWithdrawTicket, Vec<TransferDepositTicket>),
    Delivery(Vec<TransferDepositTicket>),
    FinishedDelivery(),
    Build(RemoteObjectId<ConstructionSite>),
    FinishedBuild(),
    Repair(RemoteStructureIdentifier),
    FinishedRepair(),
    Upgrade(RemoteObjectId<StructureController>),
    MoveToRoom(RoomName),
    Wait(u32),
}

#[derive(Clone, ConvertSaveload)]
pub struct HarvestJob {
    harvest_target: RemoteObjectId<Source>,
    delivery_room: Entity,
    allow_haul: bool,
    state: HarvestState,
}

#[cfg_attr(feature = "time", timing)]
impl HarvestJob {
    pub fn new(harvest_target: RemoteObjectId<Source>, delivery_room: Entity, allow_haul: bool) -> HarvestJob {
        HarvestJob {
            harvest_target,
            delivery_room,
            allow_haul,
            state: HarvestState::Idle(),
        }
    }

    pub fn harvest_target(&self) -> &RemoteObjectId<Source> {
        &self.harvest_target
    }

    fn run_idle_state(
        creep: &Creep,
        delivery_room_data: &RoomData,
        transfer_queue: &mut TransferQueue,
        harvest_target: &RemoteObjectId<Source>,
        allow_haul: bool,
    ) -> Option<HarvestState> {
        let in_delivery_room = creep.room().map(|r| r.name() == delivery_room_data.name).unwrap_or(false);

        if in_delivery_room && allow_haul {
            if let Some(state) = get_new_pickup_and_delivery_full_capacity_state(
                creep,
                &[delivery_room_data],
                TransferPriorityFlags::HIGH,
                TransferType::Haul,
                transfer_queue,
                HarvestState::Pickup,
            ) {
                return Some(state);
            }
        }

        if let Some(state) = get_new_harvest_target_state(creep, harvest_target, |id| HarvestState::Harvest(id, 0)) {
            return Some(state);
        };

        if in_delivery_room {
            if allow_haul {
                if let Some(state) = get_new_pickup_and_delivery_full_capacity_state(
                    creep,
                    &[delivery_room_data],
                    TransferPriorityFlags::MEDIUM | TransferPriorityFlags::LOW,
                    TransferType::Haul,
                    transfer_queue,
                    HarvestState::Pickup,
                ) {
                    return Some(state);
                }
            }

            get_new_delivery_current_resources_state(
                creep,
                &[delivery_room_data],
                TransferPriorityFlags::HIGH,
                TransferTypeFlags::HAUL,
                transfer_queue,
                HarvestState::Delivery,
            )
            .or_else(|| get_new_build_state(creep, delivery_room_data, HarvestState::Build))
            .or_else(|| get_new_repair_state(creep, delivery_room_data, Some(RepairPriority::Medium), HarvestState::Repair))
            .or_else(|| {
                [TransferPriority::Medium, TransferPriority::Low, TransferPriority::None]
                    .iter()
                    .filter_map(|priority| {
                        get_new_delivery_current_resources_state(
                            creep,
                            &[delivery_room_data],
                            TransferPriorityFlags::from(priority),
                            TransferTypeFlags::HAUL,
                            transfer_queue,
                            HarvestState::Delivery,
                        )
                    })
                    .next()
            })
            .or_else(|| get_new_upgrade_state(creep, delivery_room_data, HarvestState::Upgrade))
            .or_else(|| if creep.store_used_capacity(None) == 0 {
                get_new_move_to_room_state(creep, harvest_target.pos().room_name(), HarvestState::MoveToRoom)
            } else {
                None
            })
            .or_else(|| Some(HarvestState::Wait(5)))
        } else {
            get_new_move_to_room_state(creep, delivery_room_data.name, HarvestState::MoveToRoom)
        }
    }

    fn run_finished_delivery_state(
        creep: &Creep,
        delivery_room_data: &RoomData,
        transfer_queue: &mut TransferQueue,
    ) -> Option<HarvestState> {
        ALL_TRANSFER_PRIORITIES
            .iter()
            .filter_map(|priority| {
                get_new_delivery_current_resources_state(
                    creep,
                    &[delivery_room_data],
                    priority.into(),
                    TransferTypeFlags::HAUL,
                    transfer_queue,
                    HarvestState::Delivery,
                )
            })
            .next()
            .or(Some(HarvestState::Idle()))
    }

    fn run_finished_build_state(creep: &Creep, delivery_room_data: &RoomData) -> Option<HarvestState> {
        get_new_build_state(creep, delivery_room_data, HarvestState::Build).or(Some(HarvestState::Idle()))
    }

    fn run_finished_repair_state(creep: &Creep, delivery_room_data: &RoomData) -> Option<HarvestState> {
        get_new_repair_state(creep, delivery_room_data, Some(RepairPriority::Medium), HarvestState::Repair).or(Some(HarvestState::Idle()))
    }
}

#[cfg_attr(feature = "time", timing)]
impl Job for HarvestJob {
    fn describe(&mut self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        let name = describe_data.owner.name();
        let pos = describe_data.owner.pos();

        if let Some(room) = describe_data.owner.room() {
            describe_data
                .ui
                .with_room(room.name(), &mut describe_data.visualizer, |room_ui| match &self.state {
                    HarvestState::Idle() => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Idle", name), None);
                    }
                    HarvestState::Harvest(_, _) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Harvest", name), None);
                    }
                    HarvestState::Pickup(pickup_ticket, delivery_tickets) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Pickup", name), None);

                        if crate::features::transfer::visualize_haul() {
                            let pickup_pos = pickup_ticket.target().pos();
                            room_ui.visualizer().line(
                                (pos.x() as f32, pos.y() as f32),
                                (pickup_pos.x() as f32, pickup_pos.y() as f32),
                                Some(LineStyle::default().color("blue")),
                            );

                            let mut last_pos = pickup_pos;
                            for delivery_ticket in delivery_tickets {
                                let delivery_pos = delivery_ticket.target().pos();
                                room_ui.visualizer().line(
                                    (last_pos.x() as f32, last_pos.y() as f32),
                                    (delivery_pos.x() as f32, delivery_pos.y() as f32),
                                    Some(LineStyle::default().color("green")),
                                );
                                last_pos = delivery_pos;
                            }
                        }
                    }
                    HarvestState::Delivery(delivery_tickets) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Delivery", name), None);

                        if crate::features::transfer::visualize_haul() {
                            let mut last_pos = pos;
                            for delivery_ticket in delivery_tickets {
                                let delivery_pos = delivery_ticket.target().pos();
                                room_ui.visualizer().line(
                                    (last_pos.x() as f32, last_pos.y() as f32),
                                    (delivery_pos.x() as f32, delivery_pos.y() as f32),
                                    Some(LineStyle::default().color("green")),
                                );
                                last_pos = delivery_pos;
                            }
                        }
                    }
                    HarvestState::FinishedDelivery() => {
                        room_ui.jobs().add_text(format!("Harvest - {} - FinishedDelivery", name), None);
                    }
                    HarvestState::Build(_) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Build", name), None);
                    }
                    HarvestState::FinishedBuild() => {
                        room_ui.jobs().add_text(format!("Harvest - {} - FinishedBuild", name), None);
                    }
                    HarvestState::Repair(_) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Repair", name), None);
                    }
                    HarvestState::FinishedRepair() => {
                        room_ui.jobs().add_text(format!("Harvest - {} - FinishedRepair", name), None);
                    }
                    HarvestState::Upgrade(_) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Upgrade", name), None);
                    }
                    HarvestState::MoveToRoom(room) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - MoveToRoom - {}", name, room), None);
                    }
                    HarvestState::Wait(_) => {
                        room_ui.jobs().add_text(format!("Harvest - {} - Wait", name), None);
                    }
                })
        }
    }

    fn pre_run_job(&mut self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        match &self.state {
            HarvestState::Idle() => {}
            HarvestState::Harvest(_, _) => {}
            HarvestState::Pickup(pickup_ticket, delivery_tickets) => {
                runtime_data.transfer_queue.register_pickup(&pickup_ticket, TransferType::Haul);
                for delivery_ticket in delivery_tickets {
                    runtime_data.transfer_queue.register_delivery(&delivery_ticket, TransferType::Haul);
                }
            }
            HarvestState::Delivery(delivery_tickets) => {
                for delivery_ticket in delivery_tickets {
                    runtime_data.transfer_queue.register_delivery(&delivery_ticket, TransferType::Haul);
                }
            }
            HarvestState::FinishedDelivery() => {}
            HarvestState::Build(_) => {}
            HarvestState::FinishedBuild() => {}
            HarvestState::Repair(_) => {}
            HarvestState::FinishedRepair() => {}
            HarvestState::Upgrade(_) => {}
            HarvestState::MoveToRoom(_) => {}
            HarvestState::Wait(_) => {}
        };
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let creep = runtime_data.owner;

        let mut action_flags = SimultaneousActionFlags::UNSET;

        if let Some(delivery_room_data) = system_data.room_data.get(self.delivery_room) {
            loop {
                let state_result = match &mut self.state {
                    HarvestState::Idle() => Self::run_idle_state(
                        creep,
                        delivery_room_data,
                        runtime_data.transfer_queue,
                        &self.harvest_target,
                        self.allow_haul,
                    ),
                    HarvestState::Harvest(source_id, stuck_count) => run_harvest_state(creep, &mut action_flags, source_id, false, stuck_count, HarvestState::Idle),
                    HarvestState::Pickup(pickup_ticket, delivery_ticket) => {
                        run_pickup_state(creep, &mut action_flags, pickup_ticket, runtime_data.transfer_queue, || {
                            HarvestState::Delivery(delivery_ticket.clone())
                        })
                    }
                    HarvestState::Delivery(tickets) => {
                        run_delivery_state(creep, &mut action_flags, tickets, runtime_data.transfer_queue, HarvestState::FinishedDelivery)
                    }
                    HarvestState::FinishedDelivery() => {
                        Self::run_finished_delivery_state(creep, delivery_room_data, runtime_data.transfer_queue)
                    }
                    HarvestState::Build(construction_site_id) => {
                        run_build_state(creep, &mut action_flags, construction_site_id, HarvestState::FinishedBuild)
                    }
                    HarvestState::FinishedBuild() => Self::run_finished_build_state(creep, delivery_room_data),
                    HarvestState::Repair(repair_structure_id) => {
                        run_repair_state(creep, &mut action_flags, repair_structure_id, HarvestState::FinishedRepair)
                    }
                    HarvestState::FinishedRepair() => Self::run_finished_repair_state(creep, delivery_room_data),
                    HarvestState::Upgrade(controller_id) => run_upgrade_state(creep, controller_id, HarvestState::Idle),
                    HarvestState::MoveToRoom(room) => run_move_to_room_state(creep, *room, HarvestState::Idle),
                    HarvestState::Wait(time) => run_wait_state(time, HarvestState::Idle)
                };

                if let Some(next_state) = state_result {
                    self.state = next_state;
                } else {
                    break;
                }
            }
        }
    }
}
