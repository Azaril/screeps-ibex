use screeps::*;

#[macro_use] 
extern crate log;

use std::borrow::Cow;

pub type StrCow = Cow<'static, str>;

#[must_use = "The guard is immediately dropped after instantiation. This is probably not
what you want! Consider using a `let` binding to increase its lifetime."]
pub struct SpanGuard {
    name: StrCow
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        end(self.name.clone());
    }
}

/*
pub fn start_guard(ident: String) -> SpanGuard {
    let start_time = ::screeps::game::cpu::get_used();

    info!("[Timing] Enter: {} - {}", ident, start_time);

    scopeguard::guard(start_time, |previous| {
        let exit_time = ::screeps::game::cpu::get_used();
        let delta = exit_time - previous;

        info!("[Timing] - Exit: {} - {}", ident, exit_time, delta);
    })
}
*/

pub fn start_guard<S: Into<StrCow>>(name: S) -> SpanGuard {
    let name = name.into();
    start(name.clone());
    SpanGuard { name }
}

fn start<S: Into<StrCow>>(name: S) {
    let name = name.into();

    info!("Enter: {:?} - CPU: {}", name, game::cpu::get_used());
}

fn end<S: Into<StrCow>>(name: S) -> u64 {
    let name = name.into();

    info!("Exit: {:?} - CPU: {}", name, game::cpu::get_used());

    0
}