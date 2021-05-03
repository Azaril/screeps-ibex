use screeps::*;

pub trait HasExpensiveStore {
    fn expensive_free_capacity(&self) -> u32;
}

impl HasExpensiveStore for Store {
    fn expensive_free_capacity(&self) -> u32 {
        let capacity = self.get_capacity(None);
        let store_types = self.store_types();
        let used_capacity = store_types.iter().map(|r| self.get_used_capacity(Some(*r))).sum::<u32>();
        capacity - used_capacity
    }
}
