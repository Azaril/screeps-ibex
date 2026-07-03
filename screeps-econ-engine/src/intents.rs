//! Economy intents — the per-tick action vocabulary of the economy layer. [`EconIntents`] embeds
//! the kernel's [`MoveIntents`] (the ADR 0033 layering contract: the movement mechanism only ever
//! sees the movement part) and adds the economy verbs. **M0 vocabulary:** Harvest / Transfer /
//! Withdraw / Pickup / SpawnCreep. Build/Repair/UpgradeController are deliberately absent until
//! M1/M2 — a decision routine driving this layer cannot express them at the type level.

use crate::state::SimResource;
use screeps::Part;
use screeps_sim_core::{CreepId, MoveIntents};

/// A structure store target for transfer/withdraw, by construction index into the corresponding
/// [`crate::EconWorld`] Vec (indices are stable — M0 never removes structures).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructRef {
    Spawn(usize),
    Extension(usize),
    Container(usize),
    Storage,
}

/// One economy action. Actions ride in [`EconIntents::actions`] as `(CreepId, EconAction)` pairs;
/// for [`SpawnCreep`](EconAction::SpawnCreep) — a STRUCTURE intent, not a creep's — the paired
/// creep id is GENUINELY ignored by the resolver (spawn requests are keyed by
/// `(spawn index, submission order)`, never by the paired id; convention: pass `0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EconAction {
    /// Harvest an adjacent source (Pipeline A — the bot's work-intent lane, `jobs/actions.rs`).
    Harvest { source_idx: usize },
    /// Move `amount` of `resource` from the creep's store into an adjacent structure (Pipeline D).
    Transfer { target: StructRef, resource: SimResource, amount: u32 },
    /// Move `amount` of `resource` from an adjacent structure into the creep's store (Pipeline D).
    Withdraw { target: StructRef, resource: SimResource, amount: u32 },
    /// Pick an adjacent dropped pile into the creep's store (Pipeline D). `dropped_idx` indexes
    /// [`crate::EconWorld::dropped`] as of the tick's START (piles are only compacted at the
    /// decay step, after all pickups).
    Pickup { dropped_idx: usize },
    /// Start spawning `body` at spawn `spawn_idx` (the spawn MECHANISM half; bid/priority ordering
    /// is the QUEUE layer's job — [`crate::spawn_queue`] — which decides what to request; this
    /// resolver only executes the request).
    SpawnCreep { spawn_idx: usize, body: Vec<Part> },
}

/// All actors' economy intents for one tick.
#[derive(Clone, Debug, Default)]
pub struct EconIntents {
    /// Per-creep movement (resolved by the kernel at the pipeline's movement point).
    pub moves: MoveIntents,
    /// Economy actions. Submission order is NOT semantic across creeps or across spawns (the
    /// resolver re-orders by creep id / spawn index — the determinism boundary). The two
    /// documented WITHIN-actor contracts where submission order does decide: a creep's duplicate
    /// same-pipeline actions mask first-submitted-wins (counted in the report), and same-tick
    /// duplicate requests to ONE spawn resolve first-submitted-wins. Drivers therefore only need
    /// deterministic per-actor emission, never a deterministic global order.
    pub actions: Vec<(CreepId, EconAction)>,
}

impl EconIntents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn act(&mut self, creep: CreepId, action: EconAction) -> &mut Self {
        self.actions.push((creep, action));
        self
    }

    /// Queue a spawn request (the ignored creep-id convention, spelled once).
    pub fn spawn(&mut self, spawn_idx: usize, body: Vec<Part>) -> &mut Self {
        self.actions.push((0, EconAction::SpawnCreep { spawn_idx, body }));
        self
    }
}
