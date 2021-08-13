use super::{actions::*, data::JobData};
use super::context::*;
use super::jobsystem::*;
use super::utility::haulbehavior::*;
use super::utility::movebehavior::*;
use super::utility::repair::*;
use super::utility::repairbehavior::*;
use super::utility::waitbehavior::*;
use crate::{entitymappingsystem::EntityMappingData, room::data::RoomData, transfer::transfersystem::*};
use crate::{serialize::*, store::HasExpensiveStore};
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

        Idle, MoveToRoom, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, MoveToRoom, Wait => fn pre_run_job(&self, _state_context: &mut HaulJobContext, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

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
            room_data: &*tick_context.system_data.room_data,
        };

        let room_mapping = tick_context.runtime_data.mapping;
        let room_data_system = tick_context.system_data.room_data;

        let delivery_filter = |target: &TransferTarget| {
            if state_context.storage_delivery_only {
                let target_room = target.pos().room_name();
                if let Some(target_room) = room_mapping.get_room(&target_room) {
                    let has_storage = room_data_system
                        .get(target_room)
                        .and_then(|r| r.get_structures())
                        .map(|r| !r.storages().is_empty())
                        .unwrap_or(false);

                    return match target {
                        TransferTarget::Container(_) => !has_storage,
                        TransferTarget::Storage(_) => true,
                        TransferTarget::Terminal(_) => true,
                        _ => false,
                    };
                }
            }

            true
        };

        get_new_delivery_current_resources_state(
            creep,
            &transfer_queue_data,
            &delivery_rooms,
            TransferPriorityFlags::ACTIVE,
            TransferTypeFlags::HAUL,
            tick_context.runtime_data.transfer_queue,
            delivery_filter,
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
                delivery_filter,
                HaulState::delivery,
            )
        })
        .or_else(|| {
            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Haul Idle",
                room_data: &*tick_context.system_data.room_data,
            };

            get_new_pickup_and_delivery_state(
                creep,
                &transfer_queue_data,
                &pickup_rooms,
                &delivery_rooms,
                TransferPriorityFlags::ALL,
                TransferPriorityFlags::ALL,
                10,
                TransferType::Haul,
                TransferCapacity::Finite(creep.store().expensive_free_capacity()),
                tick_context.runtime_data.transfer_queue,
                target_filters::all,
                delivery_filter,
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
    fn visualize(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        visualize_pickup(describe_data, &self.withdrawl);
        visualize_delivery_from(describe_data, &self.deposits, self.withdrawl.target().pos());
    }

    fn pre_run_job(&self, state_context: &mut HaulJobContext, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        //
        // Register existing pickup and delivery.
        //

        runtime_data.transfer_queue.register_pickup(&self.withdrawl);

        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(&delivery_ticket);
        }

        //
        // Get additional delivery
        //

        let creep_entity = runtime_data.creep_entity;
        let creep = runtime_data.owner;
        //TODO: Fix this when double resource counting bug is fixed.
        let free_capacity = creep.store().expensive_free_capacity();

        let mut available_capacity = TransferCapacity::Finite(free_capacity);

        for entries in self.withdrawl.resources().values() {
            for entry in entries {
                available_capacity.consume(entry.amount());
            }
        }

        if !available_capacity.empty() {
            let delivery_rooms = state_context.delivery_rooms.clone();
            let storage_delivery_only = state_context.storage_delivery_only;

            system_data.updater.exec_mut(move |world| {
                let room_data = world.read_storage::<RoomData>();

                let delivery_rooms = delivery_rooms
                    .iter()
                    .filter_map(|e| room_data.get(*e))
                    .collect_vec();

                let room_mapping = world.read_resource::<EntityMappingData>();

                let target_filter = |target: &TransferTarget| {
                    if storage_delivery_only {
                        let target_room = target.pos().room_name();
                        if let Some(target_room) = room_mapping.get_room(&target_room) {
                            let has_storage = room_data
                                .get(target_room)
                                .and_then(|r| r.get_structures())
                                .map(|r| !r.storages().is_empty())
                                .unwrap_or(false);

                            return match target {
                                TransferTarget::Container(_) => !has_storage,
                                TransferTarget::Storage(_) => true,
                                TransferTarget::Terminal(_) => true,
                                _ => false,
                            };
                        }
                    }

                    true
                };

                let mut transfer_queue = world.write_resource::<TransferQueue>();

                let transfer_queue_data = TransferQueueGeneratorData {
                    cause: "Pickup Tick",
                    room_data: &room_data,
                };                

                let mut job_data = world.write_storage::<JobData>();

                if let Some(JobData::Haul(haul_job)) = job_data.get_mut(creep_entity) {
                    if let HaulState::Pickup(pickup_state) = &mut haul_job.state {
                        get_additional_deliveries(
                            &transfer_queue_data,
                            &delivery_rooms,
                            TransferPriorityFlags::ALL,
                            TransferType::Haul,
                            available_capacity,
                            &mut *transfer_queue,
                            &mut pickup_state.withdrawl,
                            &mut pickup_state.deposits,
                            target_filter,
                            10,
                        );
                    }
                }
            });
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

    fn pre_run_job(&self, _state_context: &mut HaulJobContext, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        for delivery_ticket in self.deposits.iter() {
            runtime_data.transfer_queue.register_delivery(&delivery_ticket);
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
    fn describe(&mut self, system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        self.state.describe(system_data, describe_data);
        self.state.visualize(system_data, describe_data);
    }

    fn pre_run_job(&mut self, system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        self.state.pre_run_job(&mut self.context, system_data, runtime_data);
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
