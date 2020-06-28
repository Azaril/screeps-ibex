use super::actions::*;
use super::context::*;
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
use crate::structureidentifier::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct HarvestJobContext {
    harvest_target: RemoteObjectId<Source>,
    delivery_room: Entity,
    allow_haul: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum HarvestState {
        Idle,
        Harvest { target: RemoteObjectId<Source> },
        Pickup { withdrawl: TransferWithdrawTicket, deposits: Vec<TransferDepositTicket> },
        Delivery { deposits: Vec<TransferDepositTicket> },
        FinishedDelivery,
        Build { target: RemoteObjectId<ConstructionSite> },
        FinishedBuild,
        Repair { target: RemoteStructureIdentifier },
        FinishedRepair,
        Upgrade { target: RemoteObjectId<StructureController> },
        MoveToRoom { room_name: RoomName },
        Wait { ticks: u32 },
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
            let room = { describe_data.owner.room() };

            if let Some(room) = room {
                let name = describe_data.owner.name();
                let room_name = room.name();

                describe_data
                    .ui
                    .with_room(room_name, &mut describe_data.visualizer, |room_ui| {
                        let description = self.status_description();

                        room_ui.jobs().add_text(format!("{} - {}", name, description), None);
                    });
            }
        }

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        Idle, Harvest, FinishedDelivery, Build, FinishedBuild, Repair, FinishedRepair, Upgrade, MoveToRoom, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, Harvest, FinishedDelivery, Build, FinishedBuild, Repair, FinishedRepair, Upgrade, MoveToRoom, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState>;
    }
);

impl Idle {
    fn tick(&mut self, state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        let delivery_room_data = tick_context.system_data.room_data.get(state_context.delivery_room)?;

        let harvest_room_name = state_context.harvest_target.pos().room_name();
        let harvest_room_data_entity = tick_context.runtime_data.mapping.get_room(&harvest_room_name)?;
        let harvest_room_data = tick_context.system_data.room_data.get(harvest_room_data_entity)?;

        let creep = tick_context.runtime_data.owner;

        let creep_room_name = creep.room().map(|r| r.name());

        let in_delivery_room = creep_room_name.map(|name| name == delivery_room_data.name).unwrap_or(false);
        let in_harvest_room = creep_room_name.map(|name| name == harvest_room_name).unwrap_or(false);

        if in_delivery_room && state_context.allow_haul {
            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Harvest Idle",
                room_data: &*tick_context.system_data.room_data,
            };

            if let Some(state) = get_new_pickup_and_delivery_full_capacity_state(
                creep,
                &transfer_queue_data,
                &[delivery_room_data],
                &[delivery_room_data],
                TransferPriorityFlags::HIGH,
                TransferType::Haul,
                tick_context.runtime_data.transfer_queue,
                HarvestState::pickup,
            ) {
                return Some(state);
            }
        }

        if let Some(state) = get_new_harvest_target_state(creep, &state_context.harvest_target, false, HarvestState::harvest) {
            return Some(state);
        };

        if in_harvest_room && !in_delivery_room {
            if let Some(state) = get_new_build_state(creep, harvest_room_data, HarvestState::build) {
                return Some(state);
            }
        } 
        
        if in_delivery_room {
            if state_context.allow_haul {
                let transfer_queue_data = TransferQueueGeneratorData {
                    cause: "Harvest Idle",
                    room_data: &*tick_context.system_data.room_data,
                };

                if let Some(state) = get_new_pickup_and_delivery_full_capacity_state(
                    creep,
                    &transfer_queue_data,
                    &[delivery_room_data],
                    &[delivery_room_data],
                    TransferPriorityFlags::MEDIUM | TransferPriorityFlags::LOW,
                    TransferType::Haul,
                    tick_context.runtime_data.transfer_queue,
                    HarvestState::pickup,
                ) {
                    return Some(state);
                }
            }

            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Harvest Idle",
                room_data: &*tick_context.system_data.room_data,
            };            

            get_new_delivery_current_resources_state(
                creep,
                &transfer_queue_data,
                &[delivery_room_data],
                TransferPriorityFlags::HIGH,
                TransferTypeFlags::HAUL,
                tick_context.runtime_data.transfer_queue,
                HarvestState::delivery,
            )
            .or_else(|| get_new_upgrade_state(creep, delivery_room_data, HarvestState::upgrade, Some(2)))
            .or_else(|| get_new_build_state(creep, delivery_room_data, HarvestState::build))
            .or_else(|| get_new_repair_state(creep, delivery_room_data, Some(RepairPriority::Medium), HarvestState::repair))
            .or_else(|| {
                [TransferPriority::Medium, TransferPriority::Low, TransferPriority::None]
                    .iter()
                    .filter_map(|priority| {
                        get_new_delivery_current_resources_state(
                            creep,
                            &transfer_queue_data,
                            &[delivery_room_data],
                            TransferPriorityFlags::from(priority),
                            TransferTypeFlags::HAUL,
                            tick_context.runtime_data.transfer_queue,
                            HarvestState::delivery,
                        )
                    })
                    .next()
            })
            .or_else(|| get_new_upgrade_state(creep, delivery_room_data, HarvestState::upgrade, None))
            .or_else(|| {
                if creep.store_used_capacity(None) == 0 {
                    get_new_move_to_room_state(creep, state_context.harvest_target.pos().room_name(), HarvestState::move_to_room)
                } else {
                    None
                }
            })
            .or_else(|| Some(HarvestState::wait(5)))
        } else {
            get_new_move_to_room_state(creep, delivery_room_data.name, HarvestState::move_to_room)
        }
    }
}

impl Harvest {
    fn tick(&mut self, state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        tick_opportunistic_repair(tick_context, Some(RepairPriority::Low));

        tick_harvest(tick_context, state_context.harvest_target, false, true, HarvestState::idle)
    }
}

impl Pickup {
    fn visualize(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        visualize_pickup(describe_data, &self.withdrawl);
        visualize_delivery_from(describe_data, &self.deposits, self.withdrawl.target().pos());
    }

    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        runtime_data.transfer_queue.register_pickup(&self.withdrawl, TransferType::Haul);

        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(&delivery_ticket, TransferType::Haul);
        }
    }

    fn tick(&mut self, _state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        let deposits = &self.deposits;

        tick_pickup(tick_context, &mut self.withdrawl, move || HarvestState::delivery(deposits.clone()))
    }
}

impl Delivery {
    fn visualize(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        visualize_delivery(describe_data, &self.deposits);
    }

    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(&delivery_ticket, TransferType::Haul);
        }
    }

    fn tick(&mut self, _state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        if let Some(consumed_energy) = tick_opportunistic_repair(tick_context, Some(RepairPriority::Medium)) {
            consume_resource_from_deposits(&mut self.deposits, ResourceType::Energy, consumed_energy);
        }

        tick_delivery(tick_context, &mut self.deposits, HarvestState::finished_delivery)
    }
}

impl FinishedDelivery {
    fn tick(&mut self, state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        let delivery_room_data = tick_context.system_data.room_data.get(state_context.delivery_room)?;

        let creep = tick_context.runtime_data.owner;

        ALL_TRANSFER_PRIORITIES
            .iter()
            .filter_map(|priority| {
                let transfer_queue_data = TransferQueueGeneratorData {
                    cause: "Harvest Finished Delivery",
                    room_data: &*tick_context.system_data.room_data,
                };

                get_new_delivery_current_resources_state(
                    creep,
                    &transfer_queue_data,
                    &[delivery_room_data],
                    priority.into(),
                    TransferTypeFlags::HAUL,
                    tick_context.runtime_data.transfer_queue,
                    HarvestState::delivery,
                )
            })
            .next()
            .or(Some(HarvestState::idle()))
    }
}

impl Build {
    fn tick(&mut self, _state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        tick_build(tick_context, self.target, HarvestState::finished_build)
    }
}

impl FinishedBuild {
    fn tick(&mut self, state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        let delivery_room_data = tick_context.system_data.room_data.get(state_context.delivery_room)?;

        let creep = tick_context.runtime_data.owner;

        get_new_build_state(creep, delivery_room_data, HarvestState::build).or(Some(HarvestState::idle()))
    }
}

impl Repair {
    fn tick(&mut self, _state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        tick_repair(tick_context, self.target, HarvestState::finished_repair)
    }
}

impl FinishedRepair {
    fn tick(&mut self, state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        let delivery_room_data = tick_context.system_data.room_data.get(state_context.delivery_room)?;

        let creep = tick_context.runtime_data.owner;

        get_new_repair_state(creep, delivery_room_data, Some(RepairPriority::Medium), HarvestState::repair).or(Some(HarvestState::idle()))
    }
}

impl Upgrade {
    fn tick(&mut self, _state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        tick_upgrade(tick_context, self.target, HarvestState::idle)
    }
}

impl MoveToRoom {
    fn tick(&mut self, _state_context: &mut HarvestJobContext, tick_context: &mut JobTickContext) -> Option<HarvestState> {
        tick_move_to_room(tick_context, self.room_name, HarvestState::idle)
    }
}

impl Wait {
    fn tick(&mut self, _state_context: &mut HarvestJobContext, _tick_context: &mut JobTickContext) -> Option<HarvestState> {
        tick_wait(&mut self.ticks, HarvestState::idle)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct HarvestJob {
    context: HarvestJobContext,
    state: HarvestState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl HarvestJob {
    pub fn new(harvest_target: RemoteObjectId<Source>, delivery_room: Entity, allow_haul: bool) -> HarvestJob {
        HarvestJob {
            context: HarvestJobContext {
                harvest_target,
                delivery_room,
                allow_haul,
            },
            state: HarvestState::idle(),
        }
    }

    pub fn harvest_target(&self) -> &RemoteObjectId<Source> {
        &self.context.harvest_target
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for HarvestJob {
    fn describe(&mut self, system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        self.state.describe(system_data, describe_data);
        self.state.visualize(system_data, describe_data);
    }

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        self.state.gather_data(system_data, runtime_data);
    }

    fn run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        let mut tick_context = JobTickContext {
            system_data,
            runtime_data,
            action_flags: SimultaneousActionFlags::UNSET,
        };

        while let Some(tick_result) = self.state.tick(&mut self.context, &mut tick_context) {
            self.state = tick_result
        }
    }
}
