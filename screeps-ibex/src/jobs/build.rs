use super::actions::*;
use super::context::*;
use super::jobsystem::*;
use super::utility::buildbehavior::*;
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
#[allow(deprecated)]
use specs::error::NoError;
use specs::saveload::*;
use specs::*;

#[derive(Clone, ConvertSaveload)]
pub struct BuildJobContext {
    home_room: Entity,
    build_room: Entity,
    allow_harvest: bool,
}

machine!(
    #[derive(Clone, Serialize, Deserialize)]
    enum BuildState {
        Idle,
        Pickup { ticket: TransferWithdrawTicket },
        FinishedPickup,
        Harvest { target: RemoteObjectId<Source> },
        Build { target: RemoteObjectId<ConstructionSite> },
        Repair { target: RemoteStructureIdentifier },
        Wait { ticks: u32 }
    }

    impl {
        * => fn describe(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        * => fn status_description(&self) -> String {
            std::any::type_name::<Self>().to_string()
        }

        Idle, FinishedPickup, Harvest, Build, Repair, Wait => fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}

        Idle, FinishedPickup, Harvest, Build, Repair, Wait => fn gather_data(&self, _system_data: &JobExecutionSystemData, _runtime_data: &mut JobExecutionRuntimeData) {}

        _ => fn tick(&mut self, state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState>;
    }
);

impl Idle {
    pub fn tick(&mut self, state_context: &BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        let creep = tick_context.runtime_data.owner;
        let build_room_data = tick_context.system_data.room_data.get(state_context.build_room)?;

        // The S1 repair stress gate was deleted at ADR 0040 M5a — the sink
        // market prices repair admission natively, so the builder uses its own
        // minimum repair priorities directly.
        get_new_repair_state(
            creep,
            build_room_data,
            tick_context.system_data.repair_queue,
            Some(RepairPriority::High),
            BuildState::repair,
        )
        .or_else(|| get_new_build_state(creep, build_room_data, BuildState::build))
        .or_else(|| {
            get_new_repair_state(
                creep,
                build_room_data,
                tick_context.system_data.repair_queue,
                None,
                BuildState::repair,
            )
        })
        .or_else(|| {
            // ADR 0044 A3 (Defect 2) — gate the builder self-fetch pickup on the floor so builders
            // too shed their Use-lane draw under a refill deficit (the doc's "gate the build
            // self-fetch pickup" completeness patch). Admit on the best pending build site's
            // per-class bid, on any pending repair target, or on an unpriced-class site.
            if !builder_self_fetch_admitted(creep, build_room_data, state_context, tick_context) {
                return None;
            }

            let transfer_queue_data = TransferQueueGeneratorData {
                cause: "Build Idle",
                room_data: tick_context.system_data.room_data,
            };

            get_new_pickup_state_fill_resource(
                creep,
                &transfer_queue_data,
                &[build_room_data],
                TransferPriorityFlags::ALL,
                TransferTypeFlags::HAUL | TransferTypeFlags::USE,
                ResourceType::Energy,
                tick_context.runtime_data.transfer_queue,
                BuildState::pickup,
            )
        })
        .or_else(|| {
            if state_context.allow_harvest {
                get_new_harvest_state(creep, build_room_data, BuildState::harvest)
            } else {
                None
            }
        })
        .or_else(|| Some(BuildState::wait(5)))
    }
}

/// ADR 0044 A3 (Defect 2) — whether the builder's Use-lane self-fetch is admitted this tick: its
/// best pending downstream sink (build site class bid or a pending repair target) clears the room's
/// opportunity floor. Reuses the shared consumer-admission gate (the same `admit_use_withdraw` the
/// upgrader and the sim's builder loop use). Only reached when the builder has no energy to build/
/// repair with (the pickup slot of the Idle/FinishedPickup cascade).
fn builder_self_fetch_admitted(
    creep: &Creep,
    build_room_data: &crate::room::data::RoomData,
    state_context: &BuildJobContext,
    tick_context: &JobTickContext,
) -> bool {
    use crate::jobs::utility::build::select_construction_site;
    use crate::jobs::utility::consumer_admission::builder_withdraw_admitted;
    use crate::jobs::utility::repair::select_repair_structure;

    let current_rcl = build_room_data
        .get_structures()
        .iter()
        .flat_map(|s| s.controllers())
        .map(|c| c.level())
        .max()
        .unwrap_or(0);
    let best_site_type = build_room_data
        .get_construction_sites()
        .and_then(|sites| select_construction_site(creep, &sites, current_rcl as u32))
        .map(|s| s.structure_type());
    // A repair target that the builder's repair selection would pick (any minimum priority) clears
    // the floor by construction (it passed the repair selection's own gate / survival override).
    let has_repair_target = select_repair_structure(build_room_data, tick_context.system_data.repair_queue, None, true).is_some();

    // Address `state_context`'s build-room binding is already reflected in `build_room_data`.
    let _ = state_context;
    builder_withdraw_admitted(build_room_data, tick_context.system_data.market_bids, best_site_type, has_repair_target)
}

impl Pickup {
    fn gather_data(&self, _system_data: &JobExecutionSystemData, runtime_data: &mut JobExecutionRuntimeData) {
        runtime_data.transfer_queue.register_pickup(&self.ticket);
    }

    pub fn tick(&mut self, _state_context: &BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_pickup_and_fill(
            tick_context,
            &mut self.ticket,
            ResourceType::Energy,
            TransferTypeFlags::HAUL | TransferTypeFlags::USE,
            TransferPriorityFlags::ALL,
            BuildState::finished_pickup,
        )
    }

    pub fn visualize(&self, _system_data: &JobExecutionSystemData, _describe_data: &mut JobDescribeData) {}
}

impl FinishedPickup {
    pub fn tick(&self, state_context: &BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        let build_room_data = tick_context.system_data.room_data.get(state_context.build_room)?;

        let transfer_queue_data = TransferQueueGeneratorData {
            cause: "Build Finished Pickup",
            room_data: tick_context.system_data.room_data,
        };

        // ADR 0044 A3 (Defect 2) — same Use-lane admission on the continuation self-fetch.
        let pickup = if builder_self_fetch_admitted(tick_context.runtime_data.owner, build_room_data, state_context, tick_context) {
            get_new_pickup_state_fill_resource(
                tick_context.runtime_data.owner,
                &transfer_queue_data,
                &[build_room_data],
                TransferPriorityFlags::ALL,
                TransferTypeFlags::HAUL | TransferTypeFlags::USE,
                ResourceType::Energy,
                tick_context.runtime_data.transfer_queue,
                BuildState::pickup,
            )
        } else {
            None
        };

        pickup.or_else(|| Some(BuildState::idle()))
    }
}

impl Harvest {
    pub fn tick(&mut self, _state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_harvest(tick_context, self.target, false, false, BuildState::idle)
    }
}

impl Build {
    pub fn tick(&mut self, _state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_build(tick_context, self.target, BuildState::idle)
    }
}

impl Repair {
    pub fn tick(&mut self, state_context: &mut BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        tick_repair(tick_context, self.target, Some(state_context.build_room), BuildState::idle)
    }
}

impl Wait {
    pub fn tick(&mut self, _state_context: &BuildJobContext, tick_context: &mut JobTickContext) -> Option<BuildState> {
        mark_idle(tick_context);
        tick_wait(&mut self.ticks, BuildState::idle)
    }
}

#[derive(Clone, ConvertSaveload)]
pub struct BuildJob {
    context: BuildJobContext,
    state: BuildState,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl BuildJob {
    pub fn new(home_room: Entity, build_room: Entity, allow_harvest: bool) -> BuildJob {
        BuildJob {
            context: BuildJobContext {
                home_room,
                build_room,
                allow_harvest,
            },
            state: BuildState::idle(),
        }
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl Job for BuildJob {
    fn summarize(&self) -> crate::visualization::SummaryContent {
        crate::visualization::SummaryContent::Text(format!("Build - {}", self.state.status_description()))
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

        crate::machine_tick::run_state_machine(&mut self.state, "BuildJob", |state| {
            state.tick(&mut self.context, &mut tick_context)
        });
    }
}
