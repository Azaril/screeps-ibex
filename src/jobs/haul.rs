use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::haulbehavior::*;
use super::utility::waitbehavior::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct HaulJobContext {
    pickup_rooms: EntityVec<Entity>,
    delivery_rooms: EntityVec<Entity>,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum HaulState {
        Idle,
        Pickup { withdrawl: TransferWithdrawTicket, deposits: Vec<TransferDepositTicket> },
        Delivery { deposits: Vec<TransferDepositTicket> },
        Wait { ticks: u32 }
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

        Idle, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}
        
        Idle, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}
        
        _ => fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState>;
    }
);

impl Idle {
    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        let creep = tick_context.runtime_data.owner;
        let pickup_rooms = state_context
            .pickup_rooms
            .iter()
            .filter_map(|e| tick_context.system_data.room_data.get(*e))
            .collect_vec();

        let delivery_rooms = state_context
            .delivery_rooms
            .iter()
            .filter_map(|e| tick_context.system_data.room_data.get(*e))
            .collect_vec();

        get_new_delivery_current_resources_state(
            creep,
            &delivery_rooms,
            TransferPriorityFlags::ACTIVE,
            TransferTypeFlags::HAUL,
            tick_context.runtime_data.transfer_queue,
            HaulState::delivery,
        )
        .or_else(|| {
            get_new_delivery_current_resources_state(
                creep,
                &delivery_rooms,
                TransferPriorityFlags::NONE,
                TransferTypeFlags::HAUL,
                tick_context.runtime_data.transfer_queue,
                HaulState::delivery,
            )
        })
        .or_else(|| {
            ACTIVE_TRANSFER_PRIORITIES
                .iter()
                .filter_map(|priority| {
                    get_new_pickup_and_delivery_full_capacity_state(
                        creep,
                        &pickup_rooms,
                        &delivery_rooms,
                        TransferPriorityFlags::from(priority),
                        TransferType::Haul,
                        tick_context.runtime_data.transfer_queue,
                        HaulState::pickup,
                    )
                })
                .next()
        })
        .or_else(|| Some(HaulState::wait(5)))
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

    fn tick(&mut self, _state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        let deposits = &self.deposits;

        tick_pickup(tick_context, &mut self.withdrawl, move || HaulState::delivery(deposits.clone()))
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

    fn tick(&mut self, _state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        tick_delivery(tick_context, &mut self.deposits, HaulState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &HaulJobContext, _tick_context: &mut JobTickContext) -> Option<HaulState> {
        tick_wait(&mut self.ticks, HaulState::idle)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct HaulJob {
    context: HaulJobContext,
    state: HaulState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl HaulJob {
    pub fn new(pickup_rooms: &[Entity], delivery_rooms: &[Entity]) -> HaulJob {
        HaulJob {
            context: HaulJobContext {
                pickup_rooms: pickup_rooms.into(),
                delivery_rooms: delivery_rooms.into()
            },
            state: HaulState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for HaulJob {
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
