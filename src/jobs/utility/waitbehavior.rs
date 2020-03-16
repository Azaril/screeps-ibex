
#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn run_wait_state<F, R>(delay: &mut u32, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    if *delay == 0 {
        Some(next_state())
    } else {
       *delay -= 1;
       
       None
    }
}
