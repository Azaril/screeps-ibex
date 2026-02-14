use screeps::*;
use specs::*;
use std::collections::HashMap;

/// Priority for boost production requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoostPriority {
    /// Normal priority -- produce when convenient.
    Normal,
    /// High priority -- prioritize over normal lab reactions.
    High,
    /// Critical -- needed immediately for active defense.
    Critical,
}

/// A request for boost compounds from the military system.
#[derive(Clone, Debug)]
pub struct BoostRequest {
    /// The mission entity requesting the boost.
    pub requester: Entity,
    /// The compound type needed (e.g., XGHO2, XLHO2).
    pub compound: ResourceType,
    /// Number of body parts to boost (determines amount needed: 30 per part).
    pub parts_to_boost: u32,
    /// Priority of this request.
    pub priority: BoostPriority,
}

impl BoostRequest {
    pub fn new(requester: Entity, compound: ResourceType, parts_to_boost: u32, priority: BoostPriority) -> Self {
        BoostRequest {
            requester,
            compound,
            parts_to_boost,
            priority,
        }
    }

    /// Total compound amount needed (30 per body part).
    pub fn amount_needed(&self) -> u32 {
        self.parts_to_boost * 30
    }
}

/// Allocation of boost resources that are ready for use.
#[derive(Clone, Debug)]
pub struct BoostAllocation {
    /// The compound type available.
    pub compound: ResourceType,
    /// Amount available in the lab.
    pub amount: u32,
    /// Room where the lab is located.
    pub room: RoomName,
}

/// Global boost request/fulfillment queue.
/// Ephemeral -- rebuilt each tick, not serialized.
#[derive(Default)]
pub struct BoostQueue {
    /// Pending requests from military missions.
    pub requests: Vec<BoostRequest>,
    /// Fulfilled allocations, keyed by requester entity.
    pub ready: HashMap<Entity, Vec<BoostAllocation>>,
}

impl BoostQueue {
    pub fn new() -> Self {
        BoostQueue {
            requests: Vec::new(),
            ready: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.requests.clear();
        self.ready.clear();
    }

    /// Add a boost request.
    pub fn request(&mut self, request: BoostRequest) {
        self.requests.push(request);
    }

    /// Mark a boost allocation as ready for a requester.
    pub fn mark_ready(&mut self, requester: Entity, allocation: BoostAllocation) {
        self.ready.entry(requester).or_default().push(allocation);
    }

    /// Check if all requested boosts for a requester are ready.
    pub fn is_ready(&self, requester: Entity) -> bool {
        let requests: Vec<_> = self.requests.iter().filter(|r| r.requester == requester).collect();

        if requests.is_empty() {
            return true;
        }

        let allocations = match self.ready.get(&requester) {
            Some(a) => a,
            None => return false,
        };

        // Check that every requested compound has sufficient allocation.
        for request in &requests {
            let allocated: u32 = allocations
                .iter()
                .filter(|a| a.compound == request.compound)
                .map(|a| a.amount)
                .sum();

            if allocated < request.amount_needed() {
                return false;
            }
        }

        true
    }

    /// Get all pending requests sorted by priority (highest first).
    pub fn pending_requests(&self) -> Vec<&BoostRequest> {
        let mut sorted: Vec<_> = self.requests.iter().collect();
        sorted.sort_by_key(|b| std::cmp::Reverse(b.priority));
        sorted
    }
}
