use screeps::*;
use std::borrow::*;

pub fn can_dismantle<T>(structure: T) -> bool
where
    T: Borrow<Structure>,
{
    structure.borrow().as_attackable().map(|a| a.hits() > 0).unwrap_or(false)
}

pub fn has_empty_storage<T>(structure: T) -> bool
where
    T: Borrow<Structure>,
{
    if let Some(store) = structure.borrow().as_has_store() {
        let store_types = store.store_types();

        return !store_types.iter().any(|t| store.store_used_capacity(Some(*t)) > 0);
    }

    true
}
