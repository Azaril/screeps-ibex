use super::build::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::remoteobjectid::*;
use crate::room::data::*;
use screeps::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_build_state<F, R>(creep: &Creep, build_room: &RoomData, state_map: F) -> Option<R>
where
    F: Fn(RemoteObjectId<ConstructionSite>) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        //TODO: This requires visibility and could fail?
        if let Some(construction_site) = build_room.get_construction_sites().and_then(|construction_sites| select_construction_site(&creep, &construction_sites)) {
            return Some(state_map(construction_site.remote_id()));
        }
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_build<F, R>(
    tick_context: &mut JobTickContext,
    construction_site_id: RemoteObjectId<ConstructionSite>,
    next_state: F,
) -> Option<R>
where
    F: Fn() -> R,
{
    let target_position = construction_site_id.pos();

    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let expect_resolve = if creep_pos.room_name() == target_position.room_name() {
        true
    } else {
        let target_room_entity = tick_context.runtime_data.mapping.get_room(&target_position.room_name())?;
        let target_room_data = tick_context.system_data.room_data.get(target_room_entity)?;

        target_room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false)
    };

    let construction_site = construction_site_id.resolve();

    if expect_resolve && construction_site.is_none() {
        return Some(next_state());
    }

    if !creep_pos.in_range_to(&target_position, 3) {
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(3);
        }

        return None;
    }

    if let Some(construction_site) = construction_site {
        if tick_context.action_flags.consume(SimultaneousActionFlags::BUILD) {
            match creep.build(&construction_site) {
                ReturnCode::Ok => None,
                _ => Some(next_state()),
            }
        } else {
            None
        }
    } else {
        Some(next_state())
    }
}
