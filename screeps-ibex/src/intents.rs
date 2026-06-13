//! Intent recorder + guarded combat sink (P1.C3 / ADR 0015 F4 +
//! P1.C4 / IBEX-029, ADR 0003 §A2).
//!
//! ## The recorder (the differ's instrument)
//!
//! Every guarded intent is counted per category AND folded into an
//! ORDER-SENSITIVE digest (chained FNV-1a over `(category, creep,
//! target-pos)` tuples). Two dispatch configurations that emit the
//! same intent stream produce the same digest — the shadow-dispatch
//! parity check for the P1.C5 scheduler seam diffs exactly this,
//! without storing the stream. Counts + digest are emitted in the
//! seg-57 block each tick.
//!
//! [`IntentRecorder`] is a specs **Resource** riding
//! `JobExecutionRuntimeData` (statics-review M2 — the statics are
//! gone). That is what makes shadow-dispatch diffing literal: two
//! recorder instances, `a.snapshot() == b.snapshot()` — no
//! snapshot/reset choreography against a process global.
//!
//! ## The guarded sink (combat's missing pipeline discipline)
//!
//! The job pipeline gates intents through
//! `SimultaneousActionFlags::consume` (one intent per engine pipeline
//! per tick — docs.screeps.com/simultaneous-actions.html), but
//! squad_combat issued its ~23 attack/heal sites bare (IBEX-029): the
//! per-creep `UNSET` flags were created and never consumed, so
//! conflicting same-pipeline intents were all SENT (0.2 CPU each) and
//! the ENGINE arbitrated which one executed. The sink helpers consume
//! the correct pipeline flag first: conflicts are suppressed
//! client-side (first caller wins — OUR priority order, deliberate and
//! diffable, instead of the engine's), and every issued intent is
//! recorded.

use crate::jobs::actions::SimultaneousActionFlags;
use screeps::prelude::*;
use screeps::{Attackable, Creep, Healable, Position};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentCategory {
    Attack = 0,
    RangedAttack = 1,
    RangedMassAttack = 2,
    Heal = 3,
    RangedHeal = 4,
}

const CATEGORY_COUNT: usize = 5;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv_fold(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The tick's intent stream view: per-category counts + the chained
/// order-sensitive FNV-1a digest. specs Resource; reset at tick start
/// (`metrics::tick_start`), written through the sink helpers, read by
/// the metrics emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRecorder {
    counts: [u32; CATEGORY_COUNT],
    digest: u64,
}

impl Default for IntentRecorder {
    fn default() -> Self {
        IntentRecorder {
            counts: [0; CATEGORY_COUNT],
            digest: FNV_OFFSET,
        }
    }
}

impl IntentRecorder {
    /// Tick-start reset.
    pub fn reset(&mut self) {
        *self = IntentRecorder::default();
    }

    /// Fold one issued intent into the tick's counts + digest.
    pub fn record(&mut self, category: IntentCategory, creep_name: &str, target_pos: Option<Position>) {
        self.counts[category as usize] += 1;
        let mut hash = self.digest;
        hash = fnv_fold(hash, &[category as u8]);
        hash = fnv_fold(hash, creep_name.as_bytes());
        if let Some(pos) = target_pos {
            hash = fnv_fold(hash, &pos.packed_repr().to_le_bytes());
        }
        self.digest = hash;
    }

    /// The tick's recorded view, for the seg-57 block.
    pub fn snapshot(&self) -> ([u32; CATEGORY_COUNT], u64) {
        (self.counts, self.digest)
    }
}

// ── The guarded sink ─────────────────────────────────────────────────

/// Pipeline A melee attack. Consumes the A flag; suppressed (false)
/// when another A-pipeline intent already fired this tick. The
/// explicit `target_pos` keeps the helpers `?Sized`-friendly (call
/// sites pass `target.pos()`; `as_attackable()` trait objects work).
pub fn attack<T>(
    creep: &Creep,
    flags: &mut SimultaneousActionFlags,
    recorder: &mut IntentRecorder,
    target: &T,
    target_pos: Position,
) -> bool
where
    T: ?Sized + Attackable,
{
    if !flags.consume(SimultaneousActionFlags::ATTACK) {
        return false;
    }
    recorder.record(IntentCategory::Attack, &creep.name(), Some(target_pos));
    let _ = creep.attack(target);
    true
}

/// Pipeline B ranged attack.
pub fn ranged_attack<T>(
    creep: &Creep,
    flags: &mut SimultaneousActionFlags,
    recorder: &mut IntentRecorder,
    target: &T,
    target_pos: Position,
) -> bool
where
    T: ?Sized + Attackable,
{
    if !flags.consume(SimultaneousActionFlags::RANGED_ATTACK) {
        return false;
    }
    recorder.record(IntentCategory::RangedAttack, &creep.name(), Some(target_pos));
    let _ = creep.ranged_attack(target);
    true
}

/// Pipeline B ranged mass attack.
pub fn ranged_mass_attack(creep: &Creep, flags: &mut SimultaneousActionFlags, recorder: &mut IntentRecorder) -> bool {
    if !flags.consume(SimultaneousActionFlags::RANGED_MASS_ATTACK) {
        return false;
    }
    recorder.record(IntentCategory::RangedMassAttack, &creep.name(), None);
    let _ = creep.ranged_mass_attack();
    true
}

/// Pipeline C heal.
pub fn heal<T>(creep: &Creep, flags: &mut SimultaneousActionFlags, recorder: &mut IntentRecorder, target: &T, target_pos: Position) -> bool
where
    T: ?Sized + Healable,
{
    if !flags.consume(SimultaneousActionFlags::HEAL) {
        return false;
    }
    recorder.record(IntentCategory::Heal, &creep.name(), Some(target_pos));
    let _ = creep.heal(target);
    true
}

/// Pipeline B ranged heal (NOT pipeline C — the engine puts rangedHeal
/// in the ranged pipeline; see actions.rs).
pub fn ranged_heal<T>(
    creep: &Creep,
    flags: &mut SimultaneousActionFlags,
    recorder: &mut IntentRecorder,
    target: &T,
    target_pos: Position,
) -> bool
where
    T: ?Sized + Healable,
{
    if !flags.consume(SimultaneousActionFlags::RANGED_HEAL) {
        return false;
    }
    recorder.record(IntentCategory::RangedHeal, &creep.name(), Some(target_pos));
    let _ = creep.ranged_heal(target);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest is order-sensitive and deterministic — the property
    /// the P1.C5 shadow-dispatch parity check rests on. Per-instance
    /// now: the two streams below are SEPARATE recorders diffed
    /// directly, exactly the shadow-dispatch shape (M2's payoff — the
    /// old global digest forced reset choreography between runs).
    #[test]
    fn digest_is_order_sensitive_and_deterministic() {
        let run = |order: &[(IntentCategory, &str)]| {
            let mut recorder = IntentRecorder::default();
            for (cat, name) in order {
                recorder.record(*cat, name, None);
            }
            recorder.snapshot()
        };
        let a1 = run(&[(IntentCategory::Attack, "c1"), (IntentCategory::Heal, "c2")]);
        let a2 = run(&[(IntentCategory::Attack, "c1"), (IntentCategory::Heal, "c2")]);
        let b = run(&[(IntentCategory::Heal, "c2"), (IntentCategory::Attack, "c1")]);
        assert_eq!(a1, a2, "same stream, same digest");
        assert_eq!(a1.0, b.0, "same counts either order");
        assert_ne!(a1.1, b.1, "different order, different digest");
        let mut recorder = IntentRecorder::default();
        recorder.record(IntentCategory::Attack, "c1", None);
        recorder.reset();
        assert_eq!(recorder.snapshot(), IntentRecorder::default().snapshot());
    }

    /// The sink's pipeline discipline mirrors the engine's: A, B, C are
    /// independent; intents within a pipeline are mutually exclusive.
    #[test]
    fn pipeline_flags_suppress_same_pipeline_conflicts() {
        use crate::jobs::actions::SimultaneousActionFlags as F;
        let mut flags = F::UNSET;
        assert!(flags.consume(F::RANGED_ATTACK));
        // rangedHeal shares pipeline B with rangedAttack — suppressed.
        assert!(!flags.consume(F::RANGED_HEAL));
        // heal is pipeline C — independent.
        assert!(flags.consume(F::HEAL));
        // attack is pipeline A — independent.
        assert!(flags.consume(F::ATTACK));
        // …but a second A intent is suppressed.
        assert!(!flags.consume(F::HARVEST));
    }
}
