use std::ops::*;

pub trait FastCacheExpiration<T> {
    fn expire(&mut self);

    fn has_expired<X>(&self, expiration: X) -> bool where X: FnOnce(&T) -> bool;
}

pub trait FastCacheGet<T> {
    fn get_or_insert_with<F: FnOnce() -> T>(&mut self, f: F) -> &T;
}


pub trait FastCacheMaybeGet<T> {
    fn maybe_get_or_insert_with<F: FnOnce() -> Option<T>>(&mut self, f: F) -> Option<&T>;
}

impl<T> FastCacheExpiration<T> for Option<T> {
    fn expire(&mut self) {
        self.take();
    }

    fn has_expired<X>(&self, expiration: X) -> bool
    where X: FnOnce(&T) -> bool {
        self.as_ref().map(expiration).unwrap_or(false)
    }
}

impl<T> FastCacheGet<T> for Option<T> {
    fn get_or_insert_with<F: FnOnce() -> T>(&mut self, f: F) -> &T {
        self.get_or_insert_with(f)
    }
}

impl<T> FastCacheMaybeGet<T> for Option<T> {
    fn maybe_get_or_insert_with<F: FnOnce() -> Option<T>>(&mut self, f: F) -> Option<&T> {
        *self = (f)();

        self.as_ref()
    }
}

pub trait FastCacheAccessor<T>: FastCacheExpiration<T> + FastCacheGet<T> where Self: Sized {
    fn access<X, F>(&mut self, expiration: X, filler: F) -> CacheAccesor<T, Self, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool;
}

pub trait FastCacheMaybeAccessor<T>: FastCacheExpiration<T> + FastCacheMaybeGet<T> where Self: Sized {
    fn maybe_access<X, F>(&mut self, expiration: X, filler: F) -> MaybeCacheAccesor<T, Self, X, F> where F: FnOnce() -> Option<T>, X: FnOnce(&T) -> bool;
}

impl<C, T> FastCacheAccessor<T> for C where C: FastCacheExpiration<T> + FastCacheGet<T> {
    fn access<X, F>(&mut self, expiration: X, filler: F) -> CacheAccesor<T, Self, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool {
        CacheAccesor {
            state: std::cell::RefCell::new(CacheState::Unknown(CacheStateUnknown {
                cache: self,
                expiration: expiration,
                fill: filler
            }))
        }
    }
}

impl<C, T> FastCacheMaybeAccessor<T> for C where C: FastCacheExpiration<T> + FastCacheMaybeGet<T> {
    fn maybe_access<X, F>(&mut self, expiration: X, filler: F) -> MaybeCacheAccesor<T, Self, X, F> where F: FnOnce() -> Option<T>, X: FnOnce(&T) -> bool {
        MaybeCacheAccesor {
            state: std::cell::RefCell::new(MaybeCacheState::Unknown(MaybeCacheStateUnknown {
                cache: self,
                expiration: expiration,
                fill: filler
            }))
        }
    }
}

pub struct CacheAccesor<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCacheGet<T> + FastCacheExpiration<T> {
    state: std::cell::RefCell<CacheState<'c, T, C, X, F>>
}

impl<'c, T, C, X, F> CacheAccesor<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCacheGet<T> + FastCacheExpiration<T> {
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

pub enum CacheState<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCacheGet<T> + FastCacheExpiration<T> {
    Unknown(CacheStateUnknown<'c, T, C, X, F>),
    Known(CacheStateKnown<'c, T>)
}

impl<'c, T, C, X, F> CacheState<'c, T, C, X, F> where F: FnOnce() -> T, X: FnOnce(&T) -> bool, C: FastCacheGet<T> + FastCacheExpiration<T> {
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

pub struct CacheStateUnknown<'c, T, C, X, F> where X: FnOnce(&T) -> bool, F: FnOnce() -> T, C: FastCacheGet<T> + FastCacheExpiration<T> {
    cache: &'c mut C,
    expiration: X,
    fill: F
}

pub struct CacheStateKnown<'c, T> {
    data: &'c T,
}

pub struct MaybeCacheAccesor<'c, T, C, X, F> where F: FnOnce() -> Option<T>, X: FnOnce(&T) -> bool, C: FastCacheMaybeGet<T> + FastCacheExpiration<T> {
    state: std::cell::RefCell<MaybeCacheState<'c, T, C, X, F>>
}

impl<'c, T, C, X, F> MaybeCacheAccesor<'c, T, C, X, F> where F: FnOnce() -> Option<T>, X: FnOnce(&T) -> bool, C: FastCacheMaybeGet<T> + FastCacheExpiration<T> {
    pub fn get(&self) -> std::cell::Ref<Option<&'c T>> {
        take_mut::take(&mut *self.state.borrow_mut(), |v| v.into_known());

        std::cell::Ref::map(self.state.borrow(), |s| {
            match s {
                MaybeCacheState::Unknown(_) => { unsafe { std::hint::unreachable_unchecked() } },
                MaybeCacheState::Known(s) => &s.data,
            }
        })
    }
}

pub enum MaybeCacheState<'c, T, C, X, F> where F: FnOnce() -> Option<T>, X: FnOnce(&T) -> bool, C: FastCacheMaybeGet<T> + FastCacheExpiration<T> {
    Unknown(MaybeCacheStateUnknown<'c, T, C, X, F>),
    Known(MaybeCacheStateKnown<'c, T>)
}

impl<'c, T, C, X, F> MaybeCacheState<'c, T, C, X, F> where F: FnOnce() -> Option<T>, X: FnOnce(&T) -> bool, C: FastCacheMaybeGet<T> + FastCacheExpiration<T> {
    pub fn into_known(self) -> Self {
        match self {
            MaybeCacheState::Unknown(state) => {
                let cache = &mut *state.cache;

                if cache.has_expired(state.expiration) {
                    cache.expire();
                }

                let val = cache.maybe_get_or_insert_with(state.fill);
        
                let new_state = MaybeCacheStateKnown {
                    data: val
                };

                MaybeCacheState::Known(new_state)            
            },
            v => v
        }
    }
}

pub struct MaybeCacheStateUnknown<'c, T, C, X, F> where X: FnOnce(&T) -> bool, F: FnOnce() -> Option<T>, C: FastCacheMaybeGet<T> + FastCacheExpiration<T> {
    cache: &'c mut C,
    expiration: X,
    fill: F
}

pub struct MaybeCacheStateKnown<'c, T> {
    data: Option<&'c T>,
}