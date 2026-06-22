//! Self-play tournament + exploitability ship-gate (ADR 0020 §4.3 step 4 / §5).
//!
//! Generalizes the single-bed `sweep_kite_weights` into a **population tournament over a bed basket**:
//! every strategy (a `SquadTacticParams`) plays every other in symmetric self-play across a basket of
//! beds (open field, a wall corridor, mutual tower crossfire), each match scored by the net-HP
//! exchange, into an antisymmetric [`PayoffMatrix`]. From the matrix:
//! - a zero-sum **mean-payoff ranking** (who beats the field);
//! - the **exploitability** of a candidate — the largest margin any population strategy beats it by —
//!   the robustness **ship-gate** (is there a hard counter to how we fight? — the "adaptive, not
//!   counterable" test);
//! - a **meta-Nash mixed strategy** (fictitious play) — the robust mix to randomize over, which is the
//!   bridge to the step-6 adaptivity layer.
//!
//! The **bed basket** is what gives the gate teeth: a single open-field bed is low-decisiveness
//! (strategies tie), but terrain + tower pressure make positioning/engage choices actually diverge.
//! Budget is a tunable tier (operator §8.4): `Quick` (CI) vs `Thorough` (final eval). All matches run
//! on the deterministic Rust sim — reproducible, no GPU/ML.
//!
//! Residual (next): asymmetric attacker-vs-defender beds with the §8.6 objective-aware turtle scorer,
//! scripted archetypes vs the managed squad, PFSP opponent mixing + behavioral de-dup, and formal Elo
//! (equivalent to the mean-payoff ranking for a complete round-robin, so omitted here).

use screeps::{Position, RoomCoordinate, RoomName};
use screeps_combat_agent::squad::ManagedSimSquad;
use screeps_combat_decision::kite::{KiteScoreParams, SquadTacticParams};
use screeps_combat_engine::{CombatWorld, SimTower};

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
    /// CI / iteration — short matches.
    Quick,
    /// Final evaluation — longer matches (more of each fight resolves).
    Thorough,
}

impl TournamentBudget {
    fn ticks(self) -> usize {
        match self {
            TournamentBudget::Quick => 50,
            TournamentBudget::Thorough => 100,
        }
    }
}

/// A symmetric self-play bed. Each is mirror-symmetric (both sides identical, opposite ends) so the
/// antisymmetric payoff is meaningful; the basket spans the regimes where strategies diverge.
#[derive(Clone, Copy, Debug)]
pub enum Bed {
    /// Open room — a straight ranged brawl (low-decisiveness, the baseline).
    OpenField,
    /// A central wall with a 3-wide gap — both squads must thread the corridor to engage (cohesion +
    /// advance choices matter).
    Corridor,
    /// Each side has a tower covering the centre — fighting under mutual crossfire (the safety term +
    /// Lanchester heal/tower calc bite).
    TowerCrossfire,
}

/// The standard basket the tournament averages each match over.
pub const BASKET: [Bed; 3] = [Bed::OpenField, Bed::Corridor, Bed::TowerCrossfire];

/// Build a fresh symmetric world for `bed`: two identical 3×ranged squads at opposite ends.
fn build_bed(bed: Bed) -> CombatWorld {
    let mut creeps = ranged_file(0, 1, 8, 24, 3);
    creeps.extend(ranged_file(1, 11, 41, 24, 3));
    let mut world = CombatWorld { creeps, ..Default::default() };
    match bed {
        Bed::OpenField => {}
        Bed::Corridor => {
            for y in 0..=49u8 {
                if !(24..=26).contains(&y) {
                    world.terrain.walls.insert((25, y));
                }
            }
        }
        Bed::TowerCrossfire => {
            // One tower per side, mirrored, each firing at the other side's nearest creep (symmetric).
            world.towers.push(SimTower { id: 100, owner: 0, pos: pos(14, 25), energy: 1000, hits: 3000, hits_max: 3000 });
            world.towers.push(SimTower { id: 101, owner: 1, pos: pos(35, 25), energy: 1000, hits: 3000, hits_max: 3000 });
        }
    }
    world
}

/// One match on `bed`: side-0 fights with `side0`, side-1 with `side1`. Returns side-0's net-HP
/// advantage (HP it retained − HP side-1 retained); a wipe shows as the full margin (decisive).
fn play_bed(bed: Bed, side0: SquadTacticParams, side1: SquadTacticParams, ticks: usize) -> i64 {
    let mut world = build_bed(bed);
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

/// Antisymmetric payoff of `a` vs `b`, **averaged over the bed basket** and over both side
/// assignments (to cancel the start-side bias the deterministic tie-break introduces).
/// `payoff(a,b) == -payoff(b,a)`.
pub fn payoff(a: SquadTacticParams, b: SquadTacticParams, ticks: usize) -> i64 {
    let per_bed: i64 = BASKET.iter().map(|&bed| (play_bed(bed, a, b, ticks) - play_bed(bed, b, a, ticks)) / 2).sum();
    per_bed / BASKET.len() as i64
}

/// The shipped-default population the gate runs against — the default plus deliberate archetypes a
/// real opponent might field. A candidate any of these beats decisively is exploitable.
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

/// The result of a tournament: the antisymmetric payoff matrix, each strategy's mean payoff (the
/// zero-sum ranking score), and the meta-Nash mixed strategy (the robust mix to randomize over).
#[derive(Clone, Debug)]
pub struct TournamentResult {
    pub names: Vec<&'static str>,
    pub matrix: Vec<Vec<i64>>,
    /// `(strategy index, mean payoff over the field)`, best first.
    pub ranking: Vec<(usize, f64)>,
    /// Meta-Nash mixed strategy (probabilities over `names`) — the step-6 adaptivity mixing distribution.
    pub nash: Vec<f64>,
}

/// Meta-Nash mixed strategy of a symmetric zero-sum payoff matrix via **fictitious play**: each round
/// best-responds to the opponent's empirical mix; the empirical play frequencies converge to a Nash
/// equilibrium. The result is the robust randomization weight per strategy (dominated strategies → ~0).
pub fn meta_nash(matrix: &[Vec<i64>], iters: usize) -> Vec<f64> {
    let n = matrix.len();
    if n == 0 {
        return vec![];
    }
    let mut counts = vec![1.0f64; n]; // empirical play counts, start uniform
    for _ in 0..iters {
        let total: f64 = counts.iter().sum();
        let (mut best, mut best_v) = (0usize, f64::NEG_INFINITY);
        for (i, row) in matrix.iter().enumerate() {
            let v: f64 = row.iter().zip(&counts).map(|(&p, &c)| p as f64 * c / total).sum();
            if v > best_v {
                best_v = v;
                best = i;
            }
        }
        counts[best] += 1.0;
    }
    let total: f64 = counts.iter().sum();
    counts.iter().map(|c| c / total).collect()
}

/// Run the full round-robin over `strategies` (each pair over the bed basket) and rank + solve Nash.
pub fn run_tournament(strategies: &[Strategy], budget: TournamentBudget) -> TournamentResult {
    let ticks = budget.ticks();
    let n = strategies.len();
    let mut matrix = vec![vec![0i64; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let p = payoff(strategies[i].tactics, strategies[j].tactics, ticks);
            matrix[i][j] = p;
            matrix[j][i] = -p;
        }
    }
    let mut ranking: Vec<(usize, f64)> = (0..n).map(|i| (i, matrix[i].iter().sum::<i64>() as f64 / n.max(1) as f64)).collect();
    ranking.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let nash = meta_nash(&matrix, 2000);
    TournamentResult { names: strategies.iter().map(|s| s.name).collect(), matrix, ranking, nash }
}

/// **Exploitability** of `candidate` against `population`: the largest margin (net HP) any population
/// strategy beats it by. ≤ 0 ⇒ unexploitable by the field (a robust strategy). The ship-gate.
pub fn exploitability(candidate: SquadTacticParams, population: &[Strategy], budget: TournamentBudget) -> i64 {
    let ticks = budget.ticks();
    population.iter().map(|opp| payoff(opp.tactics, candidate, ticks)).max().unwrap_or(0)
}

/// Render a tournament result as a readable table (the tuning-loop dashboard).
pub fn report(result: &TournamentResult) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "Self-play tournament — {} strategies over {} beds (mean payoff | Nash weight):", result.names.len(), BASKET.len());
    for &(i, score) in &result.ranking {
        let _ = writeln!(s, "  {:>14}  {:+6.0} | {:.2}", result.names[i], score, result.nash[i]);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn tournament_matrix_is_antisymmetric_and_zero_sum() {
        let pop = strategy_population();
        let r = run_tournament(&pop, TournamentBudget::Quick);
        for i in 0..pop.len() {
            assert_eq!(r.matrix[i][i], 0, "a strategy ties itself");
            for j in 0..pop.len() {
                assert_eq!(r.matrix[i][j], -r.matrix[j][i], "payoff is antisymmetric");
            }
        }
        assert!(r.ranking.iter().map(|&(_, s)| s).sum::<f64>().abs() < 1.0, "zero-sum: ranking sums to ~0");
        // Nash is a valid distribution.
        assert!((r.nash.iter().sum::<f64>() - 1.0).abs() < 1e-6, "Nash mix sums to 1");
        assert!(r.nash.iter().all(|&w| w >= 0.0), "Nash weights are non-negative");
    }

    #[test]
    fn shipped_default_is_not_grossly_exploitable() {
        // The robustness ship-gate (ADR 0020): no population archetype beats the shipped default by
        // more than a gross margin across the bed basket — our default fighting style has no hard
        // counter in the field. (A tighter Nash/exploitability bound + asymmetric objective beds land
        // with the adaptivity layer; this is the standing regression guard.)
        let pop = strategy_population();
        let exploit = exploitability(SquadTacticParams::default(), &pop, TournamentBudget::Quick);
        println!("[ADR0020 tournament] default exploitability = {exploit} net HP\n{}", report(&run_tournament(&pop, TournamentBudget::Quick)));
        const GROSS: i64 = 1500; // ~1.5 creeps' HP; a real hard-counter exceeds this
        assert!(exploit <= GROSS, "the shipped default has a hard counter in the population ({exploit} net HP) — needs adaptivity or a retune");
    }

    #[test]
    fn ev_per_cpu_at_large_n_is_bounded() {
        // ADR 0020 §5/§7: a design that wins on HP but blows the per-tick CPU budget at large N must
        // FAIL the gate. Time a 10-v-10 managed self-play (the blob regime, step 5) and bound the
        // per-squad-tick cost. LOOSE (native-host proxy, like bench.rs) — a death-spiral guard, not a
        // tight Screeps-ms threshold.
        let mut world = build_bed(Bed::OpenField);
        // Scale up to 10 creeps a side.
        world.creeps = ranged_file(0, 1, 8, 20, 10);
        world.creeps.extend(ranged_file(1, 21, 41, 20, 10));
        let a_ids: Vec<_> = world.creeps.iter().filter(|c| c.owner == 0).map(|c| c.id).collect();
        let b_ids: Vec<_> = world.creeps.iter().filter(|c| c.owner == 1).map(|c| c.id).collect();
        let mut squads = [
            ManagedSimSquad::new(0, a_ids, pos(41, 25)),
            ManagedSimSquad::new(1, b_ids, pos(8, 25)),
        ];
        let ticks = 30usize;
        let start = Instant::now();
        run_managed(&mut world, &mut squads, ticks);
        let per_squad_tick_us = start.elapsed().as_secs_f64() * 1e6 / (ticks * 2) as f64;
        println!("[ADR0020 tournament] 10v10 EV/CPU = {per_squad_tick_us:.1} us/squad-tick");
        assert!(per_squad_tick_us < 20_000.0, "large-N managed combat blew the CPU budget: {per_squad_tick_us:.0} us/squad-tick");
    }
}
