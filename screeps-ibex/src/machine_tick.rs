use log::*;

const MAX_STATE_TRANSITIONS: u32 = 20;

pub fn run_state_machine<S, F>(state: &mut S, label: &str, mut tick_fn: F)
where
    F: FnMut(&mut S) -> Option<S>,
{
    let mut transitions = 0u32;
    while let Some(new_state) = tick_fn(state) {
        *state = new_state;
        transitions += 1;
        if transitions >= MAX_STATE_TRANSITIONS {
            error!(
                "State machine '{}' exceeded {} transitions in a single tick, breaking to prevent infinite loop",
                label, MAX_STATE_TRANSITIONS
            );
            break;
        }
    }
}

pub fn run_state_machine_result<S, F>(state: &mut S, label: &str, mut tick_fn: F) -> Result<(), String>
where
    F: FnMut(&mut S) -> Result<Option<S>, String>,
{
    let mut transitions = 0u32;
    while let Some(new_state) = tick_fn(state)? {
        *state = new_state;
        transitions += 1;
        if transitions >= MAX_STATE_TRANSITIONS {
            error!(
                "State machine '{}' exceeded {} transitions in a single tick, breaking to prevent infinite loop",
                label, MAX_STATE_TRANSITIONS
            );
            break;
        }
    }
    Ok(())
}
