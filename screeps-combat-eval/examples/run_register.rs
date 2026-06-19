//! Run the EXP-* register and print the report (the tactics-tuning dashboard).
//! `cargo run --example run_register -p screeps-combat-eval`

fn main() {
    print!("{}", screeps_combat_eval::report(&screeps_combat_eval::register()));
}
