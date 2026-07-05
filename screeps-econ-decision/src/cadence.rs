//! The governor-gated re-match cadence policy (ADR 0007 Q5 item 2, delivered via ADR 0040 M3
//! reconciliation R1): whether an UNCOMMITTED hauler re-runs the matcher this tick, and how long
//! it backs off after a failed match. The governor *read* stays adapter-side (the bot maps its
//! `cpugovernor::Tier` snapshot to [`CpuPosture`]; the sim has no governor and always runs
//! [`CpuPosture::Normal`]).
//!
//! Policy (0007 item 2 text): a hauler re-runs the matcher only when it has no committed plan /
//! on completion (both enforced by the FSM shape — a committed `Pickup`/`Delivery` state never
//! re-selects); under **Conserve** the re-decision cadence stretches (a failed match backs off
//! longer before the next Idle attempt); under **Critical** re-selection is skipped entirely —
//! the hauler rides existing tickets (hauling itself is never shed; only the *re-decision* is).
//!
//! At `Normal` this is EXACTLY the pre-M3 behavior (attempt every Idle tick, `Wait(5)` after a
//! failed match) — the parity-preserving default. The Conserve/Critical arms only activate under
//! CPU pressure (ADR 0004's tiers), outside M3's A/A scope; their constants are initial values
//! pending the 0007 induced-CPU-pressure validation scenario.

/// The CPU-pressure posture as this kernel sees it (the adapter maps the bot's
/// `cpugovernor::Tier`; the mapping is 1:1 by name).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CpuPosture {
    Normal,
    Conserve,
    Critical,
}

/// The pre-M3 idle backoff after a failed match (`HaulState::wait(5)` — jobs/haul.rs).
pub const REMATCH_BACKOFF_NORMAL: u32 = 5;
/// Conserve: the stretched cadence (IBEX-030's "re-decide only every N ticks" under pressure).
pub const REMATCH_BACKOFF_CONSERVE: u32 = 10;
/// Critical: skip re-selection this window entirely; ride existing tickets.
pub const REMATCH_BACKOFF_CRITICAL: u32 = 10;

/// The re-match decision for an idle (uncommitted) hauler.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RematchDecision {
    /// Run the matcher this tick at all. `false` (Critical) ⇒ go straight to the backoff wait.
    pub attempt: bool,
    /// Idle backoff (ticks) before the next re-match attempt when no plan was found (or when
    /// `attempt` is false).
    pub backoff_ticks: u32,
}

/// The cadence policy. A creep with a committed plan never consults this (the FSM owns the
/// commitment invariant — 0007 item 3); this decides only the *idle re-selection* cadence.
pub fn rematch_policy(posture: CpuPosture) -> RematchDecision {
    match posture {
        CpuPosture::Normal => RematchDecision {
            attempt: true,
            backoff_ticks: REMATCH_BACKOFF_NORMAL,
        },
        CpuPosture::Conserve => RematchDecision {
            attempt: true,
            backoff_ticks: REMATCH_BACKOFF_CONSERVE,
        },
        CpuPosture::Critical => RematchDecision {
            attempt: false,
            backoff_ticks: REMATCH_BACKOFF_CRITICAL,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Normal is EXACTLY the pre-M3 behavior: attempt + Wait(5) — the parity pin.
    #[test]
    fn normal_posture_is_premove_behavior() {
        let d = rematch_policy(CpuPosture::Normal);
        assert!(d.attempt);
        assert_eq!(d.backoff_ticks, 5, "jobs/haul.rs HaulState::wait(5)");
    }

    /// Conserve still attempts (hauling reacts, just slower); Critical skips re-selection but
    /// never freezes a committed plan (attempt=false only gates IDLE re-selection).
    #[test]
    fn pressure_postures_stretch_or_skip() {
        let c = rematch_policy(CpuPosture::Conserve);
        assert!(c.attempt);
        assert!(c.backoff_ticks > REMATCH_BACKOFF_NORMAL);
        let k = rematch_policy(CpuPosture::Critical);
        assert!(!k.attempt);
        assert!(k.backoff_ticks >= REMATCH_BACKOFF_NORMAL);
    }
}
