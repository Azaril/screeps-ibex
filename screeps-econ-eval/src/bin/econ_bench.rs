//! **econ_bench** — the ADR 0040 M1 economy benchmark CLI (spec Part C.7):
//!
//!   cargo run --release -p screeps-econ-eval --bin econ_bench -- [fast|full]
//!
//! `fast` (default): the 4-pair fast catalog through THE REPRO GATE (N seeds) + the single-seed
//! catalog H pass + the S1-arm sanity A/B. `full`: the whole 10-pair catalog + 3 generated pairs.
//!
//! Env overrides (the repo's env-driven bench idiom; unset = defaults):
//!   ECON_SEED=1                     base seed (gate seeds are base..base+N)
//!   ECON_GATE_SEEDS=10              N paired seeds per scenario (the gate's N ≥ 10)
//!   ECON_TICK_CAP=15000             per-run tick cap
//!   ECON_RECOVER_FULL_WINDOW=50     RECOVER_FULL_WINDOW (metrics.rs)
//!   ECON_RECOVER_INCOME_FRAC_Q=900  RECOVER_INCOME_FRAC in per-mille
//!   ECON_RECOVER_INCOME_WINDOW=300  the trailing-income window
//!
//! Non-zero exit on: the in-process determinism fence failing, a conservation failure (the
//! runner panics — loud), any deadlock-sentinel occurrence, or THE REPRO GATE failing.
//! Baselines are stored under `runs/econ/` keyed (scenario, seed, SHA) — the rover_bench
//! stored-baseline shape.

use screeps_econ_eval::baseline::PolicyConfig;
use screeps_econ_eval::metrics::{family_h, percentile_u32, repro_gate_verdict, PairedRun, RecoverConsts};
use screeps_econ_eval::movement::AnalyticMover;
use screeps_econ_eval::runner::{run_scenario, run_world, RunGoal, RunOptions, RunOutcome};
use screeps_econ_eval::scenario::{
    catalog, fast_catalog, fast_downgrade_catalog, downgrade_catalog, generate, rush_catalog,
    steady_catalog, EconScenario, RushScenario, SteadyScenario,
};
use std::collections::BTreeMap;
use std::time::Instant;

/// Run one Family-G rush (its own instantiation path — greenfield, RCL goal, phase-seeded pass).
fn run_rush(rush: &RushScenario, cfg: PolicyConfig, consts: RecoverConsts, tick_cap: u32) -> RunOutcome {
    let (mut world, terrain, info) = rush.instantiate();
    let mut mover = AnalyticMover::new(&terrain);
    let mut opts = RunOptions::new(cfg, consts, tick_cap).with_goal(RunGoal::Rcl { target: rush.target_rcl });
    opts.construction_phase = rush.seed;
    run_world(&rush.shell(), &mut world, &mut mover, &info, &opts)
}

/// Run one Family-S horizon (healthy world, Horizon goal).
fn run_steady(sc: &SteadyScenario, cfg: PolicyConfig, consts: RecoverConsts) -> RunOutcome {
    let (mut world, terrain, info) = sc.instantiate();
    let mut mover = AnalyticMover::new(&terrain);
    let mut opts = RunOptions::new(cfg, consts, sc.tick_cap).with_goal(RunGoal::Horizon);
    opts.construction_phase = sc.seed;
    run_world(&sc.shell(), &mut world, &mut mover, &info, &opts)
}

fn episode_stats(episodes: &[u32]) -> (usize, f64, u32, u32) {
    let n = episodes.len();
    let mean = if n == 0 { 0.0 } else { episodes.iter().map(|&e| e as f64).sum::<f64>() / n as f64 };
    (n, mean, percentile_u32(episodes, 0.95), episodes.iter().copied().max().unwrap_or(0))
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(default)
}

fn line(out: &RunOutcome) -> String {
    format!(
        "  [{:<38}] T*={:>5} T_rec={:>6} η={:.3} selfsuff={:>6} leak(road/cont/other)={}/{}/{} blocked={:>4} idle={:.2} deficit∫={} {}",
        out.scenario,
        out.t_star,
        out.recovered_at.map(|t| t.to_string()).unwrap_or_else(|| "CAP".into()),
        out.eta,
        out.self_sufficient_at.map(|t| t.to_string()).unwrap_or_else(|| "never".into()),
        out.diagnostics.leak.roads,
        out.diagnostics.leak.containers,
        out.diagnostics.leak.other,
        out.diagnostics.spawn_energy_blocked_ticks,
        out.diagnostics.spawn_idle_frac(),
        out.diagnostics.extension_deficit_integral,
        if out.deadlocked { "DEADLOCK" } else { "" },
    )
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "fast".into());
    let mut bait: Vec<EconScenario> = match which.as_str() {
        "fast" => fast_catalog(),
        "full" => {
            let mut all = catalog();
            all.extend((100..103).map(generate));
            all
        }
        other => {
            eprintln!("econ_bench: unknown mode `{other}` (expected `fast` or `full`)");
            std::process::exit(2);
        }
    };
    let base_seed = env_u32("ECON_SEED", 1);
    let n_seeds = env_u32("ECON_GATE_SEEDS", 10).max(1);
    let tick_cap = env_u32("ECON_TICK_CAP", 15_000);
    let consts = RecoverConsts::from_env();
    for sc in &mut bait {
        sc.tick_cap = tick_cap;
    }
    let baseline_opts = RunOptions::new(PolicyConfig { s1_allowance: false }, consts, tick_cap);
    let s1_opts = RunOptions::new(PolicyConfig { s1_allowance: true }, consts, tick_cap);

    println!(
        "econ_bench: mode={which} ({} bait scenarios) seeds={base_seed}..{} tick_cap={tick_cap} recover=({}, {}‰, {}t)",
        bait.len(),
        base_seed + n_seeds - 1,
        consts.full_window,
        consts.income_frac_q,
        consts.income_window,
    );
    let started = Instant::now();
    let mut exit_code = 0;
    let mut deadlocks = 0u32;

    // ── The in-process determinism fence (coarse): the first bait scenario, twice, bit-equal. ──
    let mut fence_failed = false;
    {
        let sc = bait[0].with_seed(base_seed);
        let a = run_scenario(&sc, &baseline_opts);
        let b = run_scenario(&sc, &baseline_opts);
        if (a.state_digest, a.report_digest) != (b.state_digest, b.report_digest) {
            println!("FENCE FAIL: two identical runs of `{}` diverged", sc.name);
            fence_failed = true;
            exit_code = 1;
        } else {
            println!("fence: 2-run digest check on `{}` OK ({:#x})", sc.name, a.state_digest);
        }
    }

    // ── Single-seed catalog pass: per-scenario lines + family/pooled H. ─────────────────────────
    println!("── catalog (baseline arm, seed {base_seed}) ──");
    let mut h_samples: Vec<(f64, f64)> = Vec::new();
    let mut records: Vec<serde_json::Value> = Vec::new();
    let mut c_effective: BTreeMap<String, u32> = BTreeMap::new(); // Family D's paired baseline
    for sc in &bait {
        for arm in [sc.with_seed(base_seed), sc.control().with_seed(base_seed)] {
            let out = run_scenario(&arm, &baseline_opts);
            println!("{}", line(&out));
            deadlocks += u32::from(out.deadlocked);
            c_effective.insert(out.scenario.clone(), out.effective_t);
            h_samples.push((out.eta, out.t_star as f64));
            records.push(serde_json::json!({
                "scenario": out.scenario, "seed": out.seed, "t_star": out.t_star,
                "t_recover": out.recovered_at, "eta": out.eta,
                "leak_roads": out.diagnostics.leak.roads,
                "leak_containers": out.diagnostics.leak.containers,
                "blocked_ticks": out.diagnostics.spawn_energy_blocked_ticks,
                "state_digest": format!("{:#x}", out.state_digest),
            }));
        }
    }
    let h = family_h(&h_samples, base_seed);
    println!(
        "family C: H={:.4} CI95=[{:.4},{:.4}] p05={:.3} p95={:.3} n={}",
        h.weighted_mean, h.ci95.0, h.ci95.1, h.p05, h.p95, h.n
    );
    println!(
        "pooled:   H={:.4} CI95=[{:.4},{:.4}]  (one family in M1 — pooled == family C)",
        h.weighted_mean, h.ci95.0, h.ci95.1
    );

    // ── THE REPRO GATE: N-seed paired bait vs control. **Official recovered state = LANE-ONLY
    // (the #7 decision, metrics::RecoverConsts docs)** — the demoted income condition is
    // reported alongside as the self-sufficiency diagnostic, never gating. ──────────────────────
    println!("── THE REPRO GATE: {} scenarios × {} paired seeds (baseline arm; lane-only recovery per #7) ──", bait.len(), n_seeds);
    let mut pairs: Vec<PairedRun> = Vec::new();
    let mut ss_diag: Vec<serde_json::Value> = Vec::new();
    for sc in &bait {
        let (mut bait_ss, mut ctrl_ss) = (0u32, 0u32);
        let mut bait_ss_ts: Vec<f64> = Vec::new();
        for s in 0..n_seeds {
            let seed = base_seed + s;
            let b = run_scenario(&sc.with_seed(seed), &baseline_opts);
            let c = run_scenario(&sc.control().with_seed(seed), &baseline_opts);
            deadlocks += u32::from(b.deadlocked) + u32::from(c.deadlocked);
            bait_ss += u32::from(b.self_sufficient_at.is_some());
            ctrl_ss += u32::from(c.self_sufficient_at.is_some());
            if let Some(t) = b.self_sufficient_at {
                bait_ss_ts.push(t as f64);
            }
            pairs.push(PairedRun {
                scenario: sc.name.clone(),
                seed,
                bait_t: b.effective_t,
                control_t: c.effective_t,
                bait_recovered: b.recovered_at.is_some(),
                control_recovered: c.recovered_at.is_some(),
                bait_leak: b.diagnostics.leak,
                control_leak: c.diagnostics.leak,
            });
        }
        let sc_pairs: Vec<&PairedRun> = pairs.iter().filter(|p| p.scenario == sc.name).collect();
        let mean: f64 = sc_pairs.iter().map(|p| p.delta() as f64).sum::<f64>() / sc_pairs.len() as f64;
        let mean_leak: f64 =
            sc_pairs.iter().map(|p| p.bait_leak.total() as f64).sum::<f64>() / sc_pairs.len() as f64;
        println!(
            "  [{:<30}] mean ΔT_recover={:+8.1}  mean bait leak={:8.1}e  (control leak={:.1}e)  selfsuff bait {bait_ss}/{n_seeds} ctrl {ctrl_ss}/{n_seeds}",
            sc.name,
            mean,
            mean_leak,
            sc_pairs.iter().map(|p| p.control_leak.total() as f64).sum::<f64>() / sc_pairs.len() as f64,
        );
        ss_diag.push(serde_json::json!({
            "scenario": sc.name, "bait_selfsufficient_runs": bait_ss, "control_selfsufficient_runs": ctrl_ss,
            "n_seeds": n_seeds,
            "bait_mean_t_selfsuff": if bait_ss_ts.is_empty() { serde_json::Value::Null } else {
                serde_json::json!(bait_ss_ts.iter().sum::<f64>() / bait_ss_ts.len() as f64) },
        }));
    }
    let verdict = repro_gate_verdict(&pairs, base_seed);
    println!("┌──────────────────────────────────────────────────────────────────────┐");
    println!(
        "│ REPRO GATE (lane-only recovery, #7): {}                             ",
        if verdict.pass { "PASS ✅" } else { "FAIL ❌" }
    );
    println!(
        "│  (a) repair_leak_e > 0 on every bait run: {}  ({}/{} runs leaked)",
        if verdict.all_bait_leaked { "YES" } else { "NO" },
        verdict.bait_runs_leaked,
        verdict.n_pairs
    );
    println!(
        "│  (b) pooled ΔT_recover (bait − control): mean {:+.1} ticks, 95% CI [{:+.1}, {:+.1}] — {}",
        verdict.mean_delta,
        verdict.ci95.0,
        verdict.ci95.1,
        if verdict.ci_excludes_zero { "excludes zero" } else { "DOES NOT exclude zero" }
    );
    println!("└──────────────────────────────────────────────────────────────────────┘");
    if !verdict.pass {
        exit_code = 1;
    }

    // ── The S1-arm sanity A/B (report, NOT gated — the real A/B is M4). ─────────────────────────
    println!("── S1-allowance arm sanity A/B (bait scenarios, seed {base_seed}) ──");
    let mut leak_base = 0f64;
    let mut leak_s1 = 0f64;
    let mut dt: Vec<f64> = Vec::new();
    for sc in &bait {
        let b = run_scenario(&sc.with_seed(base_seed), &baseline_opts);
        let s = run_scenario(&sc.with_seed(base_seed), &s1_opts);
        deadlocks += u32::from(s.deadlocked);
        println!(
            "  [{:<30}] leak {:>7}e → {:>5}e   T_rec {:>6} → {:>6}   (Δ {:+})",
            sc.name,
            b.diagnostics.leak.total(),
            s.diagnostics.leak.total(),
            b.effective_t,
            s.effective_t,
            s.effective_t as i64 - b.effective_t as i64,
        );
        leak_base += b.diagnostics.leak.total() as f64;
        leak_s1 += s.diagnostics.leak.total() as f64;
        dt.push(s.effective_t as f64 - b.effective_t as f64);
    }
    println!(
        "S1 arm: mean leak {:.1}e → {:.1}e ({:.1}% of baseline), mean ΔT_recover (S1 − baseline) {:+.1} ticks",
        leak_base / bait.len() as f64,
        leak_s1 / bait.len() as f64,
        if leak_base > 0.0 { 100.0 * leak_s1 / leak_base } else { 0.0 },
        dt.iter().sum::<f64>() / dt.len() as f64,
    );

    let family_c_wall = started.elapsed();

    // ══ Family G — greenfield rush curves (M2). Splits FIXED FROM DATA (the M2 cost
    // measurement: ~50k ticks ≈ 0.5 s per N=4 run at ~100 ticks/ms on the analytic tier):
    // fast = 4 layouts × 2 seeds to N=4; full = all 13 layouts × 3 seeds to N=4 + a 3-layout
    // N=5 probe (N=6 stays out: +1.215M progress ≈ 300k+ ticks/run — not worth the wall-clock
    // until a policy candidate exists to compare). ═══════════════════════════════════════════════
    let g_started = Instant::now();
    let g_tick_cap = env_u32("ECON_G_TICK_CAP", 120_000);
    let g_seeds = env_u32("ECON_G_SEEDS", if which == "full" { 3 } else { 2 });
    let g_rushes: Vec<RushScenario> = if which == "full" {
        rush_catalog(4, g_seeds)
    } else {
        let rooms = ["E11N1", "E12S41", "E11N14", "E11N23"];
        rooms
            .iter()
            .flat_map(|r| (1..=g_seeds).map(move |s| RushScenario::new(r, 4, s)))
            .collect()
    };
    println!("── Family G: greenfield rush ({} runs, target RCL 4, cap {g_tick_cap}) ──", g_rushes.len());
    let mut g_samples: Vec<(f64, f64)> = Vec::new();
    let mut g_by_layout: BTreeMap<String, Vec<&RunOutcome>> = BTreeMap::new();
    let mut g_outcomes: Vec<RunOutcome> = Vec::new();
    let mut eta_gate_violations = 0u32;
    for rush in &g_rushes {
        let out = run_rush(rush, PolicyConfig { s1_allowance: false }, consts, g_tick_cap);
        deadlocks += u32::from(out.deadlocked);
        // Review A1: the gate reads the UNCLAMPED ratio — `eta` is clamped to 1 for H and can
        // never trip this.
        if out.eta_raw > 1.01 {
            eta_gate_violations += 1;
            println!("  ORACLE VIOLATION: {} η_raw = {:.3} > 1+ε", out.scenario, out.eta_raw);
        }
        g_samples.push((out.eta, out.t_star as f64));
        g_outcomes.push(out);
    }
    for (rush, out) in g_rushes.iter().zip(&g_outcomes) {
        g_by_layout.entry(rush.layout_room.clone()).or_default().push(out);
    }
    for (room, outs) in &g_by_layout {
        let curve = |n: u8| {
            let ts: Vec<f64> = outs.iter().filter_map(|o| o.t_rcl.get(&n).map(|&t| t as f64)).collect();
            if ts.is_empty() {
                ("  CAP".to_string(), 0)
            } else {
                let mean = ts.iter().sum::<f64>() / ts.len() as f64;
                let lo = ts.iter().cloned().fold(f64::INFINITY, f64::min);
                let hi = ts.iter().cloned().fold(0.0f64, f64::max);
                (format!("{mean:.0} [{lo:.0},{hi:.0}]"), ts.len())
            }
        };
        let (t2, _) = curve(2);
        let (t3, _) = curve(3);
        let (t4, n4) = curve(4);
        let eta = outs.iter().map(|o| o.eta).sum::<f64>() / outs.len() as f64;
        println!(
            "  [{room:<8}] T_RCL2={t2:<18} T_RCL3={t3:<22} T_RCL4={t4:<22} ({n4}/{} reached) η̄={eta:.3}",
            outs.len()
        );
    }
    let g_h = family_h(&g_samples, base_seed);
    println!(
        "family G: H_rcl={:.4} CI95=[{:.4},{:.4}] n={} (oracle: bare-fleet conservation bound — build spend excluded, see t_star_rcl docs)",
        g_h.weighted_mean, g_h.ci95.0, g_h.ci95.1, g_h.n
    );
    // The N=5 probe (full only): 3 layouts × 1 seed — the data behind the fast/full split.
    let mut g5_lines: Vec<String> = Vec::new();
    if which == "full" {
        for room in ["E11N1", "E12S41", "E11N14"] {
            let mut rush = RushScenario::new(room, 5, 1);
            rush.tick_cap = 400_000;
            let t0 = Instant::now();
            let out = run_rush(&rush, PolicyConfig { s1_allowance: false }, consts, 400_000);
            deadlocks += u32::from(out.deadlocked);
            if out.eta_raw > 1.01 {
                eta_gate_violations += 1;
                println!("  ORACLE VIOLATION: {} η_raw = {:.3} > 1+ε", out.scenario, out.eta_raw);
            }
            let line = format!(
                "  [G5 {room:<8}] T_RCL5={} T*={} η={:.3} ticks={} wall={:.1?}",
                out.t_rcl.get(&5).map(|t| t.to_string()).unwrap_or_else(|| "CAP".into()),
                out.t_star,
                out.eta,
                out.ticks_run,
                t0.elapsed()
            );
            println!("{line}");
            g5_lines.push(line);
        }
    }
    if eta_gate_violations > 0 {
        println!("ORACLE SANITY GATE: {eta_gate_violations} η > 1+ε violation(s) — hard gate");
        exit_code = 1;
    }
    let g_wall = g_started.elapsed();

    // ══ Family D — downgrade pressure (clock at 10%): the refill-vs-controller triage, scored
    // by T_recover AND levels_lost. Review B5: the run continues past recovery until the CLOCK
    // QUESTION RESOLVES (back at/above half-max, `RunGoal::RecoverThenClockSafe`) — ending at
    // recovery alone made "no downgrade" vacuous wherever the 10% clock outlived the recovery
    // horizon (7-9/10 of the catalog). `resolved` reports whether the goal was actually met. ═════
    let d_started = Instant::now();
    let d_scenarios = if which == "full" { downgrade_catalog() } else { fast_downgrade_catalog() };
    println!(
        "── Family D: downgrade pressure ({} scenarios, clock 10%, seed {base_seed}; runs until recovered AND clock ≥ half-max) ──",
        d_scenarios.len()
    );
    let d_opts = baseline_opts.with_goal(RunGoal::RecoverThenClockSafe);
    let mut d_records: Vec<serde_json::Value> = Vec::new();
    let mut d_saved = 0u32;
    let mut d_lost = 0u32;
    let mut d_unresolved = 0u32;
    for sc in &d_scenarios {
        let mut sc = sc.clone();
        sc.tick_cap = tick_cap;
        let out = run_scenario(&sc.with_seed(base_seed), &d_opts);
        deadlocks += u32::from(out.deadlocked);
        // The paired C-family run (clock 100%, same layout/axes/seed) from the catalog pass.
        let c_name = format!("{}#s{base_seed}", sc.name.trim_start_matches("D-"));
        let c_t = c_effective.get(&c_name).copied();
        // The clock question resolved iff the run STOPPED before the cap (goal met) — otherwise
        // the verdict is honest-unknown, not "saved".
        let resolved = out.ticks_run < tick_cap && !out.deadlocked;
        if !resolved {
            d_unresolved += 1;
        } else if out.levels_lost == 0 {
            d_saved += 1;
        } else {
            d_lost += 1;
        }
        println!(
            "  [{:<40}] T_rec={:>6} clock_safe_at={:>6} levels_lost={} final={:?} ΔT_rec vs C={:}",
            out.scenario,
            out.recovered_at.map(|t| t.to_string()).unwrap_or_else(|| "CAP".into()),
            if resolved { out.ticks_run.to_string() } else { "UNRESOLVED".into() },
            out.levels_lost,
            out.final_controller,
            c_t.map(|c| format!("{:+}", out.effective_t as i64 - c as i64)).unwrap_or_else(|| "n/a".into()),
        );
        d_records.push(serde_json::json!({
            "scenario": out.scenario, "t_recover": out.recovered_at, "levels_lost": out.levels_lost,
            "clock_resolved": resolved, "clock_safe_at": if resolved { serde_json::json!(out.ticks_run) } else { serde_json::Value::Null },
            "final_controller": out.final_controller.map(|(l, p)| serde_json::json!([l, p])),
            "c_effective_t": c_t,
        }));
    }
    println!(
        "family D: {d_saved}/{} clocks saved to half-max with zero downgrades, {d_lost} lost ≥ 1 level, {d_unresolved} unresolved at cap",
        d_scenarios.len()
    );
    let d_wall = d_started.elapsed();

    // ══ Family S — THE GUARD RAIL: healthy rooms (incl. LOW-RCL — the §D8 #2 evidence channel),
    // 10k-tick horizon, baseline vs S1 arm. ═══════════════════════════════════════════════════════
    let s_started = Instant::now();
    let s_all = steady_catalog(base_seed);
    let s_scenarios: Vec<SteadyScenario> =
        if which == "full" { s_all } else { s_all.into_iter().take(2).collect() };
    println!("── Family S: steady-state guard rail ({} scenarios × 2 arms, 10k ticks) ──", s_scenarios.len());
    let mut s_records: Vec<serde_json::Value> = Vec::new();
    for sc in &s_scenarios {
        let mut arm_summaries: Vec<serde_json::Value> = Vec::new();
        let mut arm_digests: Vec<(u64, u64)> = Vec::new();
        for (arm, cfg) in [("base", PolicyConfig { s1_allowance: false }), ("s1", PolicyConfig { s1_allowance: true })] {
            let out = run_steady(sc, cfg, consts);
            deadlocks += u32::from(out.deadlocked);
            arm_digests.push((out.state_digest, out.report_digest));
            let stock = |i: usize| out.road_stock.get(i).map(|&(_, h, m)| h as f64 / m.max(1) as f64);
            let start = stock(0).unwrap_or(1.0);
            let end = out.road_stock.last().map(|&(_, h, m)| h as f64 / m.max(1) as f64).unwrap_or(1.0);
            let min = out
                .road_stock
                .iter()
                .map(|&(_, h, m)| h as f64 / m.max(1) as f64)
                .fold(f64::INFINITY, f64::min);
            let (n_ep, ep_mean, ep_p95, ep_max) = episode_stats(&out.deficit_episodes);
            let flap = out.assignments as f64 / (out.ticks_run.max(1) as f64 / 1000.0);
            let ipt = out.intents_emitted as f64 / out.ticks_run.max(1) as f64;
            // Review A3: a still-open terminal deficit episode is part of the latency story.
            let open = out
                .deficit_open_at_end
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into());
            println!(
                "  [{:<22}][{arm:<4}] idle={:.3} leak={:>6} road {:.3}→{:.3} (min {:.3}) refill n={n_ep} μ={ep_mean:.1} p95={ep_p95} max={ep_max} open@end={open} flap/kt={flap:.0} intents/t={ipt:.2} built={:?} lost={}",
                out.scenario,
                out.diagnostics.spawn_idle_frac(),
                out.diagnostics.leak.total(),
                start, end, min,
                out.sites_built,
                out.levels_lost,
            );
            arm_summaries.push(serde_json::json!({
                "arm": arm,
                "spawn_idle_frac": out.diagnostics.spawn_idle_frac(),
                "repair_leak_e": out.diagnostics.leak.total(),
                // Review B8: the trajectory rides the FIXED ever-seen-tile denominator (dead
                // roads stay in it at hits 0 — no survivorship bias; see runner.rs).
                "road_stock": {"start": start, "end": end, "min": min,
                    "denominator": "fixed (ever-seen road tiles; survivorship-bias-free)",
                    "trajectory": out.road_stock.iter().map(|&(t, h, m)| serde_json::json!([t, h, m])).collect::<Vec<_>>()},
                "refill_episodes": {"n": n_ep, "mean": ep_mean, "p95": ep_p95, "max": ep_max,
                    "open_at_end": out.deficit_open_at_end},
                "extension_deficit_integral": out.diagnostics.extension_deficit_integral,
                "self_sufficient_at": out.self_sufficient_at,
                "flap_per_kilotick": flap,
                "intents_per_tick": ipt,
                "sites_built": out.sites_built.iter().map(|(k, v)| serde_json::json!([k, v])).collect::<Vec<_>>(),
                "levels_lost": out.levels_lost,
                "deadlocked": out.deadlocked,
            }));
        }
        // Review B7: the S1-vs-baseline "identical" claim at digest level, not metric level.
        let digest_identical = arm_digests.len() == 2 && arm_digests[0] == arm_digests[1];
        println!(
        "    ↳ arms digest-identical: {} (state {:#x}/{:#x})",
            if digest_identical { "YES" } else { "NO" },
            arm_digests[0].0,
            arm_digests[1].0,
        );
        s_records.push(serde_json::json!({
            "scenario": sc.name,
            "arms_digest_identical": digest_identical,
            "arms": arm_summaries,
        }));
    }
    let s_wall = s_started.elapsed();

    // ══ The cost table + the data-driven splits (EP-4.6). ═══════════════════════════════════════
    println!("── long-horizon cost (analytic tier) ──");
    println!("  family C ({} runs incl. gate): {:.1?}", 2 * bait.len() * (1 + n_seeds as usize) + 2 * bait.len(), family_c_wall);
    println!("  family G ({} runs): {:.1?} (~0.5 s per 50k-tick N=4 rush ≈ 100 ticks/ms)", g_rushes.len(), g_wall);
    println!("  family D ({} runs): {:.1?}", d_scenarios.len(), d_wall);
    println!("  family S ({} runs × 10k ticks): {:.1?} (~120 ms per 10k-tick horizon)", 2 * s_scenarios.len(), s_wall);
    println!("  splits FIXED FROM THIS DATA: fast = C-fast gate + G 4×2@N4 + D fast + S 2×2arms;");
    println!("                               full = C-full gate + G 13×3@N4 + G5 probe(3) + D full + S 6×2arms; N=6 excluded (≈300k+ ticks/run)");

    if deadlocks > 0 {
        println!("DEADLOCK SENTINEL: {deadlocks} occurrence(s) — hard gate");
        exit_code = 1;
    }

    // ── Stored baselines: runs/econ/<mode>-<sha>[-dirty][-cfg<digest>].json — keyed by (mode,
    // SHA, tree-dirty flag, non-default-config digest); NEVER written on a STRUCTURAL failure
    // (fence divergence / deadlock sentinel / oracle-sanity violation / conservation panic — a
    // broken run must not overwrite a good baseline, EP-6.7). The repro-gate VERDICT is recorded
    // in the doc either way (M2: the default-consts gate is #7-pathology-sensitive; the doc IS
    // the decision evidence — the exit code still reports it). ──────────────────────────────────
    let structural_failure = fence_failed || deadlocks > 0 || eta_gate_violations > 0;
    if structural_failure {
        println!("baseline NOT stored (structural failure — fence/deadlock/oracle; failed runs never overwrite baselines)");
    } else {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nosha".into());
        let dirty = git(&["status", "--porcelain"]).map(|s| !s.is_empty()).unwrap_or(true);
        // A short config digest, appended only when the run deviates from the defaults —
        // default-config baselines keep a stable key; overridden runs never clobber them.
        let defaults = RecoverConsts::default();
        let is_default = base_seed == 1
            && n_seeds == 10
            && tick_cap == 15_000
            && consts.full_window == defaults.full_window
            && consts.income_frac_q == defaults.income_frac_q
            && consts.income_window == defaults.income_window;
        let cfg_suffix = if is_default {
            String::new()
        } else {
            let mut d: u64 = 0xcbf2_9ce4_8422_2325;
            for v in [base_seed, n_seeds, tick_cap, consts.full_window, consts.income_frac_q, consts.income_window] {
                for b in v.to_le_bytes() {
                    d ^= b as u64;
                    d = d.wrapping_mul(0x0000_0100_0000_01B3);
                }
            }
            format!("-cfg{:08x}", (d >> 32) as u32)
        };
        let key = format!("{which}-{sha}{}{cfg_suffix}", if dirty { "-dirty" } else { "" });
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runs/econ");
        if std::fs::create_dir_all(&dir).is_ok() {
            let path = dir.join(format!("{key}.json"));
            let doc = serde_json::json!({
                "mode": which, "sha": sha, "dirty": dirty,
                "base_seed": base_seed, "gate_seeds": n_seeds, "tick_cap": tick_cap,
                "recover_consts": {
                    "full_window": consts.full_window,
                    "income_frac_q": consts.income_frac_q,
                    "income_window": consts.income_window,
                },
                "family_h": h.weighted_mean, "h_ci95": [h.ci95.0, h.ci95.1],
                // The OFFICIAL gate — lane-only recovered state per the #7 decision
                // (metrics::RecoverConsts docs; the former income condition is the
                // self_sufficiency diagnostic below, reported, never gating).
                "gate": {
                    "recovered_state": "lane-only (#7)",
                    "pass": verdict.pass, "all_bait_leaked": verdict.all_bait_leaked,
                    "mean_delta": verdict.mean_delta, "ci95": [verdict.ci95.0, verdict.ci95.1],
                    "n_pairs": verdict.n_pairs,
                },
                // The demoted 0.9 income-frac cliff, per scenario (#7 diagnostic).
                "self_sufficiency": ss_diag,
                "records": records,
                // ── M2 family sections ─────────────────────────────────────────────────────
                "family_g": {
                    "h_rcl": g_h.weighted_mean, "ci95": [g_h.ci95.0, g_h.ci95.1],
                    "tick_cap": g_tick_cap, "seeds": g_seeds,
                    "runs": g_outcomes.iter().map(|o| serde_json::json!({
                        "scenario": o.scenario, "t_star": o.t_star, "eta": o.eta,
                        "t_rcl": o.t_rcl.iter().map(|(&l, &t)| serde_json::json!([l, t])).collect::<Vec<_>>(),
                        "ticks": o.ticks_run,
                    })).collect::<Vec<_>>(),
                    "g5_probe": g5_lines,
                },
                "family_d": d_records,
                "family_s": s_records,
            });
            if std::fs::write(&path, serde_json::to_string_pretty(&doc).expect("serialize")).is_ok() {
                println!("baseline stored: {}", path.display());
            }
        }
    }

    println!("econ_bench: done in {:.1?} (exit {exit_code})", started.elapsed());
    std::process::exit(exit_code);
}
