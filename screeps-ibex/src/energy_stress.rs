//! S1 repair stress gate + `repair_leak_e` telemetry (ADR 0040 §D6).
//!
//! **INTERIM**: the allowance gate is stopgap scaffolding, superseded by the
//! unified e/t sink market at M5a (EP-2.10; removal point tied to the market's
//! default-on, EP-10.5). The interim constants below are declared interim —
//! the market replaces them, it does not calibrate them. The
//! [`EnergyLeakStats`] counter is PERMANENT: it anchors the economy sim's M1
//! repro gate and the M5 validation.
//!
//! The decision kernel ([`refill_deficit_q`] / [`repair_allowance`] /
//! [`effective_min_repair_priority`]) is functionally PURE — no `game::*` /
//! world reads, integer math only (the `room_economics.rs` precedent): it
//! takes plain room facts and returns an allowance. The bot-side adapters
//! ([`repair_allowance_for`], [`record_repair_leak`]) gather those facts from
//! the [`EconomySnapshot`] and stay at the seam.

use crate::features::Features;
use crate::jobs::utility::repair::RepairPriority;
use crate::military::economy::EconomySnapshot;
use screeps::*;
use specs::prelude::*;
use std::collections::HashMap;

/// Stored energy (storage + terminal + containers) at or above which repair is
/// unrestricted regardless of refill deficit — a room with a real buffer is
/// not energy-stressed. Precedent: `RENEW_MIN_ROOM_ENERGY` (spawnsystem.rs).
pub const REPAIR_UNRESTRICTED_STORED_ENERGY: u32 = 10_000;

/// Maximum per-mille refill deficit at which repair stays unrestricted
/// (100 = 10%): a near-full spawn/extension network is not stressed.
pub const REPAIR_UNRESTRICTED_MAX_DEFICIT_Q: u32 = 100;

/// What repair work a room's energy posture admits.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RepairAllowance {
    /// No stress: repair at the caller's own minimum priority.
    Unrestricted,
    /// Refill-deficient with no stored buffer: only Critical repair is
    /// admitted (the energy belongs to the refill chain).
    CriticalOnly,
}

/// Per-mille refill deficit. `capacity == 0` (no spawns visible) => 0 — there
/// is no refill demand to protect.
pub fn refill_deficit_q(energy_available: u32, energy_capacity: u32) -> u32 {
    if energy_capacity == 0 {
        return 0;
    }

    let available = energy_available.min(energy_capacity);
    let filled_q = ((available as u64 * 1000) / energy_capacity as u64) as u32;

    1000u32.saturating_sub(filled_q)
}

/// The room's repair allowance from its refill deficit + stored-energy buffer.
pub fn repair_allowance(deficit_q: u32, stored_energy: u32) -> RepairAllowance {
    if stored_energy >= REPAIR_UNRESTRICTED_STORED_ENERGY || deficit_q <= REPAIR_UNRESTRICTED_MAX_DEFICIT_Q {
        RepairAllowance::Unrestricted
    } else {
        RepairAllowance::CriticalOnly
    }
}

/// Raise the caller's minimum repair priority under [`RepairAllowance::CriticalOnly`].
/// `Unrestricted` leaves the minimum unchanged; `CriticalOnly` raises it to
/// `Critical` (the enum maximum, so this only ever tightens).
pub fn effective_min_repair_priority(min: Option<RepairPriority>, allowance: RepairAllowance) -> Option<RepairPriority> {
    match allowance {
        RepairAllowance::Unrestricted => min,
        RepairAllowance::CriticalOnly => Some(RepairPriority::Critical),
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::military::economy::RoomEconomyData;

    fn allowance_for(energy_available: u32, energy_capacity: u32, stored_energy: u32) -> RepairAllowance {
        repair_allowance(refill_deficit_q(energy_available, energy_capacity), stored_energy)
    }

    // ── Boundaries ──────────────────────────────────────────────────────────

    #[test]
    fn stored_exactly_at_threshold_is_unrestricted() {
        // 10k stored overrides any deficit (post-wipe deficit 1000 included).
        assert_eq!(repair_allowance(1000, REPAIR_UNRESTRICTED_STORED_ENERGY), RepairAllowance::Unrestricted);
    }

    #[test]
    fn below_stored_threshold_with_deficit_above_max_is_critical_only() {
        assert_eq!(
            repair_allowance(REPAIR_UNRESTRICTED_MAX_DEFICIT_Q + 1, REPAIR_UNRESTRICTED_STORED_ENERGY - 1),
            RepairAllowance::CriticalOnly
        );
    }

    #[test]
    fn deficit_exactly_at_max_is_unrestricted() {
        assert_eq!(repair_allowance(REPAIR_UNRESTRICTED_MAX_DEFICIT_Q, 0), RepairAllowance::Unrestricted);
    }

    #[test]
    fn zero_capacity_means_zero_deficit() {
        // No spawns visible => no refill demand to protect.
        assert_eq!(refill_deficit_q(0, 0), 0);
        assert_eq!(refill_deficit_q(500, 0), 0);
    }

    #[test]
    fn post_wipe_room_has_full_deficit() {
        assert_eq!(refill_deficit_q(0, 300), 1000);
    }

    #[test]
    fn full_room_has_zero_deficit() {
        assert_eq!(refill_deficit_q(300, 300), 0);
        assert_eq!(refill_deficit_q(12_900, 12_900), 0);
        // Clamped: available above capacity still reads as full.
        assert_eq!(refill_deficit_q(400, 300), 0);
    }

    #[test]
    fn deficit_is_per_mille_of_capacity() {
        // 90% full => 100 per-mille deficit (the unrestricted boundary).
        assert_eq!(refill_deficit_q(900, 1000), 100);
        assert_eq!(refill_deficit_q(899, 1000), 101);
        // Large capacity: u64 intermediate avoids overflow (2e9 * 1000 > u32::MAX).
        assert_eq!(refill_deficit_q(2_000_000_000, 4_000_000_000), 500);
    }

    // ── Monotonicity ────────────────────────────────────────────────────────

    #[test]
    fn more_stored_energy_never_tightens_allowance() {
        for deficit_q in (0..=1000).step_by(50) {
            let mut prev = repair_allowance(deficit_q, 0);
            for stored in (0..=20_000).step_by(500) {
                let cur = repair_allowance(deficit_q, stored);
                // Unrestricted must never regress to CriticalOnly as stored grows.
                assert!(
                    !(prev == RepairAllowance::Unrestricted && cur == RepairAllowance::CriticalOnly),
                    "tightened at deficit_q={deficit_q} stored={stored}"
                );
                prev = cur;
            }
        }
    }

    #[test]
    fn larger_deficit_never_loosens_allowance() {
        for stored in (0..=20_000).step_by(500) {
            let mut prev = repair_allowance(0, stored);
            for deficit_q in 0..=1000 {
                let cur = repair_allowance(deficit_q, stored);
                // CriticalOnly must never relax to Unrestricted as the deficit grows.
                assert!(
                    !(prev == RepairAllowance::CriticalOnly && cur == RepairAllowance::Unrestricted),
                    "loosened at deficit_q={deficit_q} stored={stored}"
                );
                prev = cur;
            }
        }
    }

    #[test]
    fn deficit_is_monotone_in_available_energy() {
        let capacity = 1300;
        let mut prev = refill_deficit_q(0, capacity);
        for available in 1..=capacity {
            let cur = refill_deficit_q(available, capacity);
            assert!(cur <= prev, "deficit grew at available={available}");
            prev = cur;
        }
    }

    // ── effective_min table ─────────────────────────────────────────────────

    #[test]
    fn effective_min_table() {
        let minimums = [
            None,
            Some(RepairPriority::VeryLow),
            Some(RepairPriority::Low),
            Some(RepairPriority::Medium),
            Some(RepairPriority::High),
            Some(RepairPriority::Critical),
        ];

        for min in minimums {
            // Unrestricted: caller's minimum passes through unchanged.
            assert_eq!(effective_min_repair_priority(min, RepairAllowance::Unrestricted), min);
            // CriticalOnly: always raised to Critical (the enum max).
            assert_eq!(
                effective_min_repair_priority(min, RepairAllowance::CriticalOnly),
                Some(RepairPriority::Critical)
            );
        }
    }

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
