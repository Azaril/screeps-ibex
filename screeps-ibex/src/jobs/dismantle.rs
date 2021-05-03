use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::dismantlebehavior::*;
use super::utility::haulbehavior::*;
use super::utility::movebehavior::*;
use super::utility::waitbehavior::*;
use crate::structureidentifier::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_machine::*;
use screeps_rover::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct DismantleJobContext {
    dismantle_room: Entity,
    delivery_room: Entity,
    ignore_storage: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum DismantleState {
        Idle,
        Dismantle { target: RemoteStructureIdentifier },
        FinishedDismantle,
        Delivery { deposits: Vec<TransferDepositTicket> },
        FinishedDelivery,
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

        Idle, Dismantle, FinishedDismantle, FinishedDelivery, MoveToRoom, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, Dismantle, FinishedDismantle, FinishedDelivery, MoveToRoom, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut DismantleJobContext, tick_context: &mut JobTickContext) -> Option<DismantleState>;
    }
);

impl Idle {
    fn tick(&mut self, state_context: &mut DismantleJobContext, tick_context: &mut JobTickContext) -> Option<DismantleState> {
        let dismantle_room_data = tick_context.system_data.room_data.get(state_context.dismantle_room)?;
        let delivery_room_data = tick_context.system_data.room_data.get(state_context.delivery_room)?;

        let creep = tick_context.runtime_data.owner;

        let in_dismantle_room = creep.room().map(|r| r.name() == dismantle_room_data.name).unwrap_or(false);

        if in_dismantle_room {
            if let Some(state) =
                get_new_dismantle_state(creep, &dismantle_room_data, tick_context.runtime_data.cost_matrix, state_context.ignore_storage, DismantleState::dismantle)
            {
                return Some(state);
            }
        }

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Dismantle Idle",
            room_data: &*tick_context.system_data.room_data,
        };

        get_new_delivery_current_resources_state(
            creep,
            &transfer_queue_data,
            &[delivery_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL,
            tick_context.runtime_data.transfer_queue,
            target_filters::all,
            DismantleState::delivery,
        )
        .or_else(|| {
            if creep.store().get_used_capacity(None) == 0 {
                get_new_move_to_room_state(creep, dismantle_room_data.name, DismantleState::move_to_room)
            } else {
                None
            }
        })
        .or_else(|| Some(DismantleState::wait(5)))
    }
}

impl Dismantle {
    fn tick(&mut self, _state_context: &mut DismantleJobContext, tick_context: &mut JobTickContext) -> Option<DismantleState> {
        tick_dismantle(tick_context, self.target, DismantleState::idle)
    }
}

impl FinishedDismantle {
    fn tick(&mut self, state_context: &mut DismantleJobContext, tick_context: &mut JobTickContext) -> Option<DismantleState> {
        let dismantle_room_data = tick_context.system_data.room_data.get(state_context.dismantle_room)?;

        get_new_dismantle_state(
            tick_context.runtime_data.owner,
            &dismantle_room_data,
            tick_context.runtime_data.cost_matrix,
            state_context.ignore_storage,
            DismantleState::dismantle,
        )
        .or_else(|| Some(DismantleState::idle()))
    }
}

impl Delivery {
    fn visualize(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        visualize_delivery(describe_data, &self.deposits);
    }

    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(&delivery_ticket);
        }
    }

    fn tick(&mut self, _state_context: &mut DismantleJobContext, tick_context: &mut JobTickContext) -> Option<DismantleState> {
        tick_delivery(tick_context, &mut self.deposits, DismantleState::finished_delivery)
    }
}

impl FinishedDelivery {
    fn tick(&mut self, state_context: &mut DismantleJobContext, tick_context: &mut JobTickContext) -> Option<DismantleState> {
        let delivery_room_data = tick_context.system_data.room_data.get(state_context.delivery_room)?;

        let creep = tick_context.runtime_data.owner;

        ALL_TRANSFER_PRIORITIES
            .iter()
            .filter_map(|priority| {
                let transfer_queue_data = TransferQueueGeneratorData {
                    cause: "Dismantle Finished Delivery",
                    room_data: &*tick_context.system_data.room_data,
                };

                get_new_delivery_current_resources_state(
                    creep,
                    &transfer_queue_data,
                    &[delivery_room_data],
                    priority.into(),
                    TransferTypeFlags::HAUL,
                    tick_context.runtime_data.transfer_queue,
                    target_filters::all,
                    DismantleState::delivery,
                )
            })
            .next()
            .or(Some(DismantleState::idle()))
    }
}

impl MoveToRoom {
    fn tick(&mut self, _state_context: &mut DismantleJobContext, tick_context: &mut JobTickContext) -> Option<DismantleState> {
        tick_move_to_room(
            tick_context,
            self.room_name,
            Some(RoomOptions::new(HostileBehavior::HighCost)),
            DismantleState::idle,
        )
    }
}

impl Wait {
    fn tick(&mut self, _state_context: &mut DismantleJobContext, _tick_context: &mut JobTickContext) -> Option<DismantleState> {
        tick_wait(&mut self.ticks, DismantleState::idle)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct DismantleJob {
    context: DismantleJobContext,
    state: DismantleState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl DismantleJob {
    pub fn new(dismantle_room: Entity, delivery_room: Entity, ignore_storage: bool) -> DismantleJob {
        DismantleJob {
            context: DismantleJobContext {
                dismantle_room,
                delivery_room,
                ignore_storage,
            },
            state: DismantleState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for DismantleJob {
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
