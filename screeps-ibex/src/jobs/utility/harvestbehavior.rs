use crate::findnearest::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use crate::store::*;
use screeps::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_harvest_state<F, R>(creep: &Creep, harvest_room_data: &RoomData, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<Source>) -> R,
{
    let available_capacity = creep.store().get_free_capacity(Some(ResourceType::Energy));

    if available_capacity > 0 {
        let source = harvest_room_data
            .get_static_visibility_data()
            .and_then(|d| d.sources().iter().find_nearest_linear_by(creep.pos(), |s| s.pos()));

        if let Some(source) = source {
            return Some(state_map(*source));
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_harvest_target_state<F, R>(
    creep: &Creep,
    source_id: &RemoteObjectId<Source>,
    ignore_free_capacity: bool,
    state_map: F,
) -> Option<R>
where
    F: Fn(RemoteObjectId<Source>) -> R,
{
    //TODO: Does it make sense to actually check for energy being available here? Reduces locomotion time towards it. Look at distance vs regen ticks?
    if (ignore_free_capacity || creep.store().get_free_capacity(Some(ResourceType::Energy)) > 0)
        && source_id.resolve().map(|s| s.energy() > 0).unwrap_or(true)
    {
        return Some(state_map(*source_id));
    }

    None
}

pub trait HarvestableResource {
    fn get_harvestable_amount(&self) -> u32;
}

impl HarvestableResource for Source {
    fn get_harvestable_amount(&self) -> u32 {
        self.energy()
    }
}

impl HarvestableResource for Mineral {
    fn get_harvestable_amount(&self) -> u32 {
        self.mineral_amount()
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_harvest<T, F, R>(
    tick_context: &mut JobTickContext,
    target_id: RemoteObjectId<T>,
    ignore_creep_capacity: bool,
    optimistic_completion: bool,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
    T: Harvestable + HasId + HarvestableResource + wasm_bindgen::JsCast,
{
    let creep = tick_context.runtime_data.owner;
    let action_flags = &mut tick_context.action_flags;

    //TODO: Check visibility cache and cancel if not reachable etc.?

    if !ignore_creep_capacity
        && creep.expensive_store_free_capacity() == 0 && !action_flags.contains(SimultaneousActionFlags::TRANSFER) {
            return Some(next_state());
        }

    let target_position = target_id.pos();

    if !creep.pos().is_near_to(target_position) {
        if action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(1);
        }

        return None;
    }

    if let Some(harvest_target) = target_id.resolve() {
        if action_flags.consume(SimultaneousActionFlags::HARVEST) {
            match creep.harvest(&harvest_target) {
                Ok(()) => {
                    if optimistic_completion {
                        let body = creep.body();
                        let work_parts = body.iter().filter(|b| b.part() == Part::Work).count();
                        let harvest_amount = (work_parts as u32 * HARVEST_POWER).min(harvest_target.get_harvestable_amount());

                        if harvest_amount as i32 >= creep.store().get_free_capacity(Some(ResourceType::Energy)) {
                            Some(next_state())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(_) => Some(next_state()),
            }
        } else {
            None
        }
    } else {
        Some(next_state())
    }
}
