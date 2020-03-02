use super::build::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;

pub fn get_new_build_state<F, R>(creep: &Creep, build_room: &RoomData, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<ConstructionSite>) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        if let Some(room) = game::rooms::get(build_room.name) {
            if let Some(construction_site) = select_construction_site(&creep, &room) {
                return Some(state_map(construction_site.remote_id()));
            }
        }
    }

    None
}

pub fn run_build_state<F, R>(creep: &Creep, construction_site_id: &RemoteObjectId<ConstructionSite>, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let creep_pos = creep.pos();
    let target_position = construction_site_id.pos();

    //TODO: Check visibility cache and cancel if construction site doesn't exist?

    if creep_pos.room_name() != target_position.room_name() {
        creep.move_to(&target_position);

        return None;
    }

    if let Some(construction_site) = construction_site_id.resolve() {
        if !creep_pos.in_range_to(&construction_site, 3) {
            creep.move_to(&target_position);

            return None;
        }

        match creep.build(&construction_site) {
            ReturnCode::Ok => None,
            _ => Some(next_state()),
        }
    } else {
        Some(next_state())
    }
}
