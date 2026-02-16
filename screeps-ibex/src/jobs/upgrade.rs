use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::controllerbehavior::*;
use super::utility::harvestbehavior::*;
use super::utility::haulbehavior::*;
use super::utility::movebehavior::*;
use super::utility::waitbehavior::*;
use crate::constants::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::transfer::transfersystem::*;
use screeps::*;
use screeps_machine::*;
use serde::{Deserialize, Serialize};
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct UpgradeJobContext {
    home_room: Entity,
}

/// A creep is considered slow when it has fewer than 1 MOVE part per 4 total
/// parts. The RCL > 3 upgrader body (`[W, C, M, M] + N*[W]`) hits this
/// threshold at 5+ parts. Slow creeps should stay near the controller and
/// rely on haulers for energy delivery.
fn is_slow_creep(creep: &Creep) -> bool {
    let body = creep.body();
    let total_parts = body.len();
    let move_parts = body.iter().filter(|p| p.part() == Part::Move).count();
    total_parts > 4 && move_parts * 4 < total_parts
}

/// Decide at runtime whether this upgrader should be allowed to harvest from
/// sources. Fast creeps (RCL <= 3 style bodies) always harvest. Slow creeps
/// only harvest when the room lacks delivery infrastructure (no storage and
/// no containers), which covers downgrade emergencies and room recovery
/// scenarios where haulers cannot deliver energy.
fn should_allow_harvest(creep: &Creep, room_data: &RoomData) -> bool {
    if !is_slow_creep(creep) {
        return true;
    }
    let structures = match room_data.get_structures() {
        Some(s) => s,
        None => return true,
    };
    structures.storages().is_empty() && structures.containers().is_empty()
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum UpgradeState {
        Idle,
        Harvest { target: RemoteObjectId<Source> },
        Pickup { ticket: TransferWithdrawTicket },
        FinishedPickup,
        Sign { target: RemoteObjectId<StructureController> },
        Upgrade { target: RemoteObjectId<StructureController> },
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        Idle, Harvest, FinishedPickup, Sign, Upgrade, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, Harvest, FinishedPickup, Sign, Upgrade, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState>;
    }
);

impl Idle {
    pub fn tick(&mut self, state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        let home_room_data = tick_context.system_data.room_data.get(state_context.home_room)?;
        let creep = tick_context.runtime_data.owner;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Upgrade Idle",
            room_data: tick_context.system_data.room_data,
        };

        let slow = is_slow_creep(creep);
        let max_range = if slow { Some(5) } else { None };

        get_new_nearby_pickup_state_fill_resource(
            creep,
            &transfer_queue_data,
            &[home_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            tick_context.runtime_data.transfer_queue,
            max_range,
            UpgradeState::pickup,
        )
        .or_else(|| {
            if should_allow_harvest(creep, home_room_data) {
                get_new_harvest_state(creep, home_room_data, UpgradeState::harvest)
            } else {
                None
            }
        })
        .or_else(|| get_new_sign_state(home_room_data, UpgradeState::sign))
        .or_else(|| get_new_upgrade_state(creep, home_room_data, UpgradeState::upgrade, None))
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
            UpgradeState::finished_pickup,
        )
    }

    pub fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}
}

impl FinishedPickup {
    pub fn tick(&self, state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        let home_room_data = tick_context.system_data.room_data.get(state_context.home_room)?;
        let creep = tick_context.runtime_data.owner;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Upgrade Finished Pickup",
            room_data: tick_context.system_data.room_data,
        };

        let slow = is_slow_creep(creep);
        let max_range = if slow { Some(5) } else { None };

        get_new_nearby_pickup_state_fill_resource(
            creep,
            &transfer_queue_data,
            &[home_room_data],
            TransferPriorityFlags::ALL,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            ResourceType::Energy,
            tick_context.runtime_data.transfer_queue,
            max_range,
            UpgradeState::pickup,
        )
        .or_else(|| Some(UpgradeState::idle()))
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
    pub fn tick(&mut self, _state_context: &UpgradeJobContext, tick_context: &mut JobTickContext) -> Option<UpgradeState> {
        mark_idle(tick_context);
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
    pub fn new(home_room: Entity) -> UpgradeJob {
        UpgradeJob {
            context: UpgradeJobContext { home_room },
            state: UpgradeState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for UpgradeJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Upgrade - {}", self.state.status_description()))
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
