//! Economy world state — JS-free value types over `screeps::Position`, mirroring
//! `screeps-combat-engine::state` (the worked overlay example): [`EconWorld`] **composes** the
//! shared kernel's [`MovementState`] (creep positions/bodies/fatigue live there) and adds the
//! economy overlay — sources, spawn structures, stores, dropped piles, per-creep stores + TTLs.
//!
//! **Determinism discipline (EP-6.13):** per-creep state is keyed by `u32` ids in `BTreeMap`s;
//! structure Vecs are construction-ordered with sequential u32 ids assigned by the builders; store
//! contents are canonical `BTreeMap`s (zero-amount keys never linger). [`EconWorld::state_digest`]
//! iterates only sorted/stable boundaries, so identical histories digest identically.
//!
//! **The one weight invariant:** sim-core's `SimCreep::carry_used` is the movement-fatigue weight
//! scalar; this crate keeps it EQUAL to the creep's store total after every mutation through the
//! single helper [`EconWorld::sync_carry_used`] (pinned by `carry_used_equals_store_total`).

use crate::constants::{extension_capacity, SPAWN_ENERGY_CAPACITY};
use screeps::{Part, Position};
use screeps_sim_core::{BoostTier, CreepId, MovementState, SimBody, SimCreep, StructureId};
use std::collections::BTreeMap;

/// Resource kinds the economy sim tracks. M0 flows only `Energy`; a few base minerals exist for
/// M6-readiness (stores/transfers already handle them; extractor/lab mechanics land M6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SimResource {
    Energy,
    Hydrogen,
    Oxygen,
    Utrium,
    Ghodium,
}

impl SimResource {
    /// Stable digest tag (NOT the game's resource id — a sim-local canonical byte).
    fn tag(self) -> u8 {
        match self {
            SimResource::Energy => 0,
            SimResource::Hydrogen => 1,
            SimResource::Oxygen => 2,
            SimResource::Utrium => 3,
            SimResource::Ghodium => 4,
        }
    }
}

/// A per-resource store with one shared capacity (engine general-store semantics: creeps,
/// containers, and storage share capacity across resource types; spawns/extensions are energy-only
/// and use a plain `store_energy` field instead).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimStore {
    /// Canonical contents: a key is present iff its amount is > 0 (kept so by [`add`](Self::add)/
    /// [`remove`](Self::remove) — required for stable digests and `==` comparisons).
    pub contents: BTreeMap<SimResource, u32>,
    pub capacity: u32,
}

impl SimStore {
    pub fn with_capacity(capacity: u32) -> Self {
        Self { contents: BTreeMap::new(), capacity }
    }

    pub fn amount(&self, r: SimResource) -> u32 {
        self.contents.get(&r).copied().unwrap_or(0)
    }

    /// Total across all resources (engine `_.sum(store)`).
    pub fn total(&self) -> u32 {
        self.contents.values().sum()
    }

    pub fn free(&self) -> u32 {
        self.capacity.saturating_sub(self.total())
    }

    /// Add up to `amount` of `r`, clamped to free capacity; returns the amount actually accepted.
    pub fn add(&mut self, r: SimResource, amount: u32) -> u32 {
        let accepted = amount.min(self.free());
        if accepted > 0 {
            *self.contents.entry(r).or_insert(0) += accepted;
        }
        accepted
    }

    /// Remove up to `amount` of `r`, clamped to what is present; returns the amount actually
    /// removed. Empty keys are dropped (canonical form).
    pub fn remove(&mut self, r: SimResource, amount: u32) -> u32 {
        let have = self.amount(r);
        let removed = amount.min(have);
        if removed > 0 {
            if removed == have {
                self.contents.remove(&r);
            } else if let Some(v) = self.contents.get_mut(&r) {
                *v -= removed;
            }
        }
        removed
    }
}

/// An energy source. `regen_at` is the engine's `nextRegenerationTime`: `None` until the first
/// harvest below capacity starts the 300-tick timer (engine-mechanics.md:445); the pool refills
/// when `tick >= regen_at − 1` (same line) and the timer clears.
#[derive(Clone, Debug)]
pub struct SimSource {
    pub pos: Position,
    pub energy: u32,
    /// 3000 owned/reserved, 1500 neutral, 4000 keeper (engine-mechanics.md:466) — set by the builder.
    pub capacity: u32,
    pub regen_at: Option<u32>,
}

/// M6 stub: a mineral deposit exists as world furniture only — NO extractor/harvest mechanics in
/// M0 (`mineral_type` stays an opaque u8 until M6 types the mineral economy).
#[derive(Clone, Debug)]
pub struct SimMineral {
    pub pos: Position,
    pub mineral_type: u8,
    pub amount: u32,
}

/// A creep mid-spawn: materializes when `tick >= done_at` (the busy-until source of truth; the
/// spec's separate `busy_until` field is folded in here — one clock, not two).
#[derive(Clone, Debug)]
pub struct PendingCreep {
    /// The creep id minted at intent time (sequential, deterministic).
    pub id: CreepId,
    pub body: Vec<Part>,
    /// Completion tick: `start_tick + CREEP_SPAWN_TIME × body.len()`; a fully blocked exit slips
    /// this +1/tick (engine-mechanics.md:242).
    pub done_at: u32,
}

/// A spawn structure: an energy-only store (cap [`SPAWN_ENERGY_CAPACITY`]) + at most one creep
/// in production.
#[derive(Clone, Debug)]
pub struct SimSpawn {
    pub id: StructureId,
    pub pos: Position,
    pub store_energy: u32,
    pub spawning: Option<PendingCreep>,
}

/// An extension: energy-only store. `capacity` is what the tick pipeline maintains — the engine
/// RECOMPUTES extension capacity from the CURRENT controller level every tick
/// (`extensions/tick.js:11`; the 50/100/200 table at engine-mechanics.md:456), and the resolver's
/// step 0 does the same whenever the world has a controller. The builder's value is only the
/// starting point (and stays authoritative in controller-less scenario worlds).
#[derive(Clone, Debug)]
pub struct SimExtension {
    pub id: StructureId,
    pub pos: Position,
    pub store_energy: u32,
    pub capacity: u32,
}

/// A container: general store. `hits` exists now; decay lands M1 (engine-mechanics.md:429).
#[derive(Clone, Debug)]
pub struct SimContainer {
    pub id: StructureId,
    pub pos: Position,
    pub store: SimStore,
    pub hits: u32,
}

/// Room storage: general store.
#[derive(Clone, Debug)]
pub struct SimStorage {
    pub pos: Position,
    pub store: SimStore,
}

/// The room controller — struct only in M0 (upgrade mechanics land M2; `UpgradeController` is
/// deliberately NOT in the M0 intent vocabulary).
#[derive(Clone, Debug)]
pub struct SimController {
    pub level: u8,
    pub progress: u32,
    pub downgrade_ticks: u32,
}

/// A road as a decaying STRUCTURE (hits; decay/wearout land M1). The road's movement effect
/// (fatigue rate 1) lives in sim-core's `SimTerrain::roads` — builders must keep both in sync.
#[derive(Clone, Debug)]
pub struct SimRoad {
    pub pos: Position,
    pub hits: u32,
    pub hits_max: u32,
}

/// A dropped-resource pile. Piles are merged per (pos, resource) by the drop helper and decay
/// `ceil(amount/1000)`/tick (engine-mechanics.md:431).
#[derive(Clone, Debug)]
pub struct SimDropped {
    pub pos: Position,
    pub resource: SimResource,
    pub amount: u32,
}

/// One room's economy state for a tick: the shared movement world plus the economy overlay.
/// The tick counter is `movement.tick` — the ONE source of truth (no shadow copy here).
#[derive(Clone, Debug)]
pub struct EconWorld {
    /// The shared movement/world kernel state (tick, terrain, creeps, exemptions — ADR 0033). The
    /// economy tick calls `screeps_sim_core::resolve_movement` over this at its movement point.
    pub movement: MovementState,
    pub sources: Vec<SimSource>,
    pub minerals: Vec<SimMineral>,
    pub spawns: Vec<SimSpawn>,
    pub extensions: Vec<SimExtension>,
    pub containers: Vec<SimContainer>,
    pub storage: Option<SimStorage>,
    pub controller: Option<SimController>,
    pub roads: Vec<SimRoad>,
    pub dropped: Vec<SimDropped>,
    /// Per-creep resource stores (creep positions/bodies live in `movement.creeps`).
    pub creep_stores: BTreeMap<CreepId, SimStore>,
    /// The engine's `ageTime` per creep: death (dropping the whole store to ground) fires on the
    /// first tick where `tick + 1 >=` this value — the engine's `gameTime >= ageTime − 1`
    /// boundary (engine-mechanics.md:57).
    pub creep_ttl: BTreeMap<CreepId, u32>,
    next_structure_id: StructureId,
    next_creep_id: CreepId,
}

impl Default for EconWorld {
    fn default() -> Self {
        Self {
            movement: MovementState::default(),
            sources: Vec::new(),
            minerals: Vec::new(),
            spawns: Vec::new(),
            extensions: Vec::new(),
            containers: Vec::new(),
            storage: None,
            controller: None,
            roads: Vec::new(),
            dropped: Vec::new(),
            creep_stores: BTreeMap::new(),
            creep_ttl: BTreeMap::new(),
            next_structure_id: 1,
            next_creep_id: 1,
        }
    }
}

impl EconWorld {
    /// The current tick — forwarded from the movement state (the one source of truth).
    pub fn tick(&self) -> u32 {
        self.movement.tick
    }

    fn mint_structure_id(&mut self) -> StructureId {
        let id = self.next_structure_id;
        self.next_structure_id += 1;
        id
    }

    pub(crate) fn mint_creep_id(&mut self) -> CreepId {
        let id = self.next_creep_id;
        self.next_creep_id += 1;
        id
    }

    // ── Builders (sequential ids; construction order is part of the world's identity) ──────────

    pub fn add_source(&mut self, pos: Position, capacity: u32) -> usize {
        self.sources.push(SimSource { pos, energy: capacity, capacity, regen_at: None });
        self.sources.len() - 1
    }

    pub fn add_mineral(&mut self, pos: Position, mineral_type: u8, amount: u32) -> usize {
        self.minerals.push(SimMineral { pos, mineral_type, amount });
        self.minerals.len() - 1
    }

    /// A spawn, born FULL (a freshly placed spawn holds its 300) — drain it in-scenario if needed.
    pub fn add_spawn(&mut self, pos: Position) -> usize {
        let id = self.mint_structure_id();
        self.spawns.push(SimSpawn { id, pos, store_energy: SPAWN_ENERGY_CAPACITY, spawning: None });
        self.spawns.len() - 1
    }

    /// An extension, born empty; `rcl` sets the STARTING capacity (engine-mechanics.md:456). When
    /// the world has a controller, the tick pipeline re-derives capacity from the controller's
    /// CURRENT level every tick (the engine recomputes it per tick — `extensions/tick.js:11`), so
    /// this argument only decides anything in controller-less scenario worlds.
    pub fn add_extension(&mut self, pos: Position, rcl: u8) -> usize {
        let id = self.mint_structure_id();
        self.extensions.push(SimExtension { id, pos, store_energy: 0, capacity: extension_capacity(rcl) });
        self.extensions.len() - 1
    }

    pub fn add_container(&mut self, pos: Position, capacity: u32, hits: u32) -> usize {
        let id = self.mint_structure_id();
        self.containers.push(SimContainer { id, pos, store: SimStore::with_capacity(capacity), hits });
        self.containers.len() - 1
    }

    pub fn set_storage(&mut self, pos: Position, capacity: u32) {
        self.storage = Some(SimStorage { pos, store: SimStore::with_capacity(capacity) });
    }

    /// A road structure; also registers the tile in the movement terrain (fatigue rate 1) so the
    /// structure and its movement effect cannot drift apart. The tile goes into whatever terrain
    /// [`MovementState::terrain_for`] actually reads for this room — an existing room override, or
    /// else the DEFAULT terrain. Never `terrain_mut`: its `or_default()` would mint an EMPTY
    /// override that silently shadows the default terrain's walls/swamps for the whole room.
    pub fn add_road(&mut self, pos: Position, hits: u32, hits_max: u32) -> usize {
        let key = (pos.x().u8(), pos.y().u8());
        match self.movement.rooms.get_mut(&pos.room_name()) {
            Some(t) => {
                t.roads.insert(key);
            }
            None => {
                self.movement.terrain.roads.insert(key);
            }
        }
        self.roads.push(SimRoad { pos, hits, hits_max });
        self.roads.len() - 1
    }

    /// Drop `amount` of `r` at `pos`, merging into an existing same-(pos, resource) pile (the
    /// engine keeps one pile per resource per tile).
    pub fn drop_resource(&mut self, pos: Position, r: SimResource, amount: u32) {
        if amount == 0 {
            return;
        }
        if let Some(pile) = self.dropped.iter_mut().find(|p| p.pos == pos && p.resource == r) {
            pile.amount += amount;
        } else {
            self.dropped.push(SimDropped { pos, resource: r, amount });
        }
    }

    /// Register a creep directly (scenario setup): places it in the movement state, creates its
    /// store (capacity from the body's CARRY parts), sets its `ageTime` to `now + ttl`, and
    /// returns its id. Like the engine, the creep lives ticks `now .. now + ttl − 1` and dies
    /// (dropping its store) during tick `now + ttl − 1` (the `ageTime − 1` boundary).
    pub fn add_creep(&mut self, pos: Position, body: &[Part], ttl: u32) -> CreepId {
        let id = self.mint_creep_id();
        let sim_body = SimBody::unboosted(body);
        let store_capacity = creep_store_capacity(&sim_body);
        self.movement.creeps.push(SimCreep { id, owner: 0, pos, body: sim_body, fatigue: 0, carry_used: 0 });
        self.creep_stores.insert(id, SimStore::with_capacity(store_capacity));
        self.creep_ttl.insert(id, self.movement.tick + ttl);
        id
    }

    // ── Accessors ───────────────────────────────────────────────────────────────────────────────

    pub fn creep(&self, id: CreepId) -> Option<&SimCreep> {
        self.movement.creeps.iter().find(|c| c.id == id)
    }

    pub fn creep_mut(&mut self, id: CreepId) -> Option<&mut SimCreep> {
        self.movement.creeps.iter_mut().find(|c| c.id == id)
    }

    /// THE weight-invariant helper: set the movement creep's `carry_used` (its move-fatigue load
    /// scalar) to its store total. Called after every store mutation; pinned by
    /// `carry_used_equals_store_total`.
    pub fn sync_carry_used(&mut self, id: CreepId) {
        let total = self.creep_stores.get(&id).map(SimStore::total).unwrap_or(0);
        if let Some(c) = self.creep_mut(id) {
            c.carry_used = total;
        }
    }

    /// Room-wide spawn-lane energy: all spawns + all extensions (the engine's `energyAvailable`).
    pub fn room_spawn_energy(&self) -> u32 {
        self.spawns.iter().map(|s| s.store_energy).sum::<u32>()
            + self.extensions.iter().map(|e| e.store_energy).sum::<u32>()
    }

    /// Whether `pos` can host a newborn creep: in-terrain walkable (not a natural wall), no living
    /// creep, and no obstacle object (spawn/extension/storage/source/mineral — roads and containers
    /// are walkable, matching the engine's `OBSTACLE_OBJECT_TYPES`).
    pub fn is_walkable(&self, pos: Position) -> bool {
        let (x, y) = (pos.x().u8(), pos.y().u8());
        if self.movement.terrain_for(pos.room_name()).is_wall(x, y) {
            return false;
        }
        if self.movement.creeps.iter().any(|c| c.is_alive() && c.pos == pos) {
            return false;
        }
        !(self.spawns.iter().any(|s| s.pos == pos)
            || self.extensions.iter().any(|e| e.pos == pos)
            || self.storage.as_ref().is_some_and(|s| s.pos == pos)
            || self.sources.iter().any(|s| s.pos == pos)
            || self.minerals.iter().any(|m| m.pos == pos))
    }

    /// Total economy stock per resource: every store (creeps, spawns, extensions, containers,
    /// storage) plus dropped piles. Source/mineral pools are NOT stock — they mint into the economy
    /// at harvest time (the ledger's `harvested` source), per the ADR 0040 §D7 accounting.
    pub fn stocks(&self) -> BTreeMap<SimResource, u64> {
        let mut out: BTreeMap<SimResource, u64> = BTreeMap::new();
        let mut bump = |r: SimResource, v: u64| {
            if v > 0 {
                *out.entry(r).or_insert(0) += v;
            }
        };
        for store in self.creep_stores.values() {
            for (&r, &v) in &store.contents {
                bump(r, v as u64);
            }
        }
        for s in &self.spawns {
            bump(SimResource::Energy, s.store_energy as u64);
        }
        for e in &self.extensions {
            bump(SimResource::Energy, e.store_energy as u64);
        }
        for c in &self.containers {
            for (&r, &v) in &c.store.contents {
                bump(r, v as u64);
            }
        }
        if let Some(st) = &self.storage {
            for (&r, &v) in &st.store.contents {
                bump(r, v as u64);
            }
        }
        for d in &self.dropped {
            bump(d.resource, d.amount as u64);
        }
        out
    }

    /// A stable digest of the full economy state — the determinism fence's instrument. Iterates
    /// only sorted/stable boundaries: `BTreeMap`s in key order, structure Vecs in construction
    /// order, movement creeps sorted by id.
    pub fn state_digest(&self) -> u64 {
        let mut d = Fnv::new();
        d.u32(self.movement.tick);
        let mut creeps: Vec<&SimCreep> = self.movement.creeps.iter().collect();
        creeps.sort_by_key(|c| c.id);
        for c in creeps {
            d.u32(c.id);
            d.pos(c.pos);
            d.u32(c.fatigue);
            d.u32(c.carry_used);
            d.u32(c.body.hits);
            for p in &c.body.parts {
                d.u8(part_tag(p.part));
            }
        }
        for s in &self.sources {
            d.pos(s.pos);
            d.u32(s.energy);
            d.u32(s.capacity);
            d.opt_u32(s.regen_at);
        }
        for m in &self.minerals {
            d.pos(m.pos);
            d.u8(m.mineral_type);
            d.u32(m.amount);
        }
        for s in &self.spawns {
            d.u32(s.id);
            d.pos(s.pos);
            d.u32(s.store_energy);
            match &s.spawning {
                None => d.u8(0),
                Some(p) => {
                    d.u8(1);
                    d.u32(p.id);
                    d.u32(p.done_at);
                    for part in &p.body {
                        d.u8(part_tag(*part));
                    }
                }
            }
        }
        for e in &self.extensions {
            d.u32(e.id);
            d.pos(e.pos);
            d.u32(e.store_energy);
            d.u32(e.capacity);
        }
        for c in &self.containers {
            d.u32(c.id);
            d.pos(c.pos);
            d.u32(c.hits);
            d.store(&c.store);
        }
        match &self.storage {
            None => d.u8(0),
            Some(s) => {
                d.u8(1);
                d.pos(s.pos);
                d.store(&s.store);
            }
        }
        match &self.controller {
            None => d.u8(0),
            Some(c) => {
                d.u8(1);
                d.u8(c.level);
                d.u32(c.progress);
                d.u32(c.downgrade_ticks);
            }
        }
        for r in &self.roads {
            d.pos(r.pos);
            d.u32(r.hits);
            d.u32(r.hits_max);
        }
        // Dropped piles: canonicalize by (pos, resource) so pile-creation order (an artifact of
        // processing order for DISTINCT tiles, which reorder must not leak through) never shows.
        let mut piles: Vec<&SimDropped> = self.dropped.iter().filter(|p| p.amount > 0).collect();
        piles.sort_by_key(|p| (p.pos.room_name().to_string(), p.pos.y().u8(), p.pos.x().u8(), p.resource.tag()));
        for p in piles {
            d.pos(p.pos);
            d.u8(p.resource.tag());
            d.u32(p.amount);
        }
        for (&id, store) in &self.creep_stores {
            d.u32(id);
            d.store(store);
        }
        for (&id, &ttl) in &self.creep_ttl {
            d.u32(id);
            d.u32(ttl);
        }
        d.finish()
    }
}

/// Store capacity from a body: `CARRY_CAPACITY × boost-mult` per CARRY part (boost-aware for
/// M6-readiness; M0 bodies are unboosted).
pub fn creep_store_capacity(body: &SimBody) -> u32 {
    body.parts
        .iter()
        .filter(|p| p.part == Part::Carry)
        .map(|p| screeps_sim_core::constants::CARRY_CAPACITY * carry_mult(p.boost))
        .sum()
}

fn carry_mult(b: BoostTier) -> u32 {
    b.carry_capacity_mult()
}

fn part_tag(p: Part) -> u8 {
    match p {
        Part::Move => 0,
        Part::Carry => 1,
        Part::Work => 2,
        Part::Attack => 3,
        Part::RangedAttack => 4,
        Part::Heal => 5,
        Part::Tough => 6,
        Part::Claim => 7,
        _ => 255,
    }
}

/// FNV-1a 64 over a canonical byte stream — tiny, dependency-free, stable across runs/platforms.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn u8(&mut self, v: u8) {
        self.0 ^= v as u64;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
    }
    fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.u8(b);
        }
    }
    fn opt_u32(&mut self, v: Option<u32>) {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1);
                self.u32(x);
            }
        }
    }
    fn pos(&mut self, p: Position) {
        for b in p.room_name().to_string().bytes() {
            self.u8(b);
        }
        self.u8(p.x().u8());
        self.u8(p.y().u8());
    }
    fn store(&mut self, s: &SimStore) {
        self.u32(s.capacity);
        for (&r, &v) in &s.contents {
            self.u8(r.tag());
            self.u32(v);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{RoomCoordinate, RoomName};

    fn pos(x: u8, y: u8) -> Position {
        let room: RoomName = "W1N1".parse().unwrap();
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room)
    }

    #[test]
    fn store_add_remove_clamp_and_stay_canonical() {
        let mut s = SimStore::with_capacity(100);
        assert_eq!(s.add(SimResource::Energy, 60), 60);
        assert_eq!(s.add(SimResource::Hydrogen, 60), 40, "clamped to shared free capacity");
        assert_eq!(s.total(), 100);
        assert_eq!(s.free(), 0);
        assert_eq!(s.remove(SimResource::Hydrogen, 999), 40, "clamped to what is present");
        assert!(!s.contents.contains_key(&SimResource::Hydrogen), "empty key dropped (canonical)");
        assert_eq!(s.remove(SimResource::Utrium, 5), 0, "absent resource removes nothing");
    }

    #[test]
    fn drop_resource_merges_same_tile_same_resource_piles() {
        let mut w = EconWorld::default();
        w.drop_resource(pos(5, 5), SimResource::Energy, 10);
        w.drop_resource(pos(5, 5), SimResource::Energy, 7);
        w.drop_resource(pos(5, 5), SimResource::Hydrogen, 3);
        w.drop_resource(pos(6, 5), SimResource::Energy, 2);
        assert_eq!(w.dropped.len(), 3, "one pile per (tile, resource)");
        assert_eq!(w.dropped[0].amount, 17);
    }

    #[test]
    fn stocks_cover_every_store_and_dropped_but_not_source_pools() {
        let mut w = EconWorld::default();
        w.add_source(pos(10, 10), 3000); // NOT stock — mints at harvest
        w.add_spawn(pos(20, 20)); // born full: 300
        let e = w.add_extension(pos(21, 20), 8);
        w.extensions[e].store_energy = 150;
        let c = w.add_container(pos(22, 20), 2000, 250_000);
        w.containers[c].store.add(SimResource::Energy, 500);
        w.containers[c].store.add(SimResource::Ghodium, 25);
        w.set_storage(pos(23, 20), 1_000_000);
        w.storage.as_mut().unwrap().store.add(SimResource::Energy, 1000);
        let id = w.add_creep(pos(24, 20), &[Part::Carry, Part::Move], 1500);
        w.creep_stores.get_mut(&id).unwrap().add(SimResource::Energy, 30);
        w.sync_carry_used(id);
        w.drop_resource(pos(25, 20), SimResource::Energy, 40);
        let stocks = w.stocks();
        assert_eq!(stocks[&SimResource::Energy], 300 + 150 + 500 + 1000 + 30 + 40);
        assert_eq!(stocks[&SimResource::Ghodium], 25);
    }

    /// The ONE weight invariant: `carry_used` (movement fatigue load) == store total, maintained
    /// solely through [`EconWorld::sync_carry_used`].
    #[test]
    fn carry_used_equals_store_total() {
        let mut w = EconWorld::default();
        let id = w.add_creep(pos(1, 1), &[Part::Carry, Part::Carry, Part::Move], 1500);
        assert_eq!(w.creep(id).unwrap().carry_used, 0);
        w.creep_stores.get_mut(&id).unwrap().add(SimResource::Energy, 60);
        w.sync_carry_used(id);
        assert_eq!(w.creep(id).unwrap().carry_used, 60);
        w.creep_stores.get_mut(&id).unwrap().remove(SimResource::Energy, 15);
        w.sync_carry_used(id);
        assert_eq!(w.creep(id).unwrap().carry_used, 45);
        assert_eq!(w.creep_stores[&id].total(), w.creep(id).unwrap().carry_used);
    }

    #[test]
    fn digest_is_stable_and_sensitive() {
        let build = || {
            let mut w = EconWorld::default();
            w.add_source(pos(10, 10), 1500);
            w.add_spawn(pos(20, 20));
            w.add_creep(pos(21, 20), &[Part::Work, Part::Carry, Part::Move], 1500);
            w
        };
        let a = build();
        let b = build();
        assert_eq!(a.state_digest(), b.state_digest(), "identical construction → identical digest");
        let mut c = build();
        c.sources[0].energy -= 1;
        assert_ne!(a.state_digest(), c.state_digest(), "digest sees a 1-energy difference");
    }
}
