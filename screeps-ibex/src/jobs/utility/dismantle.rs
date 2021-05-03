use crate::remoteobjectid::*;
use screeps::*;
use std::borrow::*;

pub fn ignore_for_dismantle(structure: &StructureObject, sources: &[RemoteObjectId<Source>]) -> bool
{
    match structure {
        StructureObject::StructureContainer(c) => {
            let pos = c.pos();

            sources.iter().any(|s| s.pos().in_range_to(pos, 1))
        }
        _ => false,
    }
}

pub fn can_dismantle(structure: &StructureObject) -> bool
{
    structure
        .borrow()
        .as_attackable()
        .map(|a| a.hits() > 0 && a.hits_max() > 0)
        .unwrap_or(false)
}

pub fn has_empty_storage(structure: &StructureObject) -> bool
{
    if let Some(store) = structure.borrow().as_has_store() {
        let store = store.store();
        let store_types = store.store_types();

        return !store_types.iter().any(|t| store.get_used_capacity(Some(*t)) > 0);
    }

    true
}
