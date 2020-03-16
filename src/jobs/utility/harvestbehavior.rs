use crate::findnearest::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;
use crate::jobs::actions::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_harvest_state<F, R>(creep: &Creep, harvest_room_data: &RoomData, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<Source>) -> R,
{
    let available_capacity = creep.store_free_capacity(Some(ResourceType::Energy));

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
pub fn get_new_harvest_target_state<F, R>(creep: &Creep, source_id: &RemoteObjectId<Source>, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<Source>) -> R,
{
    //TODO: Does it make sense to actually check for energy being available here? Reduces locomotion time towards it. Look at distance vs regen ticks?
    if creep.store_free_capacity(Some(ResourceType::Energy)) > 0 && source_id.resolve().map(|s| s.energy() > 0).unwrap_or(true) {
        return Some(state_map(*source_id));
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn run_harvest_state<F, R>(creep: &Creep, action_flags: &mut SimultaneousActionFlags, source_id: &RemoteObjectId<Source>, optimistic_completion: bool, stuck_count: &mut u8, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep_pos = creep.pos();
    let target_position = source_id.pos();

    //TODO: Check visibility cache and cancel if not reachable etc.?

    if creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
        if action_flags.contains(SimultaneousActionFlags::TRANSFER) {
            return None;
        } else {
            return Some(next_state());
        }
    }

    if creep_pos.room_name() != target_position.room_name() {
        if !action_flags.contains(SimultaneousActionFlags::MOVE) {
            action_flags.insert(SimultaneousActionFlags::MOVE);
            match creep.move_to(&target_position) {
                ReturnCode::NoPath => {
                    *stuck_count += 1;

                    if *stuck_count > 5 {
                        return Some(next_state());
                    }
                },
                _ => {}
            }
        }

        return None;
    }

    if let Some(source) = source_id.resolve() {
        if !creep.pos().is_near_to(&source) {
            if !action_flags.contains(SimultaneousActionFlags::MOVE) {
                action_flags.insert(SimultaneousActionFlags::MOVE);
                match creep.move_to(&target_position) {
                    ReturnCode::NoPath => {
                        *stuck_count += 1;
    
                        if *stuck_count > 5 {
                            return Some(next_state());
                        }
                    },
                    _ => {}
                }
            }

            return None;
        }

        if !action_flags.contains(SimultaneousActionFlags::HARVEST) {
            action_flags.insert(SimultaneousActionFlags::HARVEST);

            match creep.harvest(&source) {
                ReturnCode::Ok => if optimistic_completion {
                    let body = creep.body();
                    let work_parts = body.iter().filter(|b| b.part == Part::Work).count();
                    let harvest_amount = (work_parts as u32 * HARVEST_POWER).min(source.energy());

                    if harvest_amount >= creep.store_free_capacity(Some(ResourceType::Energy)) {
                        Some(next_state())
                    } else {
                        None
                    }                    
                } else {
                    None
                },
                _ => Some(next_state()),
            }
        } else {
            None
        }
    } else {
        Some(next_state())
    }
}
