use crate::findnearest::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;
#[cfg(feature = "time")]
use timing_annotate::*;

#[cfg_attr(feature = "time", timing)]
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

#[cfg_attr(feature = "time", timing)]
pub fn get_new_harvest_target_state<F, R>(creep: &Creep, source_id: &RemoteObjectId<Source>, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<Source>) -> R,
{
    if creep.store_free_capacity(Some(ResourceType::Energy)) > 0 && source_id.resolve().map(|s| s.energy() > 0).unwrap_or(true) {
        return Some(state_map(*source_id));
    }

    None
}

#[cfg_attr(feature = "time", timing)]
pub fn run_harvest_state<F, R>(creep: &Creep, source_id: &RemoteObjectId<Source>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep_pos = creep.pos();
    let target_position = source_id.pos();

    //TODO: Check visibility cache and cancel if not reachable etc.?

    if creep.store_free_capacity(Some(ResourceType::Energy)) == 0 {
        return Some(next_state());
    }

    if creep_pos.room_name() != target_position.room_name() {
        creep.move_to(&target_position);

        return None;
    }

    if let Some(source) = source_id.resolve() {
        if !creep.pos().is_near_to(&source) {
            creep.move_to(&source);

            return None;
        }

        match creep.harvest(&source) {
            ReturnCode::Ok => None,
            _ => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}
