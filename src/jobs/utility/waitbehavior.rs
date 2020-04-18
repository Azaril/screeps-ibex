#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
pub fn tick_wait<F, R>(ticks: &mut u32, next_state: F) -> Option<R>
where
    F: Fn() -> R,
{
    if *ticks == 0 {
        Some(next_state())
    } else {
        *ticks -= 1;

        None
    }
}
