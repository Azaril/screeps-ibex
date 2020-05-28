use specs::prelude::*;
use specs::storage::*;

pub trait ComponentAccess<C>
where
    C: Component,
{
    fn get(&self, entity: Entity) -> Option<&C>;

    fn get_mut(&mut self, entity: Entity) -> Option<&mut C>;
}

impl<'rf, 'st: 'rf, C, S, B> ComponentAccess<C> for PairedStorage<'rf, 'st, C, S, B, SequentialRestriction>
where
    C: Component,
    S: std::borrow::BorrowMut<C::Storage>,
    B: std::borrow::Borrow<BitSet>,
{
    fn get(&self, entity: Entity) -> Option<&C> {
        self.get(entity)
    }

    fn get_mut(&mut self, entity: Entity) -> Option<&mut C> {
        self.get_mut(entity)
    }
}

pub trait ComponentAccessOption<C>
where
    C: Component,
{
    fn try_get<E>(&self, entity: E) -> Option<&C>
    where
        E: Into<Option<Entity>>;
}

pub trait ComponentAccessOptionMut<C>
where
    C: Component,
{
    fn try_get_mut<E>(&mut self, entity: E) -> Option<&mut C>
    where
        E: Into<Option<Entity>>;
}

impl<'a, 'b, C> ComponentAccessOption<C> for &'a (dyn ComponentAccess<C> + 'b)
where
    C: Component,
{
    fn try_get<E>(&self, entity: E) -> Option<&C>
    where
        E: Into<Option<Entity>>,
    {
        entity.into().and_then(move |e| self.get(e))
    }
}

impl<'a, 'b, C> ComponentAccessOption<C> for &'a mut (dyn ComponentAccess<C> + 'b)
where
    C: Component,
{
    fn try_get<E>(&self, entity: E) -> Option<&C>
    where
        E: Into<Option<Entity>>,
    {
        entity.into().and_then(move |e| self.get(e))
    }
}

impl<'a, 'b, C> ComponentAccessOptionMut<C> for &'a mut (dyn ComponentAccess<C> + 'b)
where
    C: Component,
{
    fn try_get_mut<E>(&mut self, entity: E) -> Option<&mut C>
    where
        E: Into<Option<Entity>>,
    {
        entity.into().and_then(move |e| self.get_mut(e))
    }
}
