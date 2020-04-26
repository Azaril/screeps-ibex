use serde::{Deserialize, Serialize};
use specs::saveload::*;
use specs::*;
pub struct FastCache<T> {
    data: Option<T>
}

impl<T> FastCache<T> {
    pub fn new() -> FastCache<T> {
        FastCache {
            data: None
        }
    }

    pub fn expire(&mut self) {
        self.data.take();
    }

    pub fn with_access<X, F>(&mut self, expiration: X, filler: F) -> CacheAccesor<T, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool {
        CacheAccesor {
            state: std::cell::RefCell::new(CacheState::Unknown(CacheStateUnknown {
                cache: self,
                expiration: expiration,
                fill: filler
            }))
        }
    }
}
pub struct CacheAccesor<'c, T, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool {
    state: std::cell::RefCell<CacheState<'c, T, X, F>>
}

use std::ops::*;

impl<'c, T, X, F> CacheAccesor<'c, T, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool {
    pub fn get(&self) -> std::cell::Ref<T> {
        take_mut::take(&mut *self.state.borrow_mut(), |v| v.into_known());

        std::cell::Ref::map(self.state.borrow(), |s| {
            match s {
                CacheState::Unknown(_) => { unsafe { std::hint::unreachable_unchecked() } },
                CacheState::Known(s) => s.data,
            }
        })
    }
}

pub enum CacheState<'c, T, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool {
    Unknown(CacheStateUnknown<'c, T, X, F>),
    Known(CacheStateKnown<'c, T>)
}

impl<'c, T, X, F> CacheState<'c, T, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool {
    pub fn into_known(self) -> Self {
        match self {
            CacheState::Unknown(state) => {
                let cache = &mut *state.cache;

                if cache.data.as_ref().map(state.expiration).unwrap_or(true) {
                    cache.data = None;
                }
        
                let ref_val = cache.data.get_or_insert_with(state.fill);
        
                let new_state = CacheStateKnown {
                    data: ref_val
                };

                CacheState::Known(new_state)            
            },
            v => v
        }
    }
}

pub struct CacheStateUnknown<'c, T, X, F> where X: FnOnce(&T) -> bool, F: FnOnce() -> T {
    cache: &'c mut FastCache<T>,
    expiration: X,
    fill: F
}

pub struct CacheStateKnown<'c, T> {
    data: &'c mut T,
}

impl<C, M: Serialize + Marker> ConvertSaveload<M> for FastCache<C>
where
    for<'de> M: Deserialize<'de>,
    C: ConvertSaveload<M>,
{
    type Data = Option<<C as ConvertSaveload<M>>::Data>;
    type Error = <C as ConvertSaveload<M>>::Error;

    fn convert_into<F>(&self, ids: F) -> Result<Self::Data, Self::Error>
    where
        F: FnMut(Entity) -> Option<M>,
    {
        let val = match &self.data {
            Some(v) => Some(v.convert_into(ids)?),
            None => None
        };

        Ok(val)
    }

    fn convert_from<F>(data: Self::Data, ids: F) -> Result<Self, Self::Error>
    where
        F: FnMut(M) -> Option<Entity>,
    {
        let val = match data {
            Some(v) => Some(<C as ConvertSaveload<M>>::convert_from(v, ids)?),
            None => None
        };

        Ok(FastCache { data: val })
    }
}