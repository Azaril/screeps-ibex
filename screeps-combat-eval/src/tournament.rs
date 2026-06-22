//! Self-play tournament + exploitability ship-gate (ADR 0020 §4.3 step 4 / §5).
//!
//! Generalizes the single-bed `sweep_kite_weights` into a **population tournament**: every strategy
//! (a `SquadTacticParams`) plays every other in symmetric self-play, producing an antisymmetric
//! [`PayoffMatrix`] scored by the net-HP exchange. From the matrix we get a zero-sum mean-score
//! ranking and, crucially, the **exploitability** of a candidate — the largest margin any strategy in
//! the population beats it by. The shipped default fails the gate if it is *exploitable* (some
//! strategy crushes it), which is a far more robust signal than single-bed dominance: it answers "is
//! there a counter to how we fight," directly targeting the operator's "adaptive, not a fixed set of
//! counterable behaviors" goal.
//!
//! Budget is a tunable tier (operator §8.4): `Quick` for a per-change CI guard, `Thorough` for a
//! deeper final evaluation. Matches run on the deterministic Rust sim (no GPU/ML), so the whole
//! tournament is reproducible. v1 uses a symmetric open-field ranged self-play bed; the bed **basket**
//! (corridor / tower-pressure / mixed-composition) and Elo/meta-Nash ranking are the next increments.

use screeps::{Position, RoomCoordinate, RoomName};
use screeps_combat_agent::squad::ManagedSimSquad;
use screeps_combat_decision::kite::{KiteScoreParams, SquadTacticParams};
use screeps_combat_engine::CombatWorld;

use crate::{ranged_file, run_managed};

fn room() -> RoomName {
    "W1N1".parse().unwrap()
}
fn pos(x: u8, y: u8) -> Position {
    Position::new(RoomCoordinate::new(x).unwrap(), RoomCoordinate::new(y).unwrap(), room())
}

/// A named tactical strategy = the position-scoring weights the managed squad fights with.
#[derive(Clone, Copy, Debug)]
pub struct Strategy {
    pub name: &'static str,
    pub tactics: SquadTacticParams,
}

/// Tournament compute budget (operator §8.4): same code, different depth.
#[derive(Clone, Copy, Debug)]
pub enum TournamentBudget {
    /// CI / iteration — short matches, the core population.
    Quick,
    /// Final evaluation — longer matches (more of the fight resolves).
    Thorough,
}

impl TournamentBudget {
    fn ticks(self) -> usize {
        match self {
            TournamentBudget::Quick => 40,
            TournamentBudget::Thorough => 90,
        }
    }
}

/// The shipped-default population the gate runs against — the default plus deliberate archetypes a
/// real opponent might field (aggressive / cautious / kite-heavy / advance-heavy). A candidate that
/// any of these beats decisively is exploitable.
pub fn strategy_population() -> Vec<Strategy> {
    let base = SquadTacticParams::default();
    let with_engage = |f: fn(&mut KiteScoreParams)| {
        let mut e = base.engage;
        f(&mut e);
        SquadTacticParams { kite: base.kite, engage: e }
    };
    let with_kite = |f: fn(&mut KiteScoreParams)| {
        let mut k = base.kite;
        f(&mut k);
        SquadTacticParams { kite: k, engage: base.engage }
    };
    vec![
        Strategy { name: "default", tactics: base },
        Strategy { name: "aggressive", tactics: with_engage(|e| { e.w_dmg = 3.0; e.w_taken = 0.3; }) },
        Strategy { name: "cautious", tactics: with_engage(|e| { e.w_taken = 1.5; e.w_dmg = 1.0; }) },
        Strategy { name: "kite-heavy", tactics: with_kite(|k| { k.w_future = 2.0; k.w_prox = 1.0; }) },
        Strategy { name: "advance-heavy", tactics: with_engage(|e| { e.w_prox = 3.0; }) },
    ]
}

/// One symmetric self-play match: two identical 3-ranged squads start at opposite ends, each fighting
/// with its strategy's tactics. Returns **side-0's net-HP advantage** (HP it retained − HP side-1
/// retained); both sides start with equal HP, so >0 means side-0 fared better.
fn selfplay_net_hp(side0: SquadTacticParams, side1: SquadTacticParams, ticks: usize) -> i64 {
    let mut creeps = ranged_file(0, 1, 8, 24, 3);
    creeps.extend(ranged_file(1, 11, 41, 24, 3));
    let mut world = CombatWorld { creeps, ..Default::default() };
    let a_ids: Vec<_> = world.creeps.iter().filter(|c| c.owner == 0).map(|c| c.id).collect();
    let b_ids: Vec<_> = world.creeps.iter().filter(|c| c.owner == 1).map(|c| c.id).collect();
    let mut squads = [
        ManagedSimSquad::new(0, a_ids, pos(41, 25)).with_tactics(side0),
        ManagedSimSquad::new(1, b_ids, pos(8, 25)).with_tactics(side1),
    ];
    run_managed(&mut world, &mut squads, ticks);
    let kept = |owner| -> i64 { world.creeps.iter().filter(|c| c.owner == owner && c.is_alive()).map(|c| c.body.hits as i64).sum() };
    kept(0) - kept(1)
}

/// Antisymmetric payoff of `a` vs `b`: side bias (the deterministic tie-break slightly favours one
/// start) is cancelled by averaging `a`-as-side-0 with −(`b`-as-side-0). `payoff(a,b) == -payoff(b,a)`.
pub fn payoff(a: SquadTacticParams, b: SquadTacticParams, ticks: usize) -> i64 {
    (selfplay_net_hp(a, b, ticks) - selfplay_net_hp(b, a, ticks)) / 2
}

/// The result of a tournament: the antisymmetric payoff matrix (`matrix[i][j]` = strategy i vs j) +
/// each strategy's mean payoff (the zero-sum ranking score; higher = beats the field).
#[derive(Clone, Debug)]
pub struct TournamentResult {
    pub names: Vec<&'static str>,
    pub matrix: Vec<Vec<i64>>,
    /// `(strategy index, mean payoff over the field)`, best first.
    pub ranking: Vec<(usize, f64)>,
}

/// Run the full round-robin over `strategies` and rank by mean payoff.
pub fn run_tournament(strategies: &[Strategy], budget: TournamentBudget) -> TournamentResult {
    let ticks = budget.ticks();
    let n = strategies.len();
    let mut matrix = vec![vec![0i64; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let p = payoff(strategies[i].tactics, strategies[j].tactics, ticks);
            matrix[i][j] = p;
            matrix[j][i] = -p; // antisymmetric
        }
    }
    let mut ranking: Vec<(usize, f64)> = (0..n)
        .map(|i| (i, matrix[i].iter().sum::<i64>() as f64 / n.max(1) as f64))
        .collect();
    ranking.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    TournamentResult { names: strategies.iter().map(|s| s.name).collect(), matrix, ranking }
}

/// **Exploitability** of `candidate` against `population`: the largest margin (net HP) by which any
/// population strategy beats it. ≤ 0 ⇒ unexploitable by the field (a Nash-ish robust strategy). The
/// ship-gate: a candidate whose exploitability exceeds a tolerance has a hard counter — don't ship it
/// (or it needs the adaptivity layer to switch away from the countered behavior).
pub fn exploitability(candidate: SquadTacticParams, population: &[Strategy], budget: TournamentBudget) -> i64 {
    let ticks = budget.ticks();
    population.iter().map(|opp| payoff(opp.tactics, candidate, ticks)).max().unwrap_or(0)
}

/// Render a tournament result as a readable table (the tuning-loop dashboard).
pub fn report(result: &TournamentResult) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "Self-play tournament — {} strategies (mean payoff, best first):", result.names.len());
    for &(i, score) in &result.ranking {
        let _ = writeln!(s, "  {:>14}  {:+.0}", result.names[i], score);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tournament_matrix_is_antisymmetric_and_zero_sum() {
        let pop = strategy_population();
        let r = run_tournament(&pop, TournamentBudget::Quick);
        assert_eq!(r.matrix.len(), pop.len());
        // Antisymmetric (mirror-fair) + zero diagonal.
        for i in 0..pop.len() {
            assert_eq!(r.matrix[i][i], 0, "a strategy ties itself");
            for j in 0..pop.len() {
                assert_eq!(r.matrix[i][j], -r.matrix[j][i], "payoff is antisymmetric");
            }
        }
        // Zero-sum: the mean-payoff ranking sums to ~0.
        let total: f64 = r.ranking.iter().map(|&(_, s)| s).sum();
        assert!(total.abs() < 1.0, "zero-sum tournament: ranking sums to ~0 (got {total})");
    }

    #[test]
    fn shipped_default_is_not_grossly_exploitable() {
        // The robustness ship-gate (ADR 0020): no population archetype should beat the shipped default
        // by more than a gross margin — i.e. our default fighting style has no hard counter in the
        // field. (A tighter Nash/exploitability bound + a bed basket land with the adaptivity layer;
        // this is the standing regression guard.)
        let pop = strategy_population();
        let default = SquadTacticParams::default();
        let exploit = exploitability(default, &pop, TournamentBudget::Quick);
        println!("[ADR0020 tournament] default exploitability = {exploit} net HP\n{}", report(&run_tournament(&pop, TournamentBudget::Quick)));
        const GROSS: i64 = 1500; // ~1.5 creeps' worth of HP; a real hard-counter would exceed this
        assert!(exploit <= GROSS, "the shipped default has a hard counter in the population ({exploit} net HP) — needs adaptivity or a retune");
    }
}
