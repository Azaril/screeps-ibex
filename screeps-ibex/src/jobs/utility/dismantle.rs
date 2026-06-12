use crate::remoteobjectid::*;
use screeps::*;
use std::borrow::*;

pub fn ignore_for_dismantle<T>(structure: T, sources: &[RemoteObjectId<Source>]) -> bool
where
    T: Borrow<StructureObject>,
{
    match structure.borrow() {
        StructureObject::StructureContainer(c) => {
            let pos = c.pos();

            sources.iter().any(|s| s.pos().in_range_to(pos, 1))
        }
        _ => false,
    }
}

pub fn can_dismantle<T>(structure: T) -> bool
where
    T: Borrow<StructureObject>,
{
    structure
        .borrow()
        .as_attackable()
        .map(|a| a.hits() > 0 && a.hits_max() > 0)
        .unwrap_or(false)
}

pub fn has_empty_storage<T>(structure: T) -> bool
where
    T: Borrow<StructureObject>,
{
    if let Some(store) = structure.borrow().as_has_store() {
        let store_types = store.store().store_types();

        return !store_types.iter().any(|t| store.store().get_used_capacity(Some(*t)) > 0);
    }

    true
}

/// Hit-pool horizon for dismantle work: targets with more hits than
/// `max_hits` are skipped entirely (0 = no limit). Huge walls/ramparts would
/// otherwise pin a dismantle mission ~forever and block any downstream
/// handoff (e.g. salvage → mining outpost). Mission completion checks and
/// job target selection MUST share this filter or the mission never ends
/// (`features.derelict.max_structure_hits`).
pub fn within_dismantle_hits_horizon<T>(structure: T, max_hits: u32) -> bool
where
    T: Borrow<StructureObject>,
{
    if max_hits == 0 {
        return true;
    }

    structure
        .borrow()
        .as_attackable()
        .map(|a| a.hits() <= max_hits)
        .unwrap_or(false)
}

/// Structures whose stores may be looted by salvage/raid work: structures
/// owned by another player, or unowned store structures (containers) that are
/// not our mining infrastructure (source-adjacent — same exclusion as
/// [`ignore_for_dismantle`]). Own/ownerless-controller structures are never
/// loot targets.
pub fn is_salvage_loot_target<T>(structure: T, sources: &[RemoteObjectId<Source>]) -> bool
where
    T: Borrow<StructureObject>,
{
    let structure = structure.borrow();

    if has_empty_storage(structure) {
        return false;
    }

    match structure.as_owned() {
        Some(owned) => owned.owner().is_some() && !owned.my(),
        None => !ignore_for_dismantle(structure, sources),
    }
}
