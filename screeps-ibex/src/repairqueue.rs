use crate::jobs::utility::repair::RepairPriority;
use crate::structureidentifier::RemoteStructureIdentifier;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

/// A single repair request submitted by a mission.
#[derive(Clone, Debug)]
pub struct RepairRequest {
    /// The structure to repair.
    pub structure_id: RemoteStructureIdentifier,
    /// Priority of the repair.
    pub priority: RepairPriority,
    /// Current hits of the structure (at time of request).
    pub current_hits: u32,
    /// Max hits of the structure.
    pub max_hits: u32,
    /// The room the structure is in.
    pub room: RoomName,
}

/// Per-room repair data.
#[derive(Clone, Debug, Default)]
struct RoomRepairData {
    requests: Vec<RepairRequest>,
}

/// Global repair queue resource. Missions register repair requests each tick;
/// creep jobs consume them to find repair targets.
///
/// This is an **ephemeral** resource -- it is rebuilt each tick during the
/// mission pre_run/run phase, similar to how the transfer queue works.
/// It does NOT need to be serialized.
#[derive(Default)]
pub struct RepairQueue {
    rooms: HashMap<RoomName, RoomRepairData>,
}

impl RepairQueue {
    /// Submit a repair request for a structure.
    pub fn request_repair(&mut self, request: RepairRequest) {
        self.rooms.entry(request.room).or_default().requests.push(request);
    }

    /// Submit multiple repair requests at once.
    pub fn request_repairs(&mut self, requests: impl IntoIterator<Item = RepairRequest>) {
        for request in requests {
            self.request_repair(request);
        }
    }

    /// Get the highest-priority repair target for a room.
    /// Optionally filter by minimum priority.
    pub fn get_best_target(&self, room: RoomName, minimum_priority: Option<RepairPriority>) -> Option<&RepairRequest> {
        let room_data = self.rooms.get(&room)?;

        room_data
            .requests
            .iter()
            .filter(|r| minimum_priority.map(|min| r.priority >= min).unwrap_or(true))
            .max_by(|a, b| {
                // Sort by priority first, then by lowest HP fraction.
                a.priority.cmp(&b.priority).then_with(|| {
                    let a_frac = if a.max_hits > 0 {
                        a.current_hits as f64 / a.max_hits as f64
                    } else {
                        1.0
                    };
                    let b_frac = if b.max_hits > 0 {
                        b.current_hits as f64 / b.max_hits as f64
                    } else {
                        1.0
                    };
                    // Lower fraction = more damaged = higher priority.
                    b_frac.partial_cmp(&a_frac).unwrap_or(std::cmp::Ordering::Equal)
                })
            })
    }

    /// Get the highest-priority repair target for a room that is within range of a position.
    pub fn get_best_target_in_range(
        &self,
        room: RoomName,
        pos: Position,
        range: u32,
        minimum_priority: Option<RepairPriority>,
    ) -> Option<&RepairRequest> {
        let room_data = self.rooms.get(&room)?;

        room_data
            .requests
            .iter()
            .filter(|r| minimum_priority.map(|min| r.priority >= min).unwrap_or(true))
            .filter(|r| r.structure_id.pos().in_range_to(pos, range))
            .max_by(|a, b| {
                a.priority.cmp(&b.priority).then_with(|| {
                    let a_frac = if a.max_hits > 0 {
                        a.current_hits as f64 / a.max_hits as f64
                    } else {
                        1.0
                    };
                    let b_frac = if b.max_hits > 0 {
                        b.current_hits as f64 / b.max_hits as f64
                    } else {
                        1.0
                    };
                    b_frac.partial_cmp(&a_frac).unwrap_or(std::cmp::Ordering::Equal)
                })
            })
    }

    /// Get all repair requests for a room, sorted by priority (highest first).
    pub fn get_room_requests(&self, room: RoomName) -> Vec<&RepairRequest> {
        let room_data = match self.rooms.get(&room) {
            Some(d) => d,
            None => return Vec::new(),
        };

        let mut requests: Vec<_> = room_data.requests.iter().collect();
        requests.sort_by_key(|r| std::cmp::Reverse(r.priority));
        requests
    }

    /// Check if a room has any repair requests at or above a given priority.
    pub fn has_requests(&self, room: RoomName, minimum_priority: Option<RepairPriority>) -> bool {
        self.rooms
            .get(&room)
            .map(|d| {
                d.requests
                    .iter()
                    .any(|r| minimum_priority.map(|min| r.priority >= min).unwrap_or(true))
            })
            .unwrap_or(false)
    }

    /// Clear all requests (called at the start of each tick).
    pub fn clear(&mut self) {
        self.rooms.clear();
    }
}

/// System that clears the repair queue at the start of each tick.
#[derive(Default)]
pub struct RepairQueueClearSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for RepairQueueClearSystem {
    type SystemData = Write<'a, RepairQueue>;

    fn run(&mut self, mut queue: Self::SystemData) {
        queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remoteobjectid::RemoteObjectId;
    use screeps::RoomCoordinate;

    // Pin: the HP-fraction tie-breaker guards `max_hits == 0` (treated as
    // fraction 1.0, i.e. "not damaged") instead of dividing by zero. The
    // review's RepairQueue-NaN concern was REFUTED because of this guard --
    // lock it so it stays.

    fn test_room() -> RoomName {
        "E0N0".parse().expect("valid room name")
    }

    fn test_request(priority: RepairPriority, current_hits: u32, max_hits: u32, id_hex: &str) -> RepairRequest {
        let room = test_room();
        let pos = Position::new(
            RoomCoordinate::new(25).expect("valid coordinate"),
            RoomCoordinate::new(25).expect("valid coordinate"),
            room,
        );

        RepairRequest {
            structure_id: RemoteStructureIdentifier::Container(RemoteObjectId::new_from_components(
                id_hex.parse().expect("valid object id"),
                pos,
            )),
            priority,
            current_hits,
            max_hits,
            room,
        }
    }

    #[test]
    fn get_best_target_handles_zero_max_hits_without_panic() {
        let mut queue = RepairQueue::default();
        queue.request_repair(test_request(RepairPriority::Medium, 0, 0, "5bbcab9099d4e7f1f9fb1be1"));

        let best = queue.get_best_target(test_room(), None).expect("expected a target");
        assert_eq!(best.max_hits, 0);
    }

    #[test]
    fn get_best_target_prefers_damaged_over_zero_max_hits() {
        let mut queue = RepairQueue::default();
        // Same priority: the zero-max-hits entry scores as fraction 1.0
        // ("undamaged") and must lose to a genuinely damaged structure.
        queue.request_repair(test_request(RepairPriority::Medium, 0, 0, "5bbcab9099d4e7f1f9fb1be1"));
        queue.request_repair(test_request(RepairPriority::Medium, 50, 100, "5bbcab9099d4e7f1f9fb1be2"));

        let best = queue.get_best_target(test_room(), None).expect("expected a target");
        assert_eq!(best.max_hits, 100);
    }

    #[test]
    fn get_best_target_orders_by_priority_first() {
        let mut queue = RepairQueue::default();
        queue.request_repair(test_request(RepairPriority::Low, 1, 100, "5bbcab9099d4e7f1f9fb1be1"));
        queue.request_repair(test_request(RepairPriority::Critical, 99, 100, "5bbcab9099d4e7f1f9fb1be2"));

        let best = queue.get_best_target(test_room(), None).expect("expected a target");
        assert_eq!(best.priority, RepairPriority::Critical);
    }
}
