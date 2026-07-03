use super::repair::*;
use crate::energy_stress::*;
use crate::jobs::actions::*;
use crate::jobs::context::*;
use crate::jobs::utility::movebehavior::mark_working;
use crate::repairqueue::RepairQueue;
use crate::room::data::*;
use crate::structureidentifier::*;
use log::*;
use screeps::*;
use specs::prelude::*;

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

/// Energy a successful repair intent consumes this tick: one energy per WORK
/// part, clamped by carried energy and by the energy needed to finish the
/// target (`ceil(missing_hits / REPAIR_POWER)`).
fn repair_energy_consumed(work_body_parts: u32, available_energy: u32, hits: u32, hits_max: u32) -> u32 {
    let max_energy_consumed = work_body_parts.min(available_energy);
    let max_repair_energy = ((hits_max - hits) as f32 / REPAIR_POWER as f32).ceil() as u32;

    max_energy_consumed.min(max_repair_energy)
}

/// WORK parts still alive this tick — the engine spends repair energy only on
/// parts with `hits > 0` (processor repair.js repairPower filter), so the
/// `repair_leak_e` telemetry uses this count. Deposit accounting keeps the
/// historical total-parts count (S1 flag-off parity; see the call sites).
fn alive_work_parts(creep: &Creep) -> u32 {
    creep.body().iter().filter(|p| p.part() == Part::Work && p.hits() > 0).count() as u32
}

/// Record repair energy into the `repair_leak_e` telemetry (ADR 0040 §D6),
/// attributed to the creep's posture room (falling back to its current room
/// when the job has no home/delivery concept). Always-on — not gated by
/// `features.energy.repair_stress_gate` (telemetry never sheds).
fn record_creep_repair_leak(tick_context: &mut JobTickContext, posture_room: Option<Entity>, structure_type: StructureType, energy: u32) {
    let creep_room_name = tick_context.runtime_data.owner.pos().room_name();

    let Some(posture_room) = posture_room.or_else(|| tick_context.runtime_data.mapping.get_room(&creep_room_name)) else {
        return;
    };
    let Some(posture_room_name) = tick_context.system_data.room_data.get(posture_room).map(|r| r.name) else {
        return;
    };

    record_repair_leak(
        tick_context.runtime_data.energy_leak,
        tick_context.system_data.economy,
        posture_room,
        posture_room_name,
        structure_type,
        energy,
    );
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_repair<F, R>(
    tick_context: &mut JobTickContext,
    repair_structure_id: RemoteStructureIdentifier,
    posture_room: Option<Entity>,
    next_state: F,
) -> Option<R>
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
                    Ok(()) => {
                        // repair_leak_e telemetry (ADR 0040 §D6) — same
                        // arithmetic as the opportunistic path, over ALIVE
                        // WORK parts (destroyed parts spend nothing).
                        let work_body_parts = alive_work_parts(creep);
                        let available_energy = creep.store().get(ResourceType::Energy).unwrap_or(0);
                        let (hits, hits_max) = structure.as_attackable().map(|a| (a.hits(), a.hits_max())).unwrap_or((0, 0));
                        let energy_consumed = repair_energy_consumed(work_body_parts, available_energy, hits, hits_max);

                        record_creep_repair_leak(tick_context, posture_room, structure.structure_type(), energy_consumed);

                        None
                    }
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
pub fn tick_opportunistic_repair(
    tick_context: &mut JobTickContext,
    minimum_priority: Option<RepairPriority>,
    posture_room: Option<Entity>,
) -> Option<u32> {
    if !tick_context.action_flags.intersects(SimultaneousActionFlags::REPAIR) {
        let creep = tick_context.runtime_data.owner;

        let available_energy = creep.store().get(ResourceType::Energy).unwrap_or(0);

        if available_energy > 0 {
            let work_body_parts = creep.body().iter().filter(|p| p.part() == Part::Work).count() as u32;

            if work_body_parts > 0 {
                let creep_pos = creep.pos();

                let room_entity = tick_context.runtime_data.mapping.get_room(&creep_pos.room_name())?;
                let room_data = tick_context.system_data.room_data.get(room_entity)?;

                // S1 repair stress gate (ADR 0040 §D6): under refill deficit
                // with no stored buffer in the posture room (the creep's
                // home/delivery room, falling back to the current room), only
                // Critical repair is admitted.
                let posture_room = posture_room.unwrap_or(room_entity);
                let allowance = repair_allowance_for(
                    tick_context.system_data.economy,
                    tick_context.system_data.features,
                    Some(posture_room),
                );
                let minimum_priority = effective_min_repair_priority(minimum_priority, allowance);

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
                                        let (hits, hits_max) =
                                            structure.as_attackable().map(|a| (a.hits(), a.hits_max())).unwrap_or((0, 0));
                                        let energy_consumed =
                                            repair_energy_consumed(work_body_parts, available_energy, hits, hits_max);
                                        // Telemetry counts ALIVE WORK parts (the engine's
                                        // actual spend); the RETURNED value keeps the
                                        // historical total-parts arithmetic that deposit
                                        // accounting has always used (S1 flag-off parity).
                                        let telemetry_energy =
                                            repair_energy_consumed(alive_work_parts(creep), available_energy, hits, hits_max);

                                        // repair_leak_e telemetry (ADR 0040 §D6).
                                        record_creep_repair_leak(
                                            tick_context,
                                            Some(posture_room),
                                            structure.structure_type(),
                                            telemetry_energy,
                                        );

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

#[cfg(test)]
mod tests {
    use super::*;

    // Pin repair_energy_consumed = min(work_parts, carried, ceil(missing / REPAIR_POWER)) —
    // the exact-arithmetic contract the repair_leak_e telemetry rides on (ADR 0040 §D6).

    #[test]
    fn repair_energy_is_work_limited() {
        assert_eq!(repair_energy_consumed(3, 10, 0, 1000), 3);
    }

    #[test]
    fn repair_energy_is_carry_limited() {
        assert_eq!(repair_energy_consumed(10, 2, 0, 1000), 2);
    }

    #[test]
    fn repair_energy_is_missing_hits_limited_with_ceil() {
        // REPAIR_POWER = 100: 101 missing hits cost 2 energy, 100 cost 1.
        assert_eq!(repair_energy_consumed(10, 10, 899, 1000), 2);
        assert_eq!(repair_energy_consumed(10, 10, 900, 1000), 1);
        assert_eq!(repair_energy_consumed(10, 10, 999, 1000), 1);
    }

    #[test]
    fn full_health_target_consumes_nothing() {
        assert_eq!(repair_energy_consumed(10, 10, 1000, 1000), 0);
        assert_eq!(repair_energy_consumed(10, 10, 0, 0), 0);
    }
}
