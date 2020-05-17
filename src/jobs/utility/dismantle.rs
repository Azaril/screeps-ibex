use screeps::*;

pub fn get_dismantle_structures(room: Room, ignore_storage: bool) -> impl Iterator<Item = Structure> {
    let structures = room.find(find::STRUCTURES);

    structures
        .into_iter()
        .filter(|structure| structure.as_attackable().map(|a| a.hits() > 0).unwrap_or(false))
        .filter(move |structure| {
            if ignore_storage {
                return true;
            }

            if let Some(store) = structure.as_has_store() {
                let store_types = store.store_types();

                return !store_types.iter().any(|t| store.store_used_capacity(Some(*t)) > 0);
            }

            true
        })
}
