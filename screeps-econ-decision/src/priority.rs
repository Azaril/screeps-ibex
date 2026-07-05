//! The transfer priority/type vocabulary + the tier-interleave generator — MOVED verbatim from
//! `screeps-ibex/src/transfer/transfersystem.rs` (`TransferPriority`/`TransferType` + flags) and
//! `screeps-ibex/src/transfer/utility.rs` (`generate_active_priorities`) at ADR 0040 M3. Lives
//! here now, consumed by the bot (re-exported from `transfer::transfersystem`, so the serialized
//! ticket shapes are UNCHANGED) and by the sim (`screeps-econ-eval::baseline`, whose `Tier`/
//! `TierMask`/`interleave_combos` mirrors are deleted).
//!
//! `TransferPriority` and `TransferType` ride inside serialized tickets
//! (`TransferWithdrawTicket`/`TransferDepositTicket`/`HaulState`); their serde shapes are
//! byte-identical to the pre-move definitions (zero WFV).

use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Clone, Copy, Serialize, Deserialize)]
#[repr(u8)]
pub enum TransferPriority {
    High = 0,
    Medium = 1,
    Low = 2,
    None = 3,
}

pub const ACTIVE_TRANSFER_PRIORITIES: &[TransferPriority] = &[TransferPriority::High, TransferPriority::Medium, TransferPriority::Low];
pub const ALL_TRANSFER_PRIORITIES: &[TransferPriority] = &[
    TransferPriority::High,
    TransferPriority::Medium,
    TransferPriority::Low,
    TransferPriority::None,
];

bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct TransferPriorityFlags: u8 {
        const UNSET = 0;

        const HIGH = 1u8 << (TransferPriority::High as u8);
        const MEDIUM = 1u8 << (TransferPriority::Medium as u8);
        const LOW = 1u8 << (TransferPriority::Low as u8);
        const NONE = 1u8 << (TransferPriority::None as u8);

        const ALL = Self::HIGH.bits() | Self::MEDIUM.bits() | Self::LOW.bits() | Self::NONE.bits();
        const ACTIVE = Self::HIGH.bits() | Self::MEDIUM.bits() | Self::LOW.bits();
    }
}

impl<T> From<T> for TransferPriorityFlags
where
    T: Borrow<TransferPriority>,
{
    fn from(priority: T) -> TransferPriorityFlags {
        match priority.borrow() {
            TransferPriority::High => TransferPriorityFlags::HIGH,
            TransferPriority::Medium => TransferPriorityFlags::MEDIUM,
            TransferPriority::Low => TransferPriorityFlags::LOW,
            TransferPriority::None => TransferPriorityFlags::NONE,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
pub enum TransferType {
    Haul = 0,
    Link = 1,
    Terminal = 2,
    Use = 3,
}

bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct TransferTypeFlags: u8 {
        const UNSET = 0;

        const HAUL = 1u8 << (TransferType::Haul as u8);
        const LINK = 1u8 << (TransferType::Link as u8);
        const TERMINAL = 1u8 << (TransferType::Terminal as u8);
        const USE = 1u8 << (TransferType::Use as u8);
    }
}

impl<T> From<T> for TransferTypeFlags
where
    T: Borrow<TransferType>,
{
    fn from(transfer_type: T) -> TransferTypeFlags {
        match transfer_type.borrow() {
            TransferType::Haul => TransferTypeFlags::HAUL,
            TransferType::Link => TransferTypeFlags::LINK,
            TransferType::Terminal => TransferTypeFlags::TERMINAL,
            TransferType::Use => TransferTypeFlags::USE,
        }
    }
}

enum ActivePriorityGeneratorState {
    Pickup,
    Delivery,
}

struct ActivePriorityGenerator {
    pickup_priorities: TransferPriorityFlags,
    delivery_priorities: TransferPriorityFlags,

    next_pickup_priority: Option<TransferPriority>,
    next_delivery_priority: Option<TransferPriority>,

    state: ActivePriorityGeneratorState,
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
                        self.next_pickup_priority = next_priority(pickup_priority);

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
                        self.next_delivery_priority = next_priority(delivery_priority);

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

/// The pickup+delivery TIER-INTERLEAVE combinations for the allowed-priority masks — seeded
/// High/High starting in the Delivery arm: per tier in High→Medium→Low→None order, the delivery
/// arm `(allowed, {tier})` then the same tier's pickup arm `({tier}, allowed)`; a named NONE tier
/// masks the opposite side to ACTIVE (the null-loop guard). Tiers absent from the allowed masks
/// emit nothing.
pub fn generate_active_priorities(
    pickup_priorities: TransferPriorityFlags,
    delivery_priorities: TransferPriorityFlags,
) -> impl Iterator<Item = (TransferPriorityFlags, TransferPriorityFlags)> {
    ActivePriorityGenerator {
        pickup_priorities,
        delivery_priorities,

        next_pickup_priority: Some(TransferPriority::High),
        next_delivery_priority: Some(TransferPriority::High),

        state: ActivePriorityGeneratorState::Delivery,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mask parameterization pin (moved from the sim baseline's
    /// `interleave_mask_restricts_the_generator`, itself pinned against the live generator):
    /// for allowed = HIGH|NONE on both sides the sequence is
    /// (H|N→H), (H→H|N), (H→N), (N→H) — no M/L anywhere, and the named-NONE arm masks the
    /// opposite side to allowed ∩ ACTIVE.
    #[test]
    fn interleave_mask_restricts_the_generator() {
        let mask = TransferPriorityFlags::HIGH | TransferPriorityFlags::NONE;
        let combos: Vec<_> = generate_active_priorities(mask, mask).collect();
        assert_eq!(
            combos,
            vec![
                (mask, TransferPriorityFlags::HIGH),
                (TransferPriorityFlags::HIGH, mask),
                (TransferPriorityFlags::HIGH, TransferPriorityFlags::NONE),
                (TransferPriorityFlags::NONE, TransferPriorityFlags::HIGH),
            ]
        );
    }

    /// The full-mask sequence: delivery-arm-first per tier, High→Medium→Low→None.
    #[test]
    fn interleave_full_mask_order() {
        let all = TransferPriorityFlags::ALL;
        let active = TransferPriorityFlags::ACTIVE;
        let combos: Vec<_> = generate_active_priorities(all, all).collect();
        assert_eq!(
            combos,
            vec![
                (all, TransferPriorityFlags::HIGH),
                (TransferPriorityFlags::HIGH, all),
                (all, TransferPriorityFlags::MEDIUM),
                (TransferPriorityFlags::MEDIUM, all),
                (all, TransferPriorityFlags::LOW),
                (TransferPriorityFlags::LOW, all),
                (active, TransferPriorityFlags::NONE),
                (TransferPriorityFlags::NONE, active),
            ]
        );
    }

    /// Serde-shape pin: the enum variants serialize by name exactly as the pre-move
    /// `transfersystem.rs` definitions did (tickets/HaulState are serialized — zero WFV).
    #[test]
    fn serde_shapes_unchanged() {
        assert_eq!(serde_json::to_string(&TransferPriority::High).unwrap(), "\"High\"");
        assert_eq!(serde_json::to_string(&TransferPriority::None).unwrap(), "\"None\"");
        assert_eq!(serde_json::to_string(&TransferType::Haul).unwrap(), "\"Haul\"");
        assert_eq!(serde_json::to_string(&TransferType::Use).unwrap(), "\"Use\"");
        let p: TransferPriority = serde_json::from_str("\"Medium\"").unwrap();
        assert_eq!(p, TransferPriority::Medium);
        let t: TransferType = serde_json::from_str("\"Terminal\"").unwrap();
        assert_eq!(t, TransferType::Terminal);
    }
}
