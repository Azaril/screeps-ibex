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
use screeps_econ_eval::metrics::{family_h, repro_gate_verdict, PairedRun, RecoverConsts};
use screeps_econ_eval::runner::{run_scenario, RunOptions, RunOutcome};
use screeps_econ_eval::scenario::{catalog, fast_catalog, generate, EconScenario};
use std::time::Instant;

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(default)
}

fn line(out: &RunOutcome) -> String {
    format!(
        "  [{:<38}] T*={:>5} T_rec={:>6} η={:.3} leak(road/cont/other)={}/{}/{} blocked={:>4} idle={:.2} deficit∫={} {}",
        out.scenario,
        out.t_star,
        out.recovered_at.map(|t| t.to_string()).unwrap_or_else(|| "CAP".into()),
        out.eta,
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
    {
        let sc = bait[0].with_seed(base_seed);
        let a = run_scenario(&sc, &baseline_opts);
        let b = run_scenario(&sc, &baseline_opts);
        if (a.state_digest, a.report_digest) != (b.state_digest, b.report_digest) {
            println!("FENCE FAIL: two identical runs of `{}` diverged", sc.name);
            exit_code = 1;
        } else {
            println!("fence: 2-run digest check on `{}` OK ({:#x})", sc.name, a.state_digest);
        }
    }

    // ── Single-seed catalog pass: per-scenario lines + family/pooled H. ─────────────────────────
    println!("── catalog (baseline arm, seed {base_seed}) ──");
    let mut h_samples: Vec<(f64, f64)> = Vec::new();
    let mut records: Vec<serde_json::Value> = Vec::new();
    for sc in &bait {
        for arm in [sc.with_seed(base_seed), sc.control().with_seed(base_seed)] {
            let out = run_scenario(&arm, &baseline_opts);
            println!("{}", line(&out));
            deadlocks += u32::from(out.deadlocked);
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

    // ── THE REPRO GATE: N-seed paired bait vs control. ──────────────────────────────────────────
    println!("── THE REPRO GATE: {} scenarios × {} paired seeds (baseline arm) ──", bait.len(), n_seeds);
    let mut pairs: Vec<PairedRun> = Vec::new();
    for sc in &bait {
        for s in 0..n_seeds {
            let seed = base_seed + s;
            let b = run_scenario(&sc.with_seed(seed), &baseline_opts);
            let c = run_scenario(&sc.control().with_seed(seed), &baseline_opts);
            deadlocks += u32::from(b.deadlocked) + u32::from(c.deadlocked);
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
            "  [{:<30}] mean ΔT_recover={:+8.1}  mean bait leak={:8.1}e  (control leak={:.1}e)",
            sc.name,
            mean,
            mean_leak,
            sc_pairs.iter().map(|p| p.control_leak.total() as f64).sum::<f64>() / sc_pairs.len() as f64,
        );
    }
    let verdict = repro_gate_verdict(&pairs, base_seed);
    println!("┌──────────────────────────────────────────────────────────────────────┐");
    println!(
        "│ REPRO GATE: {}                                                      ",
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

    if deadlocks > 0 {
        println!("DEADLOCK SENTINEL: {deadlocks} occurrence(s) — hard gate");
        exit_code = 1;
    }

    // ── Stored baselines: runs/econ/<mode>-<sha>[-dirty][-cfg<digest>].json — keyed by (mode,
    // SHA, tree-dirty flag, non-default-config digest); NEVER written on a failed run (a
    // gate-failed / deadlocked / fence-failed run must not overwrite a good baseline —
    // EP-6.7's reviewed-baseline intent). The body records the full config (self-describing). ───
    if exit_code != 0 {
        println!("baseline NOT stored (exit {exit_code} — failed runs never overwrite baselines)");
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
                "gate": {
                    "pass": verdict.pass, "all_bait_leaked": verdict.all_bait_leaked,
                    "mean_delta": verdict.mean_delta, "ci95": [verdict.ci95.0, verdict.ci95.1],
                    "n_pairs": verdict.n_pairs,
                },
                "records": records,
            });
            if std::fs::write(&path, serde_json::to_string_pretty(&doc).expect("serialize")).is_ok() {
                println!("baseline stored: {}", path.display());
            }
        }
    }

    println!("econ_bench: done in {:.1?} (exit {exit_code})", started.elapsed());
    std::process::exit(exit_code);
}
