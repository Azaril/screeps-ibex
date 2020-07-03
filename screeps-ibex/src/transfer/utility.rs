use super::transfersystem::*;
use screeps::*;

pub fn calc_transaction_cost_fractional(from: RoomName, to: RoomName) -> f64 {
    let distance = game::map::get_room_linear_distance(from, to, true) as f64;

    1.0 - (-distance / 30.0).exp()
}

enum ActivePriorityGeneratorState {
    Pickup,
    Delivery
}

struct ActivePriorityGenerator {
    pickup_priorities: TransferPriorityFlags,
    delivery_priorities: TransferPriorityFlags,

    next_pickup_priority: Option<TransferPriority>,
    next_delivery_priority: Option<TransferPriority>,

    state: ActivePriorityGeneratorState
}

fn next_priority(priority: TransferPriority) -> Option<TransferPriority> {
    match priority {
        TransferPriority::High => Some(TransferPriority::Medium),
        TransferPriority::Medium => Some(TransferPriority::Low),
        TransferPriority::Low => Some(TransferPriority::None),
        TransferPriority::None => None,
    }
}

impl Iterator for ActivePriorityGenerator {
    type Item = (TransferPriorityFlags, TransferPriorityFlags);

    fn next(&mut self) -> Option<Self::Item> { 
        while self.next_pickup_priority.is_some() || self.next_delivery_priority.is_some() {
            match self.state {
                ActivePriorityGeneratorState::Pickup => {
                    self.state = ActivePriorityGeneratorState::Delivery;

                    if let Some(pickup_priority) = self.next_pickup_priority {
                        self.next_pickup_priority  = next_priority(pickup_priority);

                        let priority_mask = pickup_priority.into();

                        if self.pickup_priorities.contains(priority_mask) {
                            let delivery_priorities = if priority_mask.contains(TransferPriorityFlags::NONE) {
                                self.delivery_priorities & TransferPriorityFlags::ACTIVE
                            } else {
                                self.delivery_priorities
                            };

                            return Some((priority_mask, delivery_priorities));
                        }
                    }
                }
                ActivePriorityGeneratorState::Delivery => {
                    self.state = ActivePriorityGeneratorState::Pickup;

                    if let Some(delivery_priority) = self.next_delivery_priority {
                        self.next_delivery_priority  = next_priority(delivery_priority);

                        let priority_mask = delivery_priority.into();

                        if self.delivery_priorities.contains(priority_mask) {
                            let pickup_priorities = if priority_mask.contains(TransferPriorityFlags::NONE) {
                                self.pickup_priorities & TransferPriorityFlags::ACTIVE
                            } else {
                                self.pickup_priorities
                            };

                            return Some((pickup_priorities, priority_mask));
                        }
                    }
                }
            }
        }

        None
    }
    
}

pub fn generate_active_priorities(
    pickup_priorities: TransferPriorityFlags,
    delivery_priorities: TransferPriorityFlags,
) -> impl Iterator<Item = (TransferPriorityFlags, TransferPriorityFlags)> {
    ActivePriorityGenerator {
        pickup_priorities,
        delivery_priorities,

        next_pickup_priority: Some(TransferPriority::High),
        next_delivery_priority: Some(TransferPriority::High),

        state: ActivePriorityGeneratorState::Pickup
    }
}