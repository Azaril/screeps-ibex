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
