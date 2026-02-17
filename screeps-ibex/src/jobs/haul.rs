use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::haulbehavior::*;
use super::utility::movebehavior::*;
use super::utility::repair::*;
use super::utility::repairbehavior::*;
use super::utility::waitbehavior::*;
use crate::serialize::*;
use crate::transfer::transfersystem::*;
use itertools::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct HaulJobContext {
    pickup_rooms: EntityVec<Entity>,
    delivery_rooms: EntityVec<Entity>,
    allow_repair: bool,
    storage_delivery_only: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum HaulState {
        Idle,
        Pickup { withdrawl: TransferWithdrawTicket, deposits: Vec<TransferDepositTicket> },
        Delivery { deposits: Vec<TransferDepositTicket> },
        Wait { ticks: u32 },
        MoveToRoom { room_name: RoomName },
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        Idle, MoveToRoom, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, MoveToRoom, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

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

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Haul Idle",
            room_data: tick_context.system_data.room_data,
        };

        let target_filter = if state_context.storage_delivery_only {
            target_filters::storage
        } else {
            target_filters::all
        };

        get_new_delivery_current_resources_state(
            creep,
            &transfer_queue_data,
            &delivery_rooms,
            TransferPriorityFlags::ACTIVE,
            TransferTypeFlags::HAUL,
            tick_context.runtime_data.transfer_queue,
            target_filter,
            HaulState::delivery,
        )
        .or_else(|| {
            get_new_delivery_current_resources_state(
                creep,
                &transfer_queue_data,
                &delivery_rooms,
                TransferPriorityFlags::NONE,
                TransferTypeFlags::HAUL,
                tick_context.runtime_data.transfer_queue,
                target_filter,
                HaulState::delivery,
            )
        })
        .or_else(|| {
            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Haul Idle",
                room_data: tick_context.system_data.room_data,
            };

            get_new_pickup_and_delivery_full_capacity_state(
                creep,
                &transfer_queue_data,
                &pickup_rooms,
                &delivery_rooms,
                TransferPriorityFlags::ALL,
                TransferPriorityFlags::ALL,
                10,
                TransferType::Haul,
                tick_context.runtime_data.transfer_queue,
                target_filter,
                HaulState::pickup,
            )
        })
        .or_else(|| {
            for room in &pickup_rooms {
                if room.get_dynamic_visibility_data().map(|v| !v.visible()).unwrap_or(true) {
                    if let Some(state) = get_new_move_to_room_state(creep, room.name, HaulState::move_to_room) {
                        return Some(state);
                    }
                }
            }

            None
        })
        .or_else(|| Some(HaulState::wait(5)))
    }
}

impl Pickup {
    fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        runtime_data.transfer_queue.register_pickup(&self.withdrawl);

        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(delivery_ticket);
        }
    }

    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        //
        // NOTE: All haulers run this at the same time so that transfer data is only hydrated on this tick.
        //

        if game::time().is_multiple_of(5) {
            let creep = tick_context.runtime_data.owner;

            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Pickup Tick",
                room_data: tick_context.system_data.room_data,
            };

            let delivery_rooms = state_context
                .delivery_rooms
                .iter()
                .filter_map(|e| tick_context.system_data.room_data.get(*e))
                .collect_vec();

            let capacity = creep.store().get_capacity(None);
            let store_types = creep.store().store_types();
            let used_capacity = store_types.iter().map(|r| creep.store().get_used_capacity(Some(*r))).sum::<u32>();
            //TODO: Fix this when double resource counting bug is fixed.
            //let used_capacity = creep.store().get_used_capacity(None);
            let free_capacity = capacity - used_capacity;

            let mut available_capacity = TransferCapacity::Finite(free_capacity);

            for entries in self.withdrawl.resources().values() {
                for entry in entries {
                    available_capacity.consume(entry.amount());
                }
            }

            let target_filter = if state_context.storage_delivery_only {
                target_filters::storage
            } else {
                target_filters::all
            };

            get_additional_deliveries(
                &transfer_queue_data,
                &delivery_rooms,
                TransferPriorityFlags::ALL,
                TransferType::Haul,
                available_capacity,
                tick_context.runtime_data.transfer_queue,
                &mut self.withdrawl,
                &mut self.deposits,
                target_filter,
                10,
            );
        }

        let deposits = &self.deposits;

        tick_pickup(tick_context, &mut self.withdrawl, move || HaulState::delivery(deposits.clone()))
    }
}

impl Delivery {
    fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(delivery_ticket);
        }
    }

    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        if state_context.allow_repair {
            if let Some(consumed_energy) = tick_opportunistic_repair(tick_context, Some(RepairPriority::Low)) {
                consume_resource_from_deposits(&mut self.deposits, ResourceType::Energy, consumed_energy);
            }
        }

        tick_delivery(tick_context, &mut self.deposits, HaulState::idle)
    }
}

impl MoveToRoom {
    fn tick(&mut self, state_context: &mut HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        if state_context.allow_repair {
            tick_opportunistic_repair(tick_context, Some(RepairPriority::Low));
        }

        tick_move_to_room(tick_context, self.room_name, None, HaulState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &HaulJobContext, tick_context: &mut JobTickContext) -> Option<HaulState> {
        mark_idle(tick_context);
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
    pub fn new(pickup_rooms: &[Entity], delivery_rooms: &[Entity], allow_repair: bool, storage_delivery_only: bool) -> HaulJob {
        HaulJob {
            context: HaulJobContext {
                pickup_rooms: pickup_rooms.into(),
                delivery_rooms: delivery_rooms.into(),
                allow_repair,
                storage_delivery_only,
            },
            state: HaulState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for HaulJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Haul - {}", self.state.status_description()))
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

        crate::machine_tick::run_state_machine(&mut self.state, "HaulJob", |state| {
            state.tick(&mut self.context, &mut tick_context)
        });
    }
}
