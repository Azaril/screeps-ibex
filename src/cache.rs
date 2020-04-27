pub trait FastCache<T> {
    fn expire(&mut self);

    fn has_expired<X>(&self, expiration: X) -> bool where X: FnOnce(&T) -> bool;

    fn get_or_insert_with<F: FnOnce() -> T>(&mut self, f: F) -> &mut T;
}

impl<T> FastCache<T> for Option<T> {
    fn expire(&mut self) {
        self.take();
    }

    fn has_expired<X>(&self, expiration: X) -> bool
    where X: FnOnce(&T) -> bool {
        self.as_ref().map(expiration).unwrap_or(true)
    }

    fn get_or_insert_with<F: FnOnce() -> T>(&mut self, f: F) -> &mut T {
        self.get_or_insert_with(f)
    }
}

pub trait FastCacheAccessor<T>: FastCache<T> where Self: Sized {
    fn with_access<X, F>(&mut self, expiration: X, filler: F) -> CacheAccesor<T, Self, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool;
}

impl<C, T> FastCacheAccessor<T> for C where C: FastCache<T> {
    fn with_access<X, F>(&mut self, expiration: X, filler: F) -> CacheAccesor<T, Self, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool {
        CacheAccesor {
            state: std::cell::RefCell::new(CacheState::Unknown(CacheStateUnknown {
                cache: self,
                expiration: expiration,
                fill: filler
            }))
        }
    }
}

pub struct CacheAccesor<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCache<T> {
    state: std::cell::RefCell<CacheState<'c, T, C, X, F>>
}

use std::ops::*;

impl<'c, T, C, X, F> CacheAccesor<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCache<T> {
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

pub enum CacheState<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCache<T> {
    Unknown(CacheStateUnknown<'c, T, C, X, F>),
    Known(CacheStateKnown<'c, T>)
}

impl<'c, T, C, X, F> CacheState<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCache<T> {
    pub fn into_known(self) -> Self {
        match self {
            CacheState::Unknown(state) => {
                let cache = &mut *state.cache;

                if cache.has_expired(state.expiration) {
                    cache.expire();
                }
        
                let ref_val = cache.get_or_insert_with(state.fill);
        
                let new_state = CacheStateKnown {
                    data: ref_val
                };

                CacheState::Known(new_state)            
            },
            v => v
        }
    }
}

pub struct CacheStateUnknown<'c, T, C, X, F> where X: FnOnce(&T) -> bool, F: FnOnce() -> T, C: FastCache<T> {
    cache: &'c mut C,
    expiration: X,
    fill: F
}

pub struct CacheStateKnown<'c, T> {
    data: &'c mut T,
}