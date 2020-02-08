#[cfg(timing)]
extern crate scopeguard;

#[allow(unused_macros)]
#[cfg(timing)]
macro_rules! scope_timing {
    ($($x:expr),*) => {
        let __data = format!($($x),+);
        let __guard = scopeguard::guard(::screeps::game::cpu::get_used(), |previous| {
            let delta = ::screeps::game::cpu::get_used() - previous;

            info!("[Timing] {} - {}", __data, delta);
        });
    };    
}

#[cfg(not(timing))]
macro_rules! scope_timing {
    ($($x:expr),*) => {
    }
}