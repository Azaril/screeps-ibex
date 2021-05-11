use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::controllerbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::waitbehavior::*;
use crate::constants::*;
use crate::remoteobjectid::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeJobContext {
    home_room: Entity,
    allow_harvest: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum UpgradeState {
        Idle,
        Harvest { target: RemoteObjectId<Source> },
        Pickup { ticket: TransferWithdrawTicket },
        Sign { target: RemoteObjectId<StructureController> },
        Upgrade { target: RemoteObjectId<StructureController> },
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

        Idle, Harvest, Sign, Upgrade, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, Harvest, Sign, Upgrade, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState>;
    }
);

impl Idle {
    pub fn tick(&mut self, state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        let home_room_data = tick_context.system_data.room_data.get(state_context.home_room)?;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Upgrade Idle",
            room_data: &*tick_context.system_data.room_data,
        };

        //TODO: Only allow long movement if set up for it.
        //TODO: wiarchbe: Need transfer pickup filter for range.
        let (move_parts, non_move_parts) = tick_context.runtime_data.owner.body()
            .iter()
            .fold((0, 0), |(m, nm), part| {
                if part.part() == Part::Move {
                    (m + 1, nm)
                } else {
                    (m, nm + 1)
                }
            });

        let allow_movement = move_parts >= non_move_parts;

        let creep_pos = tick_context.runtime_data.owner.pos();

        let pickup_filter = |target: &TransferTarget| {
            if !allow_movement {
                creep_pos.get_range_to(target.pos()) <= 5
            } else {
                true
            }
        };

        get_new_pickup_state_fill_resource(
            &tick_context.runtime_data.owner,
            &transfer_queue_data,
            &[home_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            tick_context.runtime_data.transfer_queue,
            pickup_filter,
            UpgradeState::pickup,
        )
        .or_else(|| {
            if state_context.allow_harvest {
                get_new_harvest_state(&tick_context.runtime_data.owner, home_room_data, UpgradeState::harvest)
            } else {
                None
            }
        })
        .or_else(|| get_new_sign_state(home_room_data, UpgradeState::sign))
        .or_else(|| get_new_upgrade_state(&tick_context.runtime_data.owner, home_room_data, UpgradeState::upgrade, None))
        .or_else(|| Some(UpgradeState::wait(5)))
    }
}

impl Harvest {
    pub fn tick(&mut self, _state_context: &mut UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        tick_harvest(tick_context, self.target, false, false, UpgradeState::idle)
    }
}

impl Pickup {
    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        runtime_data.transfer_queue.register_pickup(&self.ticket);
    }

    pub fn tick(&mut self, _state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        tick_pickup_and_fill(
            tick_context,
            &mut self.ticket,
            ResourceType::Energy,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            TransferPriorityFlags::ALL,
            UpgradeState::idle,
        )
    }

    pub fn visualize(&self, _system_data: &JobExecutionSystemData, describe_data: &mut JobDescribeData) {
        visualize_pickup(describe_data, &self.ticket);
    }
}

impl Sign {
    pub fn tick(&mut self, _state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        tick_sign(tick_context, self.target, ROOM_SIGN, UpgradeState::idle)
    }
}

impl Upgrade {
    pub fn tick(&mut self, _state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        tick_upgrade(tick_context, self.target, UpgradeState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &UpgradeJobContext, _tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        tick_wait(&mut self.ticks, UpgradeState::idle)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeJob {
    context: UpgradeJobContext,
    state: UpgradeState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl UpgradeJob {
    pub fn new(home_room: Entity, allow_harvest: bool) -> UpgradeJob {
        UpgradeJob {
            context: UpgradeJobContext { home_room, allow_harvest },
            state: UpgradeState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for UpgradeJob {
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
