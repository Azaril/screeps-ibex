use super::repair::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::jobs::utility::movebehavior::mark_working;
use crate::repairqueue::RepairQueue;
use crate::room::data::*;
use crate::structureidentifier::*;
use log::*;
use screeps::*;

/// Get a repair target for a creep. Checks the repair queue first (for
/// mission-requested repairs), then falls back to the room-scan approach.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_repair_state<F, R>(
    creep: &Creep,
    build_room: &RoomData,
    repair_queue: &RepairQueue,
    minimum_priority: Option<RepairPriority>,
    state_map: F,
) -> Option<R>
where
    F: Fn(RemoteStructureIdentifier) -> R,
{
    if creep.store().get_used_capacity(Some(ResourceType::Energy)) == 0 {
        return None;
    }

    if let Some(structure_id) = select_repair_structure(build_room, repair_queue, minimum_priority, true) {
        return Some(state_map(structure_id));
    }

    None
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_repair<F, R>(tick_context: &mut JobTickContext, repair_structure_id: RemoteStructureIdentifier, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    let target_position = repair_structure_id.pos();

    let creep = tick_context.runtime_data.owner;
    let creep_pos = creep.pos();

    let expect_resolve = if creep_pos.room_name() == target_position.room_name() {
        true
    } else {
        let target_room_entity = tick_context.runtime_data.mapping.get_room(&target_position.room_name())?;
        let target_room_data = tick_context.system_data.room_data.get(target_room_entity)?;

        target_room_data.get_dynamic_visibility_data().map(|v| v.visible()).unwrap_or(false)
    };

    let repair_target = repair_structure_id.resolve();

    if let Some(repair_target) = repair_target.as_ref() {
        if let Some(attackable) = repair_target.as_attackable() {
            if attackable.hits() >= attackable.hits_max() {
                return Some(next_state());
            }
        }
    } else if expect_resolve {
        return Some(next_state());
    }

    if !creep_pos.in_range_to(target_position, 3) {
        if tick_context.action_flags.consume(SimultaneousActionFlags::MOVE) {
            tick_context
                .runtime_data
                .movement
                .move_to(tick_context.runtime_data.creep_entity, target_position)
                .range(3);
        }

        return None;
    }

    // In range — mark as working within range 3 of the repair target.
    mark_working(tick_context, target_position, 3);

    if let Some(structure) = repair_target.as_ref() {
        if let Some(attackable) = structure.as_attackable() {
            if attackable.hits() >= attackable.hits_max() {
                return Some(next_state());
            }
        }

        if tick_context.action_flags.consume(SimultaneousActionFlags::REPAIR) {
            if let Some(repairable) = structure.as_repairable() {
                match creep.repair(repairable) {
                    Ok(()) => None,
                    Err(_) => Some(next_state()),
                }
            } else {
                Some(next_state())
            }
        } else {
            None
        }
    } else {
        Some(next_state())
    }
}

/// Opportunistically repair a nearby structure while performing another task
/// (e.g. hauling, harvesting, moving). Checks the repair queue for in-range
/// mission-requested repairs first, then falls back to a room scan.
///
/// Returns the amount of energy consumed if a repair was performed.
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_opportunistic_repair(tick_context: &mut JobTickContext, minimum_priority: Option<RepairPriority>) -> Option<u32> {
    if !tick_context.action_flags.intersects(SimultaneousActionFlags::REPAIR) {
        let creep = tick_context.runtime_data.owner;

        let available_energy = creep.store().get(ResourceType::Energy).unwrap_or(0);

        if available_energy > 0 {
            let work_body_parts = creep.body().iter().filter(|p| p.part() == Part::Work).count() as u32;

            if work_body_parts > 0 {
                let creep_pos = creep.pos();

                let room_entity = tick_context.runtime_data.mapping.get_room(&creep_pos.room_name())?;
                let room_data = tick_context.system_data.room_data.get(room_entity)?;

                // Check repair queue for in-range targets first, then fall
                // back to room scan. Walls are excluded from opportunistic
                // repair (too expensive for a drive-by).
                let repair_target = select_repair_structure_in_range(
                    room_data,
                    tick_context.system_data.repair_queue,
                    creep_pos,
                    3,
                    minimum_priority,
                    false,
                );

                if let Some((_, target_id)) = repair_target {
                    if let Some(structure) = target_id.resolve() {
                        if tick_context.action_flags.consume(SimultaneousActionFlags::REPAIR) {
                            if let Some(repairable) = structure.as_repairable() {
                                match creep.repair(repairable) {
                                    Ok(()) => {
                                        let max_energy_consumed = work_body_parts.min(available_energy);
                                        let (hits, hits_max) = structure
                                            .as_attackable()
                                            .map(|a| (a.hits(), a.hits_max()))
                                            .unwrap_or((0, 0));
                                        let max_repair_energy =
                                            ((hits_max - hits) as f32 / REPAIR_POWER as f32).ceil() as u32;
                                        let energy_consumed = max_energy_consumed.min(max_repair_energy);

                                        return Some(energy_consumed);
                                    }
                                    Err(err) => {
                                        info!(
                                            "Failed to repair structure: {:?} - Position: {:?} - Error: {:?}",
                                            structure.structure_type(),
                                            structure.pos(),
                                            err
                                        );
                                        return None;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    None
}
