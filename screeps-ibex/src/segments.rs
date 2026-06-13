//! RawMemory segment registry — the single code-form of ADR 0002's
//! "Segment allocation registry" table (`docs/design/0002-serialization.md`).
//!
//! EVERY segment id the bot touches is a named constant here, and every
//! subsystem imports its id(s) from this module rather than defining them
//! locally. The compile-time uniqueness check at the bottom runs over the
//! whole table, so a colliding allocation fails to compile instead of
//! silently wiping another subsystem's segment (the seg-55 cost-matrix wipe,
//! IBEX-013, is exactly how an unregistered allocation fails at runtime).
//!
//! Registry (mirrors the ADR 0002 table; reserved ids are design-allocated
//! but not yet implemented in code):
//!
//! | Seg   | Owner / contents                                            |
//! |-------|-------------------------------------------------------------|
//! | 50–53 | ECS component payload (`COMPONENT_SEGMENTS`)                |
//! | 54    | *freed 2026-06-12* (former 5th component chunk — shrunk to fund the always-active market segment; stale chunk data may linger server-side until reused) |
//! | 55    | cost-matrix cache (`COST_MATRIX_SEGMENT`)                   |
//! | 56    | stats history (`STATS_HISTORY_SEGMENT`)                     |
//! | 57    | metrics block (`METRICS_SEGMENT`, ADR 0006 / P1.A1)         |
//! | 58    | market memory: history cache + exposure ledger (`MARKET_SEGMENT`, ADR 0012; always-active) |
//! | 60    | room-planner resume state (`PLANNER_MEMORY_SEGMENT`)        |
//! | 61    | *reserved:* RoomGraph + inter-room road sets (ADR 0009)     |
//! | 99    | live stats, legacy JSON (`LIVE_STATS_SEGMENT`)              |
//!
//! Adding a segment? Add the constant here, add it to `OTHER_SEGMENT_IDS`
//! below, and add the matching row to ADR 0002's registry table.

/// Segment ids used for ECS component serialization (world state).
///
/// `serialize_world` (game_loop) chunks the encoded world across these ids
/// and blanks every id in the list not consumed by a chunk — which is why
/// this list must never contain a segment owned by another subsystem
/// (IBEX-013). The chunk-count watermark log in `serialize_world` monitors
/// how close the payload gets to this budget.
///
/// Shrunk 5 → 4 (seg 54 freed) on 2026-06-12 to fund the always-active
/// market segment within the engine's 10-touch budget — gated on watermark
/// evidence (BASELINE-2 scale used 1 of 5 chunks; the watermark warns at
/// budget − 1). If a future empire approaches 4 chunks, reclaim a slot via
/// the periodic-segment rotation planned for lazily-used ids (60 et al.)
/// rather than re-growing this list.
pub const COMPONENT_SEGMENTS: &[u32] = &[50, 51, 52, 53];

/// Cost-matrix cache (`pathing::costmatrixsystem`). Formerly collided with
/// the component range (IBEX-013); the registry check below keeps it
/// disjoint at compile time.
pub const COST_MATRIX_SEGMENT: u32 = 55;

/// Stats history ring buffers (`stats_history`).
pub const STATS_HISTORY_SEGMENT: u32 = 56;

/// Always-on versioned metrics block (`metrics`; ADR 0006/P1.A1 — the
/// schema lives in the `screeps-ibex-metrics` crate, shared with the
/// eval harness reader).
pub const METRICS_SEGMENT: u32 = 57;

/// Market memory: per-resource history-day cache + exposure ledger
/// (`transfer::fairvalue` data, `transfer::ordersystem` glue — the interim
/// form of ADR 0012 M3's risk-ledger segment). Deliberately self-contained:
/// it carries its own version field and decodes independently of
/// `WORLD_FORMAT_VERSION`, so reshaping the component segments can never
/// cost the market state.
///
/// ALWAYS-ACTIVE (registered `SegmentRequirement`, loaded via `on_load`) —
/// risk data wants zero save gaps, so its slot is funded by the component
/// shrink above rather than the queued-write displacement dance. The
/// arbiter's `queue_write` path remains the mechanism for future segments
/// that stay outside the active set.
pub const MARKET_SEGMENT: u32 = 58;

/// Room-planner resume state (`room::roomplansystem`).
pub const PLANNER_MEMORY_SEGMENT: u32 = 60;

/// Live stats consumed by external tooling (`statssystem`; legacy JSON).
pub const LIVE_STATS_SEGMENT: u32 = 99;

/// Every registered non-component id, including the reserved-but-unbuilt
/// ones, so the uniqueness check covers the WHOLE ADR 0002 table. When a
/// reserved id gets implemented, replace the literal with its new constant.
const OTHER_SEGMENT_IDS: &[u32] = &[
    54, // freed: former 5th component chunk (shrunk 2026-06-12); reusable by a future allocation
    COST_MATRIX_SEGMENT,
    STATS_HISTORY_SEGMENT,
    METRICS_SEGMENT,
    MARKET_SEGMENT,
    PLANNER_MEMORY_SEGMENT,
    61, // reserved: RoomGraph + inter-room road sets (ADR 0009)
    LIVE_STATS_SEGMENT,
];

const SEGMENT_TABLE_LEN: usize = COMPONENT_SEGMENTS.len() + OTHER_SEGMENT_IDS.len();

/// The full registry as one table: component ids followed by everything else.
const fn segment_table() -> [u32; SEGMENT_TABLE_LEN] {
    let mut table = [0u32; SEGMENT_TABLE_LEN];

    let mut i = 0;
    while i < COMPONENT_SEGMENTS.len() {
        table[i] = COMPONENT_SEGMENTS[i];
        i += 1;
    }

    let mut j = 0;
    while j < OTHER_SEGMENT_IDS.len() {
        table[i + j] = OTHER_SEGMENT_IDS[j];
        j += 1;
    }

    table
}

const fn all_unique(ids: &[u32]) -> bool {
    let mut i = 0;
    while i < ids.len() {
        let mut j = i + 1;
        while j < ids.len() {
            if ids[i] == ids[j] {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

// Compile-time uniqueness check over the whole registry. This assert IS the
// regression test for segment-map disjointness (Phase 0 D1 / IBEX-013); it
// has no runtime twin by design.
const _: () = {
    let table = segment_table();
    assert!(
        all_unique(&table),
        "RawMemory segment registry contains a duplicate id (see ADR 0002's segment-allocation registry)"
    );
};
