//! Scenario builder for the combat-eval harness (P2.H5 / U1): compose `CombatWorld`s with terrain
//! (walls / swamps), passive structures (constructed walls / ramparts / spawns / perimeters), and
//! firing towers — the room variety (open skirmish / walled-with-gap / rampart bunker / tower nest /
//! corridor / mixed base) the EXP-* register needs beyond the trivial open-field cases.
//!
//! Pure compositions of existing engine fields (`CombatWorld.terrain/.structures/.towers/
//! .safe_mode_owner`); adds no engine behavior. All synthesized coordinates are clamped/bounds-
//! checked to 0..=49 so a perimeter or tower nest flush against the edge can never panic.

use crate::opponents::{world_from_units, Unit};
use screeps::{Position, RoomCoordinate, RoomName};
use screeps_combat_engine::{CombatWorld, PlayerId, SimStructure, SimTower, StructureId, StructureKind};

/// Structure ids start high so they never collide with creep ids (creeps start at 1 in `world_from_units`).
const STRUCT_ID_BASE: StructureId = 1_000_000;
/// Engine structure hit pools (mirror Screeps: a spawn is 5000, a tower 3000 hits).
const SPAWN_HITS: u32 = 5_000;
const TOWER_HITS: u32 = 3_000;
const ROOM_MAX: u8 = 49;

/// Fluent builder over a single-room [`CombatWorld`]. Terrain/structure conveniences chain by value
/// (`.wall_column(..).rampart(..)`); `structure`/`tower` take `&mut self` and return the minted id
/// for scenarios that need to assert on a specific structure.
pub struct ScenarioBuilder {
    world: CombatWorld,
    room: RoomName,
    next_struct_id: StructureId,
}

impl ScenarioBuilder {
    /// Seed the world with two sides' creeps (via [`world_from_units`]), then layer terrain/structures.
    pub fn from_units(room: RoomName, a_owner: PlayerId, a_units: &[Unit], b_owner: PlayerId, b_units: &[Unit]) -> Self {
        Self { world: world_from_units(a_owner, a_units, b_owner, b_units), room, next_struct_id: STRUCT_ID_BASE }
    }
    /// An empty room to fill from scratch.
    pub fn empty(room: RoomName) -> Self {
        Self { world: CombatWorld::default(), room, next_struct_id: STRUCT_ID_BASE }
    }

    /// In-room position, clamped to 0..=49 so a synthesized tile can never panic `RoomCoordinate::new`.
    fn pos(&self, x: u8, y: u8) -> Position {
        let cx = RoomCoordinate::new(x.min(ROOM_MAX)).expect("clamped <= 49");
        let cy = RoomCoordinate::new(y.min(ROOM_MAX)).expect("clamped <= 49");
        Position::new(cx, cy, self.room)
    }
    fn alloc_id(&mut self) -> StructureId {
        let id = self.next_struct_id;
        self.next_struct_id += 1;
        id
    }

    // ── terrain ──
    pub fn wall(mut self, x: u8, y: u8) -> Self {
        self.world.terrain.walls.insert((x.min(ROOM_MAX), y.min(ROOM_MAX)));
        self
    }
    /// A full wall column at `x`, with an optional passable gap `(lo..=hi)` in y (a choke/door).
    pub fn wall_column(mut self, x: u8, gap: Option<(u8, u8)>) -> Self {
        for y in 0..=ROOM_MAX {
            if !gap.is_some_and(|(lo, hi)| y >= lo && y <= hi) {
                self.world.terrain.walls.insert((x.min(ROOM_MAX), y));
            }
        }
        self
    }
    pub fn wall_row(mut self, y: u8, gap: Option<(u8, u8)>) -> Self {
        for x in 0..=ROOM_MAX {
            if !gap.is_some_and(|(lo, hi)| x >= lo && x <= hi) {
                self.world.terrain.walls.insert((x, y.min(ROOM_MAX)));
            }
        }
        self
    }
    pub fn swamp_rect(mut self, x0: u8, y0: u8, x1: u8, y1: u8) -> Self {
        for y in y0.min(ROOM_MAX)..=y1.min(ROOM_MAX) {
            for x in x0.min(ROOM_MAX)..=x1.min(ROOM_MAX) {
                self.world.terrain.swamps.insert((x, y));
            }
        }
        self
    }

    // ── passive structures (attack/dismantle targets; ramparts shield + suppress attack-back) ──
    pub fn structure(&mut self, kind: StructureKind, owner: Option<PlayerId>, x: u8, y: u8, hits: u32, hits_max: u32) -> StructureId {
        let id = self.alloc_id();
        let pos = self.pos(x, y);
        self.world.structures.push(SimStructure { id, kind, owner, pos, hits, hits_max });
        id
    }
    pub fn cwall(mut self, x: u8, y: u8, hits: u32) -> Self {
        self.structure(StructureKind::Wall, None, x, y, hits, hits);
        self
    }
    pub fn rampart(mut self, owner: PlayerId, x: u8, y: u8, hits: u32) -> Self {
        self.structure(StructureKind::Rampart, Some(owner), x, y, hits, hits);
        self
    }
    pub fn spawn(mut self, owner: PlayerId, x: u8, y: u8) -> Self {
        self.structure(StructureKind::Spawn, Some(owner), x, y, SPAWN_HITS, SPAWN_HITS);
        self
    }
    /// A constructed-wall ring (the box of a base). `_owner` reserved for a future owned-perimeter.
    pub fn perimeter(mut self, _owner: PlayerId, x0: u8, y0: u8, x1: u8, y1: u8, wall_hits: u32) -> Self {
        for x in x0..=x1 {
            self = self.cwall(x, y0, wall_hits).cwall(x, y1, wall_hits);
        }
        for y in (y0 + 1)..y1 {
            self = self.cwall(x0, y, wall_hits).cwall(x1, y, wall_hits);
        }
        self
    }

    // ── towers (fire with falloff + are damage targets) ──
    pub fn tower(&mut self, owner: PlayerId, x: u8, y: u8, energy: u32) -> StructureId {
        let id = self.alloc_id();
        let pos = self.pos(x, y);
        self.world.towers.push(SimTower { id, owner, pos, energy, hits: TOWER_HITS, hits_max: TOWER_HITS });
        id
    }
    /// A tight tower cluster around `(cx,cy)` — up to 6, bounds-checked (off-room offsets skipped).
    pub fn tower_nest(mut self, owner: PlayerId, cx: u8, cy: u8, count: u8, energy: u32) -> Self {
        const OFFSETS: [(i32, i32); 6] = [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1), (1, 1)];
        for &(dx, dy) in OFFSETS.iter().take(count as usize) {
            let (x, y) = (cx as i32 + dx, cy as i32 + dy);
            if (0..=ROOM_MAX as i32).contains(&x) && (0..=ROOM_MAX as i32).contains(&y) {
                self.tower(owner, x as u8, y as u8, energy);
            }
        }
        self
    }

    pub fn safe_mode(mut self, owner: PlayerId) -> Self {
        self.world.safe_mode_owner = Some(owner);
        self
    }
    /// Escape hatch for scenarios needing direct world access (e.g. extra creeps).
    pub fn world_mut(&mut self) -> &mut CombatWorld {
        &mut self.world
    }
    pub fn build(self) -> CombatWorld {
        self.world
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }

    #[test]
    fn wall_column_leaves_a_gap() {
        let w = ScenarioBuilder::empty(room()).wall_column(25, Some((24, 26))).build();
        assert!(w.terrain.walls.contains(&(25, 10)), "wall above the gap");
        assert!(w.terrain.walls.contains(&(25, 40)), "wall below the gap");
        assert!(!w.terrain.walls.contains(&(25, 25)), "gap is passable");
        assert_eq!(w.terrain.walls.iter().filter(|(x, _)| *x == 25).count(), 47, "50 tiles minus a 3-wide gap");
    }

    #[test]
    fn rampart_bunker_has_walls_spawn_and_a_rampart() {
        let w = ScenarioBuilder::empty(room())
            .perimeter(1, 20, 20, 30, 30, 300_000)
            .spawn(1, 25, 25)
            .rampart(1, 25, 24, 1_000_000)
            .build();
        let spawns = w.structures.iter().filter(|s| s.kind == StructureKind::Spawn).count();
        let ramparts = w.structures.iter().filter(|s| s.kind == StructureKind::Rampart).count();
        let walls = w.structures.iter().filter(|s| s.kind == StructureKind::Wall).count();
        assert_eq!(spawns, 1);
        assert_eq!(ramparts, 1);
        assert!(walls > 30, "an 11x11 perimeter ring is ~40 walls (was {walls})");
        // ids are unique + above the creep-id band.
        let mut ids: Vec<_> = w.structures.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), w.structures.len(), "structure ids unique");
        assert!(ids[0] >= STRUCT_ID_BASE);
    }

    #[test]
    fn tower_nest_at_the_edge_does_not_panic_and_skips_off_room() {
        // Flush in the SE corner: only (49,49),(_,49 right is off),(49,_ down is off),(48,49),(49,48) are in-room.
        let w = ScenarioBuilder::empty(room()).tower_nest(1, 49, 49, 6, 100).build();
        assert!(!w.towers.is_empty(), "the center + in-room offsets placed");
        assert!(w.towers.iter().all(|t| t.pos.x().u8() <= 49 && t.pos.y().u8() <= 49));
        // (0,0)->(49,49) ok; (1,0)->off; (0,1)->off; (-1,0)->(48,49) ok; (0,-1)->(49,48) ok; (1,1)->off ⇒ 3 placed.
        assert_eq!(w.towers.len(), 3);
    }

    #[test]
    fn from_units_keeps_creeps_and_layers_a_tower() {
        let mut b = ScenarioBuilder::from_units(
            room(),
            0,
            &[Unit::new(vec![(screeps::Part::RangedAttack, 5)], vec![Position::new(RoomCoordinate::new(10).unwrap(), RoomCoordinate::new(25).unwrap(), room())])],
            1,
            &[],
        );
        let tid = b.tower(1, 25, 25, 200);
        let w = b.build();
        assert_eq!(w.creeps.len(), 1, "the seeded creep survives");
        assert_eq!(w.towers.len(), 1);
        assert_eq!(w.towers[0].id, tid);
    }
}
