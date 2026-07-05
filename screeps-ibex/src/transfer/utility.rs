use screeps::*;

// The tier-interleave generator (`generate_active_priorities`) lives in
// `screeps_econ_decision::priority` since ADR 0040 M3 — one implementation, consumed by the
// K2 selection kernel AND any direct bot caller; its mask/order pins moved with it.
pub use screeps_econ_decision::priority::generate_active_priorities;

pub fn calc_transaction_cost_fractional(from: RoomName, to: RoomName) -> f64 {
    let distance = game::map::get_room_linear_distance(from, to, true) as f64;

    1.0 - (-distance / 30.0).exp()
}
