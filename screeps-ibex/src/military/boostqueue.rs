use screeps::*;
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

/// ADR 0041 P3 — a request to load boost compounds for ONE creep (the `AwaitBoost` member). Keyed by
/// the CREEP NAME — the stable per-member identity that exists today (never a raw `Entity`, the
/// recyclable-index hazard ADR 0001 bans; ADR 0011's minted `DemandId` supersedes this key when the
/// spawn orchestrator lands). Re-filed EVERY TICK by the owner (the SquadManager) while the member
/// still has unboosted parts — the queue is ephemeral and self-healing, nothing serialized.
#[derive(Clone, Debug)]
pub struct BoostRequest {
    /// The awaiting creep's name (the stable key).
    pub creep: String,
    /// The home room whose labs should service this (the room the member awaits in).
    pub room: RoomName,
    /// The compounds still needed: `(compound, parts_to_boost)` — 30 mineral + 20 energy per part
    /// (engine `boostCreep`).
    pub compounds: Vec<(ResourceType, u32)>,
    /// Priority of this request (defense = Critical preempts reactions; offense = High).
    pub priority: BoostPriority,
}

/// A lab loaded and ready to boost one compound for one requester.
#[derive(Clone, Debug)]
pub struct BoostAllocation {
    /// The compound the lab holds.
    pub compound: ResourceType,
    /// The loaded lab — the tile the awaiting creep walks adjacent to.
    pub lab: ObjectId<StructureLab>,
}

/// Global boost request/fulfillment queue (ADR 0041 P3 / ADR 0010 §4 — the IBEX-027 closure).
/// EPHEMERAL — cleared at the top of every tick (`game_loop`), re-filed by the owners, re-marked by
/// the labs; nothing serialized. Producer: the SquadManager (owners file demands). Fulfiller: the
/// room's `LabsMission` (loads labs via the transfer system, calls `mark_ready`). Consumer: the
/// member's `AwaitBoost` job state (walks to the ready lab; the sole `boost_creep` site).
#[derive(Default)]
pub struct BoostQueue {
    /// Pending requests, in file order (deterministic).
    pub requests: Vec<BoostRequest>,
    /// Ready allocations, keyed by creep name.
    pub ready: HashMap<String, Vec<BoostAllocation>>,
}

impl BoostQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Full reset (tests).
    pub fn clear(&mut self) {
        self.requests.clear();
        self.ready.clear();
    }

    /// STAGED clears (the tick order is PreRun → RunMission (labs FULFIL) → SquadManager (PRODUCES)
    /// → RunJob (CONSUMES), so each half is cleared by its owner at its stage): requests are wiped
    /// by the producer right before re-filing (the labs consumed the previous tick's filing
    /// earlier this tick) …
    pub fn clear_requests(&mut self) {
        self.requests.clear();
    }

    /// … and ready marks are wiped at TICK START (`game_loop`), remade by the labs at RunMission,
    /// consumed by the jobs at RunJob the same tick.
    pub fn clear_ready(&mut self) {
        self.ready.clear();
    }

    /// File (or re-file) a request. Owners call this every tick the need persists.
    pub fn request(&mut self, request: BoostRequest) {
        self.requests.push(request);
    }

    /// Mark one compound's lab loaded for a requester.
    pub fn mark_ready(&mut self, creep: &str, allocation: BoostAllocation) {
        self.ready.entry(creep.to_string()).or_default().push(allocation);
    }

    /// The ready allocations for a creep (empty slice when none).
    pub fn ready_for(&self, creep: &str) -> &[BoostAllocation] {
        self.ready.get(creep).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Pending requests for one room, priority-sorted (highest first; stable within a priority — the
    /// file order). The labs mission services these.
    pub fn pending_for_room(&self, room: RoomName) -> Vec<&BoostRequest> {
        let mut out: Vec<&BoostRequest> = self.requests.iter().filter(|r| r.room == room).collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.priority));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The staged-queue mechanics: per-room filtering + priority order, per-creep ready marks, and
    /// the two owner-staged clears wiping only their half.
    #[test]
    fn queue_stages_rooms_priorities_and_clears() {
        let mut q = BoostQueue::new();
        let room_a: RoomName = "W1N1".parse().unwrap();
        let room_b: RoomName = "W2N2".parse().unwrap();
        let req = |name: &str, room: RoomName, prio: BoostPriority| BoostRequest {
            creep: name.to_string(),
            room,
            compounds: vec![(ResourceType::CatalyzedLemergiumAlkalide, 3)],
            priority: prio,
        };
        q.request(req("a", room_a, BoostPriority::Normal));
        q.request(req("b", room_b, BoostPriority::High));
        q.request(req("c", room_a, BoostPriority::Critical));
        let pending: Vec<&str> = q.pending_for_room(room_a).iter().map(|r| r.creep.as_str()).collect();
        assert_eq!(pending, vec!["c", "a"], "room-filtered, Critical first");

        let lab_id: ObjectId<StructureLab> = "5bbcab9099cf1a5a4d8ac000".parse().unwrap();
        q.mark_ready("a", BoostAllocation { compound: ResourceType::CatalyzedLemergiumAlkalide, lab: lab_id });
        assert_eq!(q.ready_for("a").len(), 1);
        assert!(q.ready_for("b").is_empty());

        q.clear_requests();
        assert!(q.pending_for_room(room_a).is_empty(), "requests wiped by the producer's stage clear");
        assert_eq!(q.ready_for("a").len(), 1, "…without touching the ready half");
        q.clear_ready();
        assert!(q.ready_for("a").is_empty(), "ready wiped at tick start");
    }
}
