//! The ibex smoke-gate markers — THE canonical ibex-specific strings
//! (P0.A14: policy lives here; the counting mechanism lives in
//! `screeps_server_kit::capture` and consumes these via [`capture_spec`]).
//!
//! ## Pinned against the bot crate's sources
//!
//! Console line format is `(<LEVEL>) <target>: <message>`
//! (screeps-ibex/src/logging.rs:32, the fern dispatch format). Markers:
//! - **panic**: the panic hook formats `PanicHookInfo` Display — the
//!   message contains `panicked at` — and logs it via `log::error!`
//!   (screeps-ibex/src/panic.rs `panic_hook`).
//! - **deserialization failure**: `Failed deserialization: <e>`
//!   (screeps-ibex/src/game_loop.rs:556) and `Failed to decode stats
//!   history` (screeps-ibex/src/stats_history.rs:200).
//!   Serialize-side errors (`Failed serialization:` game_loop.rs:427,
//!   `Encode failed:` game_loop.rs:432) are counted under
//!   `error_log_lines` but are NOT deser-gate markers.
//! - **live stats**: the bot writes its stats JSON
//!   (`{"shard":{"<shard>":{time,gcl,gpl,cpu:{bucket,limit,used},room,market}}}`)
//!   to segment 99 every tick (screeps-ibex/src/segments.rs
//!   `LIVE_STATS_SEGMENT`, written by statssystem.rs).
//! - **metrics block** (P1.A1/A2): the versioned ADR 0006 block on
//!   segment 57 (`METRICS_SEGMENT`, written by metrics.rs every tick;
//!   schema = the shared `screeps-ibex-metrics` crate). Captured into
//!   `metrics.jsonl` as the `metrics` field; [`parse_metrics_block`]
//!   interprets it.

use screeps_server_kit::capture::{CaptureSpec, MarkerSpec};

/// screeps-ibex/src/panic.rs `panic_hook` — std `PanicHookInfo` Display.
pub const PANIC_MARKER: &str = "panicked at";

/// screeps-ibex/src/game_loop.rs:556 and stats_history.rs:200.
pub const DESER_FAILURE_MARKERS: &[&str] =
    &["Failed deserialization:", "Failed to decode stats history"];

/// `(ERROR) <target>: ...` — the fern console format (logging.rs:32).
pub const ERROR_LOG_PREFIX: &str = "(ERROR)";

/// The bot's live-stats segment (segments.rs, `LIVE_STATS_SEGMENT`).
pub const STATS_SEGMENT: u8 = 99;

/// The bot's versioned metrics segment (segments.rs, `METRICS_SEGMENT`;
/// ADR 0006 / P1.A1).
pub const METRICS_SEGMENT: u8 = 57;

/// The ibex marker set, in the shape the kit's counters consume.
pub fn marker_spec() -> MarkerSpec {
    MarkerSpec {
        panic_markers: vec![PANIC_MARKER.to_string()],
        deser_markers: DESER_FAILURE_MARKERS
            .iter()
            .map(|m| m.to_string())
            .collect(),
        error_log_prefix: Some(ERROR_LOG_PREFIX.to_string()),
    }
}

/// Everything `screeps_server_kit::capture::run` needs to capture an
/// ibex run: the markers plus the live-stats and metrics segments.
pub fn capture_spec() -> CaptureSpec {
    CaptureSpec {
        markers: marker_spec(),
        stats_segment: Some(STATS_SEGMENT),
        metrics_segment: Some(METRICS_SEGMENT),
        // Fault injections are per-scenario (P1.A3/A5): the scenario
        // runner fills them from the fault schedule.
        console_injections: Vec::new(),
    }
}

/// Interpret a captured `metrics` value (the raw seg-57 JSON the kit
/// stores in `metrics.jsonl`) as the shared schema type. `None` for
/// absent/unparseable values — the schema crate's own tests pin the
/// tolerance rules; this is the reader-side seam.
pub fn parse_metrics_block(raw: &serde_json::Value) -> Option<screeps_ibex_metrics::MetricsBlock> {
    serde_json::from_value(raw.clone()).ok()
}

// ===================================================================
// tests — the marker pins (literal shapes from the bot's sources and
// the live server, 2026-06-09) + the gate semantics under ibex markers
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use screeps_server_kit::capture::{ConsoleCounters, ConsoleKind, ConsoleLine};

    /// The panic hook output (panic.rs — std PanicHookInfo Display)
    /// through the fern console format (logging.rs:32).
    #[test]
    fn panic_marker_matches_hook_output() {
        let spec = marker_spec();
        let line = "(ERROR) screeps_ibex::panic: panicked at 'index out of bounds', src/lib.rs:1:1";
        assert!(spec.is_panic_line(line));
        // Modern rustc Display variant (message on the next line).
        assert!(spec
            .is_panic_line("(ERROR) screeps_ibex::panic: panicked at src/missions/data.rs:66:13:"));
        assert!(!spec.is_panic_line("(INFO) screeps_ibex: spawning hauler"));
    }

    /// game_loop.rs:556 + stats_history.rs:200 are deser-gate markers;
    /// serialize-side failures are NOT (game_loop.rs:427/:432).
    #[test]
    fn deser_markers_match_pinned_sources() {
        let spec = marker_spec();
        assert!(spec.is_deser_failure_line(
            "(ERROR) screeps_ibex::game_loop: Failed deserialization: invalid value"
        ));
        assert!(spec.is_deser_failure_line(
            "(WARN) screeps_ibex::stats_history: Failed to decode stats history, using default: x"
        ));
        assert!(!spec
            .is_deser_failure_line("(ERROR) screeps_ibex::game_loop: Failed serialization: oops"));
        assert!(!spec.is_deser_failure_line("(ERROR) screeps_ibex::game_loop: Encode failed: oops"));
    }

    /// The exact marker strings are load-bearing — pin them so a
    /// rewording in the bot crate is caught here, not in a live run.
    #[test]
    fn marker_constants_are_pinned() {
        assert_eq!(PANIC_MARKER, "panicked at");
        assert_eq!(
            DESER_FAILURE_MARKERS,
            &["Failed deserialization:", "Failed to decode stats history"]
        );
        assert_eq!(ERROR_LOG_PREFIX, "(ERROR)");
        // segments.rs LIVE_STATS_SEGMENT — the seg-99 live-stats JSON.
        assert_eq!(STATS_SEGMENT, 99);
        // segments.rs METRICS_SEGMENT — the seg-57 metrics block.
        assert_eq!(METRICS_SEGMENT, 57);
        let spec = capture_spec();
        assert_eq!(spec.stats_segment, Some(99));
        assert_eq!(spec.metrics_segment, Some(57));
        assert_eq!(spec.markers.panic_markers, ["panicked at"]);
    }

    /// Reader-side seam: a writer-shaped block parses; junk does not.
    #[test]
    fn metrics_block_parses_from_captured_value() {
        let raw = serde_json::json!({
            "v": 1, "tick": 42, "vm_fresh": true,
            "cpu": {"used": 5.0, "limit": 100.0, "tick_limit": 500.0,
                     "bucket": 9000, "bucket_trend": -0.5},
            "gcl": {"level": 1, "progress": 0.0, "progress_total": 1000.0},
            "gpl": {"level": 0, "progress": 0.0, "progress_total": 1000.0},
            "faults": {"deser_failures": 0, "segment_chunks_used": 1,
                        "segment_chunk_budget": 5}
        });
        let block = parse_metrics_block(&raw).expect("writer-shaped block parses");
        assert_eq!(block.tick, 42);
        assert!(block.vm_fresh);
        assert_eq!(block.faults.segment_chunk_budget, 5);
        assert!(parse_metrics_block(&serde_json::json!("not a block")).is_none());
    }

    /// End-to-end through the kit's counters: ibex console lines are
    /// classified exactly as the old in-crate counters did.
    #[test]
    fn counters_classify_ibex_lines() {
        let spec = marker_spec();
        let mut c = ConsoleCounters::default();
        let mk = |kind, line: &str| ConsoleLine {
            ts_ms: 0,
            tick: None,
            kind,
            line: line.into(),
        };
        c.record(&mk(ConsoleKind::Log, "(INFO) a: fine"), &spec);
        c.record(
            &mk(ConsoleKind::Log, "(ERROR) a: bad but not gating"),
            &spec,
        );
        c.record(
            &mk(
                ConsoleKind::Log,
                "(ERROR) screeps_ibex::panic: panicked at 'x', s.rs:1:1",
            ),
            &spec,
        );
        c.record(
            &mk(
                ConsoleKind::Log,
                "(ERROR) screeps_ibex::game_loop: Failed deserialization: e",
            ),
            &spec,
        );
        c.record(&mk(ConsoleKind::Result, "undefined"), &spec);
        c.record(&mk(ConsoleKind::Error, "Error: thrown"), &spec);
        assert_eq!(c.lines, 6);
        assert_eq!(c.log_lines, 4);
        assert_eq!(c.result_lines, 1);
        assert_eq!(c.error_events, 1);
        assert_eq!(c.error_log_lines, 3); // all (ERROR)-prefixed log lines
        assert_eq!(c.panic_lines, 1);
        assert_eq!(c.deser_failure_lines, 1);
    }
}
