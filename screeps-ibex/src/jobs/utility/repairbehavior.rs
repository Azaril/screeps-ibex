use super::repair::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::room::data::*;
use crate::structureidentifier::*;
use screeps::*;
use log::*;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn get_new_repair_state<F, R>(creep: &Creep, build_room: &RoomData, minimum_priority: Option<RepairPriority>, state_map: F) -> Option<R>
where
    F: Fn(RemoteStructureIdentifier) -> R,
{
    if creep.store_used_capacity(Some(ResourceType::Energy)) > 0 {
        if let Some(structure) = select_repair_structure(&build_room, minimum_priority, true) {
            return Some(state_map(RemoteStructureIdentifier::new(&structure)));
        }
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

    if let Some(structure) = repair_target.as_ref() {
        if let Some(attackable) = structure.as_attackable() {
            if attackable.hits() >= attackable.hits_max() {
                return Some(next_state());
            }
        }

        if tick_context.action_flags.consume(SimultaneousActionFlags::REPAIR) {
            match creep.repair(structure) {
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


#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_opportunistic_repair(tick_context: &mut JobTickContext, minimum_priority: Option<RepairPriority>) -> Option<u32>
{
    if !tick_context.action_flags.intersects(SimultaneousActionFlags::REPAIR) {
        let creep = tick_context.runtime_data.owner;

        let available_energy = creep.store_of(ResourceType::Energy);

        if available_energy > 0 {
            let work_body_parts = creep.body().iter().filter(|p| p.part == Part::Work).count() as u32;

            if work_body_parts > 0 {
                let creep_pos = creep.pos();

                let room_entity = tick_context.runtime_data.mapping.get_room(&creep_pos.room_name())?;
                let room_data = tick_context.system_data.room_data.get(room_entity)?;

                let structures = room_data.get_structures()?;

                let repair_structure = get_prioritized_repair_targets(structures.all(), None, false, false)
                    .filter(|(priority, _)| minimum_priority.map(|p| *priority >= p).unwrap_or(true))
                    .filter(|(_, structure)| structure.pos().in_range_to(&creep_pos, 3))
                    .max_by_key(|(priority, _)| *priority);

                if let Some((_, repair_structure)) = repair_structure {
                    if tick_context.action_flags.consume(SimultaneousActionFlags::REPAIR) {
                        match creep.repair(repair_structure) {
                            ReturnCode::Ok => {
                                let max_energy_consumed = work_body_parts.min(available_energy);
                                let (hits, hits_max) = repair_structure.as_attackable().map(|a| (a.hits(), a.hits_max())).unwrap_or((0, 0));
                                let max_repair_energy = ((hits_max - hits) as f32 / REPAIR_POWER as f32).ceil() as u32;
                                let energy_consumed = max_energy_consumed.min(max_repair_energy);

                                return Some(energy_consumed);
                            },
                            err => {
                                info!("Failed to repair structure: {:?} - Position: {:?} - Error: {:?}", repair_structure.structure_type(), repair_structure.pos(), err);
                                return None;
                            }
                        }
                    }
                }
            }
        }
    }

    None
}
