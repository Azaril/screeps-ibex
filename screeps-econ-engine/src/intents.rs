//! Economy intents — the per-tick action vocabulary of the economy layer. [`EconIntents`] embeds
//! the kernel's [`MoveIntents`] (the ADR 0033 layering contract: the movement mechanism only ever
//! sees the movement part) and adds the economy verbs. **M2 vocabulary:** Harvest / Repair /
//! Build / UpgradeController / Transfer / Withdraw / Pickup / SpawnCreep.
//!
//! **Pipelines** (the bot's `jobs/actions.rs:27-56` model, matching the engine's conflict matrix
//! `creeps/intents.js:3-13`): Harvest/Repair/**Build** share Pipeline A (mutually exclusive —
//! harvest conflicts with build and repair, build conflicts with repair; engine-mechanics.md:70-72);
//! Transfer/Withdraw/Pickup share Pipeline D; **UpgradeController is Pipeline E, its OWN lane**
//! (`actions.rs:50-51`; absent from the engine conflict matrix — it coexists with everything).
//! *The M2 spec sketch said upgrade "shares Pipeline A with Harvest/Repair" — that is a spec
//! error against both the engine matrix and the bot's own flags; the E-lane is implemented and
//! the deviation-from-spec is documented here.*

use crate::state::SimResource;
use screeps::Part;
use screeps_sim_core::{CreepId, MoveIntents};

/// A structure target, by construction index into the corresponding [`crate::EconWorld`] Vec.
/// Transfer/withdraw accept the store-bearing variants; [`EconAction::Repair`] accepts the
/// hit-bearing ones (`Road`/`Container` in M1). **Index stability (M1):** roads and containers are
/// REMOVED (compacted) at the decay step when they die, so an index is valid only within the tick
/// whose world state it was read from — drivers re-derive indices from the world every tick (the
/// dropped-pile contract, generalized).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructRef {
    Spawn(usize),
    Extension(usize),
    Container(usize),
    Storage,
    /// A road structure (repair target only — roads have no store; a transfer/withdraw naming one
    /// is rejected).
    Road(usize),
}

/// One economy action. Actions ride in [`EconIntents::actions`] as `(CreepId, EconAction)` pairs;
/// for [`SpawnCreep`](EconAction::SpawnCreep) — a STRUCTURE intent, not a creep's — the paired
/// creep id is GENUINELY ignored by the resolver (spawn requests are keyed by
/// `(spawn index, submission order)`, never by the paired id; convention: pass `0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EconAction {
    /// Harvest an adjacent source (Pipeline A — the bot's work-intent lane, `jobs/actions.rs`).
    Harvest { source_idx: usize },
    /// Repair a hit-bearing structure within Chebyshev range 3 (Pipeline A — shares the one work
    /// intent per creep per tick with Harvest, `jobs/actions.rs:27-31`; this is exactly the S1
    /// leak mechanic: a repairing harvester SKIPS that tick's harvest). 100 hits/WORK/tick at
    /// 0.01 energy/hit, clamped by carried energy + missing hits (engine `creeps/repair.js`,
    /// engine-mechanics.md:118). ALIVE-WORK semantics are moot in-sim — no partial body damage is
    /// modeled, so every WORK part is always alive (noted per the M1 spec).
    Repair { target: StructRef },
    /// Move `amount` of `resource` from the creep's store into an adjacent structure (Pipeline D).
    Transfer { target: StructRef, resource: SimResource, amount: u32 },
    /// Move `amount` of `resource` from an adjacent structure into the creep's store (Pipeline D).
    Withdraw { target: StructRef, resource: SimResource, amount: u32 },
    /// Pick an adjacent dropped pile into the creep's store (Pipeline D). `dropped_idx` indexes
    /// [`crate::EconWorld::dropped`] as of the tick's START (piles are only compacted at the
    /// decay step, after all pickups).
    Pickup { dropped_idx: usize },
    /// Build a construction site within Chebyshev range 3 (Pipeline A — shares the work lane with
    /// Harvest/Repair, `jobs/actions.rs:27-34` / engine conflicts `creeps/intents.js:8,10`).
    /// 5 progress/WORK/tick at 1 energy per progress (`creeps/build.js:67-69,83`); completion
    /// materializes the structure (`build.js:108-293`). `site_idx` indexes
    /// [`crate::EconWorld::sites`] as of the tick's START (completed sites compact at the end of
    /// the work lane).
    Build { site_idx: usize },
    /// Upgrade THE room controller from within Chebyshev range 3 (Pipeline E — its own lane,
    /// `jobs/actions.rs:50-51`; coexists with a same-tick Pipeline-D withdraw, the live
    /// upgrader's parallel-refill idiom `controllerbehavior.rs:107-124`). 1 progress/WORK/tick at
    /// 1 energy per progress (`creeps/upgradeController.js:33-34,92`); RCL8 caps the room at
    /// 15 e/t shared across upgraders (`:42-52`).
    UpgradeController,
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
