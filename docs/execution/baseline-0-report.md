# BASELINE-0 Report — pre-Phase-0 bot behavior

- **Date:** 2026-06-10 · **Bot code:** unchanged `master` lineage (no Phase-0 bot edits exist yet; harness commit `8983bc7`)
- **Artifacts:** `runs/baseline-0-8983bc7-20260610-034053/` (gitignored; `console.jsonl`, `metrics.jsonl`, `summary.json`)
- **Purpose:** the *before* measurement bracketing Phase 0 ([phase-0.md](phase-0.md) §5). BASELINE-1 re-runs the identical scenario after Workstreams C (cleanup) and D (fixes) land; per plan §5, **gates are hard-zeros only — every metric here is informational.**

## Environment

Fresh world (`bootstrap --reset`: 11×11 default map, user registered, Spawn1 placed programmatically @ W7N4 (25,30)), tick rate 100 ms (read-back verified), release deploy via `js_tools/deploy.js` (13 s warm build; **2.99 MiB of the 5 MiB code limit**), capture via `screeps-eval run --ticks 2000 --scenario baseline-0`.

## Headline numbers

| Metric | Value |
|---|---|
| Ticks observed | **2,009** (game tick 139 → 2,148) |
| Wall clock | 199.6 s (~99 ms/tick — the server **sustained** the configured 100 ms) |
| Creeps | 1 → **38** (max 38) |
| CPU used | avg **7.60** / max **55.01** (limit 100) |
| Bucket | min 10,000 / last 10,000 (never dipped) |
| Console | 186 log lines · 0 error events · **1 `(ERROR)` line** · **0 panics** · **0 deser failures** |

## Hard-zero gates (the only gates)

✅ deploy succeeded · ✅ ticks observed > 0 · ✅ zero panic lines · ✅ zero deserialization-failure lines

## Findings worth recording

1. **The one error line** (tick 200): `(ERROR) screeps_ibex::machine_tick: State machine 'UpgradeJob' exceeded 20 transitions in a single tick, breaking to prevent infinite loop` — the silent `MAX_STATE_TRANSITIONS=20` cap firing during *normal early-game economy*. This is live corroboration of review Field Report F / IBEX-006 (FSM friction; the cap's opacity), already owned by [ADR 0003](../design/0003-behavior-modeling.md). No action in Phase 0; expect this line to persist in BASELINE-1 (it is not a Phase-0 fix target) — its presence/absence is a useful FSM-rewrite signal later (Increment 6).
2. **CPU max 55** of 100 on a 1-room, ≤38-creep colony is high in relative terms — consistent with the review's CPU findings (un-amortized spikes; ADR 0004 territory, Increment 1, not Phase 0). The avg 7.6 is comfortable.
3. **Bucket never moved off 10,000** — no pressure at this scale, as expected; the death-spiral class is unobservable in a 1-room baseline (harness pressure scenarios come with Increment 0/1 per ADR 0006/0015).
4. Growth to 38 creeps in ~2,000 ticks with stable CPU is a healthy economic bring-up — a good reference curve for the BASELINE-1 comparison.

## BASELINE-1 comparison (to fill at phase end)

| Metric | BASELINE-0 | BASELINE-1 | Note |
|---|---|---|---|
| Ticks observed | 2,009 | — | |
| Creeps max | 38 | — | |
| CPU avg / max | 7.60 / 55.01 | — | |
| Panic / deser lines | 0 / 0 | 0 / 0 required | the hard gates |
| `(ERROR)` lines | 1 (FSM cap) | — | informational |
