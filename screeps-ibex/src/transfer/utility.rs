use super::transfersystem::*;
use itertools::*;
use screeps::*;

pub fn calc_transaction_cost_fractional(from: RoomName, to: RoomName) -> f64 {
    let distance = game::map::get_room_linear_distance(from, to, true) as f64;

    1.0 - (-distance / 30.0).exp()
}

pub fn generate_active_priorities(
    pickup_priorities: TransferPriorityFlags,
    delivery_priorities: TransferPriorityFlags,
) -> Vec<(TransferPriority, TransferPriority)> {
    let mut priorities = ALL_TRANSFER_PRIORITIES
        .iter()
        .cartesian_product(ALL_TRANSFER_PRIORITIES.iter())
        .filter(|(&p1, &p2)| pickup_priorities.contains(p1.into()) || delivery_priorities.contains(p2.into()))
        .filter(|(&p1, &p2)| p1 != TransferPriority::None || p2 != TransferPriority::None)
        .map(|(p1, p2)| (*p1, *p2))
        .collect_vec();

    priorities.sort_by(|(a_1, a_2), (b_1, b_2)| a_1.max(a_2).cmp(b_1.max(b_2)).then_with(|| a_1.cmp(b_1)).then_with(|| a_2.cmp(b_2)));

    priorities
}
