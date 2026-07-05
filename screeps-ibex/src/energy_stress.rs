//! S1 repair stress gate + `repair_leak_e` telemetry (ADR 0040 §D6).
//!
//! **INTERIM**: the allowance gate is stopgap scaffolding, superseded by the
//! unified e/t sink market at M5a (EP-2.10; removal point tied to the market's
//! default-on, EP-10.5). The [`EnergyLeakStats`] counter is PERMANENT: it
//! anchors the economy sim's M1 repro gate and the M5 validation.
//!
//! The decision kernel ([`refill_deficit_q`] / [`repair_allowance`] /
//! [`effective_min_repair_priority`] / [`RepairAllowance`]) lives in
//! `screeps_econ_decision::stress` since ADR 0040 M3 (K3) — consumed by this
//! adapter AND by the economy sim (`screeps-econ-eval`), one implementation
//! (EP-2.6). Re-exported here so every bot call site keeps its
//! `crate::energy_stress::*` path. The bot-side adapters
//! ([`repair_allowance_for`], [`record_repair_leak`]) gather plain room facts
//! from the [`EconomySnapshot`] and stay at the seam.

use crate::features::Features;
use crate::military::economy::EconomySnapshot;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

#[allow(unused_imports)] // the constants are re-exported API (tests + future consumers)
pub use screeps_econ_decision::stress::{
    effective_min_repair_priority, refill_deficit_q, repair_allowance, RepairAllowance, REPAIR_UNRESTRICTED_MAX_DEFICIT_Q,
    REPAIR_UNRESTRICTED_STORED_ENERGY,
};

/// Allowance for a posture room (the creep's HOME/delivery room — ADR 0040
/// §D8 #3; callers with no home concept fall back to the creep's current
/// room). Flag off, or room missing from the snapshot (not owned / not
/// visible) => `Unrestricted` — fail-open to current behavior.
pub fn repair_allowance_for(economy: &EconomySnapshot, features: &Features, posture_room: Option<Entity>) -> RepairAllowance {
    if !features.energy.repair_stress_gate {
        return RepairAllowance::Unrestricted;
    }

    posture_room
        .and_then(|room| economy.room(&room))
        .map(|room| room.repair_allowance())
        .unwrap_or(RepairAllowance::Unrestricted)
}

// ---------------------------------------------------------------------------
// repair_leak_e telemetry (PERMANENT — anchors the economy sim's M1 repro gate)
// ---------------------------------------------------------------------------

/// Energy spent on repair intents this tick while the spending creep's/tower's
/// posture room had a refill deficit, by structure class. Keyed by the POSTURE
/// room's name (always an owned room, so the metrics export finds it).
///
/// Ephemeral resource — cleared each tick by [`EnergyLeakClearSystem`],
/// exported per-room via `metrics.rs`. Records regardless of the
/// `repair_stress_gate` flag (telemetry never sheds — EP-4.3).
#[derive(Default)]
pub struct EnergyLeakStats {
    pub rooms: HashMap<RoomName, RoomLeak>,
}

/// Per-room repair-leak counters (energy units).
#[derive(Debug, Default, Clone, Copy)]
pub struct RoomLeak {
    pub repair_roads: u32,
    pub repair_containers: u32,
    pub repair_other: u32,
}

impl EnergyLeakStats {
    /// Clear all counters (called at the start of each tick).
    pub fn clear(&mut self) {
        self.rooms.clear();
    }
}

/// Record repair energy against the posture room IF that room currently has a
/// refill deficit (`spawn_energy < spawn_energy_capacity` per the snapshot).
/// A room missing from the snapshot has no known deficit — nothing recorded.
pub fn record_repair_leak(
    stats: &mut EnergyLeakStats,
    economy: &EconomySnapshot,
    posture_room: Entity,
    posture_room_name: RoomName,
    structure_type: StructureType,
    energy: u32,
) {
    let has_deficit = economy
        .room(&posture_room)
        .map(|r| r.spawn_energy < r.spawn_energy_capacity)
        .unwrap_or(false);

    if !has_deficit || energy == 0 {
        return;
    }

    let leak = stats.rooms.entry(posture_room_name).or_default();

    match structure_type {
        StructureType::Road => leak.repair_roads += energy,
        StructureType::Container => leak.repair_containers += energy,
        _ => leak.repair_other += energy,
    }
}

/// System that clears the repair-leak counters at the start of each tick.
#[derive(Default)]
pub struct EnergyLeakClearSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for EnergyLeakClearSystem {
    type SystemData = Write<'a, EnergyLeakStats>;

    fn run(&mut self, mut stats: Self::SystemData) {
        stats.clear();
    }
}

// The kernel's boundary/monotonicity tests MOVED with it to
// `screeps_econ_decision::stress` (ADR 0040 M3). The tests below pin the
// bot-side ADAPTERS only (flag/snapshot behavior + leak telemetry).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::military::economy::RoomEconomyData;

    // ── Adapter: flag / snapshot behavior ───────────────────────────────────

    fn snapshot_with_room(world: &mut World, room: RoomEconomyData) -> (EconomySnapshot, Entity) {
        let entity = world.create_entity().build();
        let mut economy = EconomySnapshot::default();
        economy.rooms.insert(entity, room);
        (economy, entity)
    }

    fn stressed_room() -> RoomEconomyData {
        RoomEconomyData {
            stored_energy: 0,
            spawn_energy: 0,
            spawn_energy_capacity: 300,
            ..RoomEconomyData::default()
        }
    }

    #[test]
    fn flag_off_is_always_unrestricted() {
        let mut world = World::new();
        let (economy, entity) = snapshot_with_room(&mut world, stressed_room());

        let features = Features {
            energy: crate::features::EnergyFeatures { repair_stress_gate: false },
            ..Features::default()
        };

        assert_eq!(
            repair_allowance_for(&economy, &features, Some(entity)),
            RepairAllowance::Unrestricted
        );
    }

    #[test]
    fn flag_on_gates_a_stressed_room_and_fails_open_otherwise() {
        let mut world = World::new();
        let (economy, entity) = snapshot_with_room(&mut world, stressed_room());
        let missing = world.create_entity().build();

        let features = Features::default();
        assert!(features.energy.repair_stress_gate, "gate defaults on");

        assert_eq!(
            repair_allowance_for(&economy, &features, Some(entity)),
            RepairAllowance::CriticalOnly
        );
        // Not in the snapshot (not owned/visible) => fail-open.
        assert_eq!(
            repair_allowance_for(&economy, &features, Some(missing)),
            RepairAllowance::Unrestricted
        );
        // No posture room at all => fail-open.
        assert_eq!(repair_allowance_for(&economy, &features, None), RepairAllowance::Unrestricted);
    }

    // ── Leak telemetry ──────────────────────────────────────────────────────

    #[test]
    fn leak_records_by_class_only_under_deficit() {
        let mut world = World::new();
        let (economy, entity) = snapshot_with_room(&mut world, stressed_room());
        let room_name: RoomName = "E0N0".parse().expect("valid room name");

        let mut stats = EnergyLeakStats::default();
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Road, 3);
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Road, 2);
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Container, 4);
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Rampart, 5);
        // Zero-energy repairs are not counted.
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Road, 0);

        let leak = stats.rooms.get(&room_name).copied().expect("room recorded");
        assert_eq!(leak.repair_roads, 5);
        assert_eq!(leak.repair_containers, 4);
        assert_eq!(leak.repair_other, 5);

        // A room with no refill deficit records nothing.
        let full = RoomEconomyData {
            spawn_energy: 300,
            spawn_energy_capacity: 300,
            ..RoomEconomyData::default()
        };
        let (full_economy, full_entity) = snapshot_with_room(&mut world, full);
        let mut full_stats = EnergyLeakStats::default();
        record_repair_leak(&mut full_stats, &full_economy, full_entity, room_name, StructureType::Road, 3);
        assert!(full_stats.rooms.is_empty());

        // A room missing from the snapshot has no known deficit — nothing recorded.
        let missing = world.create_entity().build();
        let mut missing_stats = EnergyLeakStats::default();
        record_repair_leak(&mut missing_stats, &full_economy, missing, room_name, StructureType::Road, 3);
        assert!(missing_stats.rooms.is_empty());
    }

    #[test]
    fn leak_records_under_any_deficit_independent_of_gate() {
        // The telemetry condition (ANY refill deficit) is deliberately different
        // from the gate condition (deficit > 10% AND stored < 10k) — the counter
        // measures the symptom wherever it occurs; the gate acts only under real
        // stress. A future "simplification" that reuses the gate's allowance as
        // the telemetry condition must fail here.
        let mut world = World::new();
        let room_name: RoomName = "E0N0".parse().expect("valid room name");

        // Tiny deficit (deficit_q = 8 <= 100): gate Unrestricted, telemetry records.
        let tiny_deficit = RoomEconomyData {
            spawn_energy: 1290,
            spawn_energy_capacity: 1300,
            stored_energy: 0,
            ..RoomEconomyData::default()
        };
        assert_eq!(tiny_deficit.repair_allowance(), RepairAllowance::Unrestricted);
        let (economy, entity) = snapshot_with_room(&mut world, tiny_deficit);
        let mut stats = EnergyLeakStats::default();
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Road, 2);
        assert_eq!(stats.rooms.get(&room_name).map(|l| l.repair_roads), Some(2));

        // Full deficit but a >=10k stored buffer: gate Unrestricted, telemetry records.
        let buffered = RoomEconomyData {
            spawn_energy: 0,
            spawn_energy_capacity: 300,
            stored_energy: REPAIR_UNRESTRICTED_STORED_ENERGY,
            ..RoomEconomyData::default()
        };
        assert_eq!(buffered.repair_allowance(), RepairAllowance::Unrestricted);
        let (economy, entity) = snapshot_with_room(&mut world, buffered);
        let mut stats = EnergyLeakStats::default();
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Container, 4);
        assert_eq!(stats.rooms.get(&room_name).map(|l| l.repair_containers), Some(4));
    }
}
