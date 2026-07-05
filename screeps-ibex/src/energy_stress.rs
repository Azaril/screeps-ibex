//! `repair_leak_e` telemetry (ADR 0040 §D6).
//!
//! The S1 repair-stress GATE was DELETED at ADR 0040 M5a (operator decision
//! 2026-07-05): the unified e/t sink market now prices repair natively (the
//! market's opportunity-floor admission owns repair admission live), so the
//! bot-side allowance adapter, the `features.energy.repair_stress_gate`
//! kill-switch, and the ad-hoc energy thresholds are gone (EP-2.6/2.10). The
//! S1 allowance KERNEL stays in `screeps_econ_decision::stress` — the economy
//! sim's S1 tournament arm still consumes it.
//!
//! The [`EnergyLeakStats`] counter is PERMANENT: it anchors the economy sim's
//! M1 repro gate and the M5 validation. **The counter is EXPECTED to RISE
//! versus the old S1-gated code** — that is correct re-pricing, not a
//! regression: the market re-prices repair against the opportunity floor
//! instead of hard-gating it (ADR 0040 M4 attribution; §D6). The bot-side
//! adapter ([`record_repair_leak`]) gathers plain room facts from the
//! [`EconomySnapshot`] and stays at the seam.

use crate::military::economy::EconomySnapshot;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

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

// The S1 allowance KERNEL's boundary/monotonicity tests live in
// `screeps_econ_decision::stress` (ADR 0040 M3; the sim's S1 arm still
// consumes it). The bot-side GATE was deleted at M5a; the tests below pin the
// surviving bot-side ADAPTER only — the `repair_leak_e` leak telemetry.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::military::economy::RoomEconomyData;

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

    // ── Leak telemetry (PERMANENT — the S1 gate that used to sit alongside it
    //    was deleted at M5a; this counter is EXPECTED to rise) ───────────────

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
    fn leak_records_under_any_deficit() {
        // The telemetry condition is ANY refill deficit (`spawn_energy <
        // spawn_energy_capacity`) — it measures the symptom wherever it occurs,
        // independent of any stress posture. Now that the S1 gate is gone (M5a),
        // this counter is EXPECTED to rise, because the market re-prices repair
        // against the opportunity floor rather than hard-gating it: a nonzero,
        // rising value is correct re-pricing, not a regression (§D6).
        let mut world = World::new();
        let room_name: RoomName = "E0N0".parse().expect("valid room name");

        // Tiny deficit: telemetry records.
        let tiny_deficit = RoomEconomyData {
            spawn_energy: 1290,
            spawn_energy_capacity: 1300,
            stored_energy: 0,
            ..RoomEconomyData::default()
        };
        let (economy, entity) = snapshot_with_room(&mut world, tiny_deficit);
        let mut stats = EnergyLeakStats::default();
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Road, 2);
        assert_eq!(stats.rooms.get(&room_name).map(|l| l.repair_roads), Some(2));

        // Full deficit even with a large stored buffer: telemetry still records
        // (it never sheds on a stored buffer — that WAS the old gate's condition,
        // which no longer applies).
        let buffered = RoomEconomyData {
            spawn_energy: 0,
            spawn_energy_capacity: 300,
            stored_energy: 50_000,
            ..RoomEconomyData::default()
        };
        let (economy, entity) = snapshot_with_room(&mut world, buffered);
        let mut stats = EnergyLeakStats::default();
        record_repair_leak(&mut stats, &economy, entity, room_name, StructureType::Container, 4);
        assert_eq!(stats.rooms.get(&room_name).map(|l| l.repair_containers), Some(4));
    }
}
