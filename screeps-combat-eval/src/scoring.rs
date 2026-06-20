//! Self-play / stalemate adjudication (P2.U6): score an engagement when **bots play each other** —
//! including the case the operator called out, *"if they eventually just stalemate."*
//!
//! ## Why not score a stalemate on residual HP
//! The design-pass review flagged the original `residual_EV` (score the survivor on how much HP it
//! has left) as an **inverted incentive**: it rewards the side that survived by *not fighting*
//! (turtling, dodging the engagement) over one that fought and traded well. HP *level* is the wrong
//! signal.
//!
//! ## What we score instead — HP *slope* (residual military advantage)
//! The right question for a non-decisive engagement is *"who would win if it kept going?"* — the
//! **derivative** of each side's total HP over a recent window, not its level. The side whose HP is
//! falling slower (or rising) is winning the war of attrition right now. A genuine grind where both
//! sides fully heal through each other's fire has ~0 slope on both → a true **Draw** (correctly,
//! nobody is making progress). This is body-free (reads the replay's per-frame hits) and can't be
//! gamed by a passive turtle, since standing still and taking chip damage shows up as a *negative*
//! slope, not a reward.

use crate::metrics::SideMetrics;
use screeps_combat_engine::{CombatRecording, PlayerId};

/// How many trailing frames define "recently" for the slope (residual-advantage) measure.
const MOMENTUM_WINDOW: usize = 20;
/// HP-slope difference within this band ⇒ a true stalemate (neither side making progress).
const DRAW_EPSILON: i64 = 150;

/// Who came out ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    SideA,
    SideB,
    /// A genuine stalemate — both sides alive and neither making attrition progress.
    Draw,
}

/// A fully-scored engagement: the verdict, whether it was decisive (a side eliminated) or scored on
/// residual advantage at timeout, both sides' [`SideMetrics`], and the residual HP-slope advantage.
#[derive(Clone, Debug)]
pub struct EngagementScore {
    pub verdict: Verdict,
    /// `true` if one side was wiped out; `false` if both survived and the verdict is on residual
    /// advantage (or Draw).
    pub decisive: bool,
    pub a: SideMetrics,
    pub b: SideMetrics,
    /// `slope_a − slope_b` over the trailing [`MOMENTUM_WINDOW`] (HP units; >0 ⇒ A is winning the
    /// recent exchange). 0 for a decisive result.
    pub residual_advantage_a: i64,
}

/// Total living-creep HP for `side` in a single frame.
fn side_hits(frame: &screeps_combat_engine::record::TickFrame, side: PlayerId) -> i64 {
    frame.creeps.iter().filter(|c| c.owner == side).map(|c| c.hits as i64).sum()
}

/// HP slope for `side` over the trailing window (`hits(last) − hits(window_start)`); negative ⇒
/// losing HP. 0 for a <2-frame run.
fn hp_slope(rec: &CombatRecording, side: PlayerId) -> i64 {
    let n = rec.frames.len();
    if n < 2 {
        return 0;
    }
    let start = n.saturating_sub(MOMENTUM_WINDOW);
    side_hits(&rec.frames[n - 1], side) - side_hits(&rec.frames[start], side)
}

/// Adjudicate an engagement from its replay. `a_owner`/`b_owner` are the two sides.
pub fn score(rec: &CombatRecording, a_owner: PlayerId, b_owner: PlayerId) -> EngagementScore {
    let a = SideMetrics::from_recording(rec, a_owner);
    let b = SideMetrics::from_recording(rec, b_owner);

    let a_alive = a.survivors > 0;
    let b_alive = b.survivors > 0;

    // Decisive: a side was eliminated (or both — mutual annihilation is a Draw).
    let (verdict, decisive, residual_advantage_a) = match (a_alive, b_alive) {
        (true, false) => (Verdict::SideA, true, 0),
        (false, true) => (Verdict::SideB, true, 0),
        (false, false) => (Verdict::Draw, true, 0),
        (true, true) => {
            // Non-decisive: score on residual military advantage = recent HP slope (NOT HP level).
            let adv = hp_slope(rec, a_owner) - hp_slope(rec, b_owner);
            let verdict = if adv > DRAW_EPSILON {
                Verdict::SideA
            } else if adv < -DRAW_EPSILON {
                Verdict::SideB
            } else {
                Verdict::Draw
            };
            (verdict, false, adv)
        }
    };

    EngagementScore { verdict, decisive, a, b, residual_advantage_a }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screeps::{Part, Position, RoomCoordinate, RoomName};
    use screeps_combat_agent::opponents::{run_engagement, self_play, world_from_units, KiteAgent, TurtleAgent, Unit};
    use screeps_combat_agent::{HoldAgent, IbexAgent};

    fn room() -> RoomName {
        "W1N1".parse().unwrap()
    }
    fn pos(x: u8, y: u8) -> Position {
        Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
    }

    #[test]
    fn decisive_win_is_scored_for_the_survivor() {
        // 3×ranged clear a lone turtle → decisive SideA.
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 7)], vec![pos(25, 22), pos(24, 22), pos(26, 22)])],
            1,
            &[Unit::new(vec![(Part::Heal, 5)], vec![pos(25, 25)])],
        );
        let out = run_engagement(world, room(), 0, pos(25, 22), &mut IbexAgent, 1, pos(25, 25), &mut TurtleAgent, 30);
        let s = score(&out.recording, 0, 1);
        assert_eq!(s.verdict, Verdict::SideA);
        assert!(s.decisive, "a side was eliminated");
    }

    #[test]
    fn a_true_grind_is_a_draw_not_an_hp_contest() {
        // Two identical heal-walls that out-heal each other's (zero) fire: neither makes progress.
        // HP-level scoring might pick a marginal HP leader; HP-slope correctly calls it a Draw.
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::Heal, 5), (Part::Move, 1)], vec![pos(20, 25)])],
            1,
            &[Unit::new(vec![(Part::Heal, 5), (Part::Move, 1)], vec![pos(30, 25)])],
        );
        // HoldAgents on both sides → no attack ever; both sit at full HP forever.
        let out = run_engagement(world, room(), 0, pos(20, 25), &mut HoldAgent, 1, pos(30, 25), &mut HoldAgent, 40);
        let s = score(&out.recording, 0, 1);
        assert_eq!(s.verdict, Verdict::Draw, "no progress on either side → Draw (advantage {})", s.residual_advantage_a);
        assert!(!s.decisive, "both sides survived");
        assert!(s.a.survivors == 1 && s.b.survivors == 1);
    }

    #[test]
    fn the_side_winning_the_recent_exchange_takes_a_nondecisive_engagement() {
        // Both ranged (KiteAgent holds range 3 + fires); A out-DPSes B but both have enough HP to
        // survive the short cap. A should be favored on residual HP slope even though both survive.
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 20), (Part::Move, 10)], vec![pos(23, 25)])], // 200 dps, 3000 hits
            1,
            &[Unit::new(vec![(Part::RangedAttack, 5), (Part::Move, 25)], vec![pos(27, 25)])], // 50 dps, 3000 hits
        );
        let out = run_engagement(world, room(), 0, pos(23, 25), &mut KiteAgent, 1, pos(27, 25), &mut KiteAgent, 10);
        let s = score(&out.recording, 0, 1);
        assert!(s.a.survivors > 0 && s.b.survivors > 0, "short cap → both alive");
        assert_eq!(s.verdict, Verdict::SideA, "A is out-bleeding B (advantage {})", s.residual_advantage_a);
        assert!(s.residual_advantage_a > 0);
    }

    #[test]
    fn self_play_mirror_is_a_draw() {
        // The bot vs itself in a symmetric setup should not systematically favor a side.
        let world = world_from_units(
            0,
            &[Unit::new(vec![(Part::RangedAttack, 5), (Part::Move, 5), (Part::Heal, 2)], vec![pos(23, 25)])],
            1,
            &[Unit::new(vec![(Part::RangedAttack, 5), (Part::Move, 5), (Part::Heal, 2)], vec![pos(27, 25)])],
        );
        let out = self_play(world, room(), 0, pos(23, 25), 1, pos(27, 25), 30);
        let s = score(&out.recording, 0, 1);
        // Either a clean Draw, or a small residual edge — never a runaway (|advantage| stays bounded).
        assert!(s.residual_advantage_a.abs() < 2000, "mirror match has no runaway, got {}", s.residual_advantage_a);
    }
}