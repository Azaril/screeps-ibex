use screeps::*;

pub trait HasExpensiveStore: HasStore {
    fn expensive_store_free_capacity(&self) -> u32;
}

impl<T> HasExpensiveStore for T
where
    T: HasStore,
{
    fn expensive_store_free_capacity(&self) -> u32 {
        let capacity = self.store().get_capacity(None);
        let store_types = self.store().store_types();
        let used_capacity = store_types.iter().map(|r| self.store().get_used_capacity(Some(*r))).sum::<u32>();
        capacity - used_capacity
    }
}
