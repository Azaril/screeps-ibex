# Phase 0 Execution Plan — Baseline Tooling, Cleanup & Critical Fixes

- **Status:** Active (drives implementation)
- **Date:** 2026-06-09
- **Precedes:** ADR-driven rewrite increments ([rewrite plan](../plans/rewrite-plan.md) Inc 0–9). Phase 0 ≈ a concrete slice of Increment 0 (harness + test substrate) + the safe Increment-1 quick-wins, executed **before** any ADR pillar work begins.
- **Related:** [ADR 0006](../design/0006-eval-and-iteration-harness.md) (harness design — authoritative for components/endpoints), [ADR 0015](../design/0015-testing-and-validation-strategy.md) (test taxonomy; Phase 0 stands up the L0 lane), [component-test-plans](../plans/component-test-plans.md) (F1–F19 fixture registry), [proposed-fixes](../plans/proposed-fixes.md) (Group A — the fix specs live there; this plan sequences them), [ADR 0008](../design/0008-combat-and-squad-architecture.md) (the verified dead-code inventory).

> **Purpose.** Establish a baseline of *capability* (run/deploy/observe the bot on a local private server with one command), *measurement* (a recorded baseline run + the first pinned kernel tests), and *hygiene* (supplanted code removed, the obviously-incorrect bugs fixed) — so every subsequent change is made against a measurable, reproducible baseline. **No ADR pillar implementation happens in Phase 0.**

---

## 0. Ground rules

- **Read-only for design:** ADRs are not re-decided here. Where a Phase-0 task touches an ADR's territory, the ADR governs and is cited.
- **Breaking changes:** Phase 0 contains exactly **one** sanctioned Memory/format break (P0.C1 — orphan mission-variant removal), absorbed by the next eval-server `bootstrap --reset`. Everything else is None/Behavioral. **MMO guard:** no MMO deploy between C1 landing and an explicitly accepted MMO state-drop window — on MMO, C1's discriminant shift would decode the whole mission/operation world to empty.
- **Secrets:** `.screeps.yaml` is already gitignored (`.gitignore:50`) with a tracked `.example-screeps.yaml` template. Phase 0 extends that discipline to the harness (§A7) — credentials never appear in logs, run artifacts, or committed files.
- **Verification baseline:** every fix lands with its pin test (§B3); the phase ends with a recorded before/after baseline comparison (§5).

---

## 1. Workstream A — `screeps-eval`: private-server execution & deployment harness

A Rust crate providing a **library** (consumed later by integration tests / `screeps-testkit`) and a **CLI** (`cargo run -- <cmd>`). **The CLI serves two users with equal priority: the automation harness AND the operator's manual iterative testing** — every automation primitive (server lifecycle, bootstrap, deploy, tick control) is exposed as a human-friendly command, so speeding up manual iteration is a first-class deliverable, not a by-product (P0.A8). Implements ADR 0006's components 1–7 in their Phase-0 form.

**Lifecycle decision (D-1, RESOLVED):** start **in-repo** (workspace-excluded), **extract to a submodule with its own remote once the crate stabilizes**. Design for that extraction from day one: self-contained README + example config, no dependence on workspace-internal crates or repo-relative paths beyond the documented config-discovery rule, independent dependency tree. The operator creates the remote at extraction time.

**Stack (standard, well-regarded crates):** `tokio` (async runtime), `bollard` (Docker API — connects to Docker Desktop via named pipe on Windows, `Docker::connect_with_local_defaults()`), `reqwest` (rustls; HTTP API), `tokio-tungstenite` (console websocket), `clap` (derive CLI), `serde`/`serde_yaml`/`serde_json`, `anyhow`/`thiserror`, `tracing` + `tracing-subscriber`, `secrecy` (secret wrapping, §A7). The Rust `screeps-api` crate is **evaluated** in P0.A4 (auth + console stream); if unmaintained/awkward, the thin `reqwest` client per ADR 0006's endpoint list is the fallback — the task is written to be resilient to either outcome.

| ID | Task | Detail | Done when |
|---|---|---|---|
| **P0.A1** | Crate scaffold | `screeps-eval/` at repo root, **workspace-excluded** (add to root `Cargo.toml` `exclude`, the `screeps-foreman-bench` precedent). Own **`screeps-eval/.cargo/config.toml`** pinning the host triple (the parent workspace config sets `wasm32-unknown-unknown`, and config discovery walks up from CWD — without an override, `cd screeps-eval && cargo run` would cross-compile; foreman-bench survives only via explicit `--target`). `src/lib.rs` (library: config, docker, server, api, capture modules) + `src/main.rs` (clap CLI). Depends on **no workspace wasm crate**. | `cd screeps-eval && cargo run -- --help` works host-native with no `--target` flag |
| **P0.A2** | Server lifecycle (bollard) | Manage the launcher stack **natively via bollard** (no compose dependency): create network + `mongo:8` + `redis:7` + `screepers/screeps-launcher` containers (volumes per `C:\code\screeps-launcher\docker-compose.yml`, which stays the reference). Bind-mount a **vendored config template** (`screeps-eval/server/config.yml`, committed) — **WARNING: the reference `C:\code\screeps-launcher\config.yml` begins with a live `steamKey` secret; do NOT copy it verbatim.** The template ships with **no steamKey**; the harness injects it as container env (`STEAM_KEY`, sourced from `.screeps.yaml`/`SCREEPS_EVAL_STEAM_KEY` through A7's `secrecy` wrapper), and A7's sweep explicitly covers the template diff. The template must also set the launcher **CLI bind host to `0.0.0.0`** (verify the exact key name against screeps-launcher docs — the CLI binds in-container localhost by default, so publishing the port alone yields connection-refused); publish **21025 (game/API)** and **21026 (server CLI — the screeps-launcher default; moderate-high confidence, not verifiable from this repo)**. Documented fallback if the bind fights us: `docker exec <launcher> screeps-launcher cli`. Health-wait on `http://127.0.0.1:21025/` with a **generous first-boot timeout (10 min+)** — the launcher runs an in-container npm install on first start. Commands: `server up\|down\|destroy\|status\|logs`. | `server up` from a cold Docker Desktop reaches healthy; the **host-side CLI connection succeeds**; `server destroy && server up` is repeatable |
| **P0.A3** | World bootstrap | Drive the **server CLI (21026)**: `system.resetAllData()`, `setPassword(user, pass)` (from config — §A7), `system.setTickDuration(ms)` (config; fast eval default), spawn placement (config coords or first-valid auto-pick). Command: `bootstrap [--reset]`. Opponent bots: **out of Phase 0** (harness scenario work, Inc 0 proper). | `bootstrap --reset` yields a running world with our spawn placed and creds matching `.screeps.yaml` |
| **P0.A4** | Deploy | Interim: shell out to `node js_tools/deploy.js --server private-server` (the working path; sanctioned as interim by ADR 0006). Then native: `POST /api/user/code` with token from `/api/auth/signin` (same endpoint deploy.js hits). Evaluate the `screeps-api` crate here. Command: `deploy [--debug]`. | `deploy` pushes the current build and the server console shows the new code tick over |
| **P0.A5** | Data capture | Console websocket subscription → `runs/<scenario>-<git-sha>-<stamp>/console.jsonl` (structured; severity-tagged lines pass through; **directory keyed by (scenario, git SHA) per the F14 fixture convention** so baselines and the later differ share one scheme); memory/segment reads via HTTP (`/api/user/memory-segment`) — **seg 99** (live stats) now, **seg 57** when ADR 0006's metrics segment lands → `metrics.jsonl`. Command: `run --ticks N` (collect for N ticks, then summarize `summary.json`: scenario, git SHA, ticks observed, CPU stats from seg-99, creep counts, errors seen). Add **`runs/` to `.gitignore`**. | `run --ticks 200` produces console+metrics+summary artifacts with zero manual steps |
| **P0.A6** | Smoke loop + baselines | `smoke` = `server up` → `bootstrap --reset` → `deploy` → `run --ticks K` → summary + nonzero-exit on: deploy failure, zero ticks observed, or console panic/deser-failure lines. Record **BASELINE-0** (current `master` code, pre-Phase-0 changes) and later **BASELINE-1** (§5). | `smoke` is one command, green, repeatable; BASELINE-0 artifact exists |
| **P0.A7** | Secrets policy (enforced, not aspirational) | (a) Config loading: `.screeps.yaml` + env overrides (`SCREEPS_EVAL_*`); password/token/steamKey held in `secrecy::SecretString` — `Debug`/`Display` redact by construction. (b) **No secret ever enters `runs/` artifacts or tracing output** — redaction at the config boundary, not at call sites. (c) **The CLI send path is a named leak risk**: A3's composed `setPassword("<user>", "<pass>")` payload exists post-`expose_secret()` — never log raw CLI payloads; mask `setPassword` args in any command echo/transcript; the manual sweep explicitly covers the CLI send path (the deploy.js subprocess is verified clean — it reads `.screeps.yaml` itself; no credentials on argv/env). (d) `.example-screeps.yaml` updated if the harness adds keys (e.g. steamKey); A2's vendored template diff is part of the sweep. (e) A **pin test** asserts `format!("{:?}", config)` contains no secret material (known limit: it cannot catch the CLI path — hence (c)). (f) `runs/` gitignored (A5). | The pin test exists and passes; manual sweep of a smoke run's artifacts **and the CLI transcript** finds no credential |

| **P0.A8** | Operator mode (manual iteration) | First-class commands for the operator's hands-on loop, beyond what automation needs: `server up\|down\|destroy\|status\|logs` (A2) with human-readable `status` (container health, tick rate, API reachability); **`cli`** — interactive passthrough/REPL to the server CLI (send arbitrary commands, e.g. `system.pauseSimulation()`), with `setPassword`-class payload masking per A7(c); **`tick set <ms>\|pause\|resume`** — tick-rate control without remembering CLI incantations (floor: see D-2); **`open`** — print/launch the web-client URL (`http://127.0.0.1:21025/`). All built on the same library functions the harness uses — no parallel code path. | The operator can cold-start, watch, pause, retune, and tear down the server with single commands and zero references to docs |

**Decision point D-1 — RESOLVED (operator):** in-repo, workspace-excluded now; **extract to a submodule with its own remote once stable** (see the lifecycle decision in the workstream intro). The operator creates the remote when extraction time comes.

---

## 2. Workstream B — host test lane + baseline pin tests

Stands up ADR 0015's **L0 lane** in its minimal Phase-0 form. (Full Tranche-1 ~60 kernel tests are Increment-0 work and may proceed in parallel; Phase 0 requires only the pins below.)

| ID | Task | Detail | Done when |
|---|---|---|---|
| **P0.B1** | `test-host` lane | Root `.cargo/config.toml` gains the alias (per ADR 0015 §4): `test-host = ["test", "--target", "x86_64-pc-windows-msvc"]` (CI later passes its own triple explicitly). Verified working on this machine by the testing pass (`cargo test -p screeps-ibex --target x86_64-pc-windows-msvc --no-run` succeeds). | `cargo test-host -p screeps-ibex` compiles & runs (zero tests initially) |
| **P0.B2** | `screeps-testkit` | **Deferred unless needed** — Phase-0 pins are plain in-crate `#[cfg(test)]` against pure functions; create the testkit crate only when the first fixture double (MemoryArbiter/GameView) is actually required. Do not scaffold speculatively. | n/a (explicit non-goal unless triggered) |
| **P0.B3** | Baseline pin tests (~12) | In-crate tests pinning (i) **known-correct behavior Phase 0 must not break** and (ii) **each Workstream-D fix**: spawn-queue **descending-priority invariant** (the review's false-positive guard — lock it so it is never "fixed" into an inversion; `spawnsystem.rs:85–94/236`), `create_body` min-cost clamp (IBEX-022 refuted — lock the clamp), serialize encode/decode round-trip (pure helpers, `serialize.rs:310–344`), `RepairQueue` `max_hits=0` guard, segment-map disjointness (**the D1 `const` assert IS the regression test** — per component-test-plans §1(c); no runtime twin), capacity helper boundary cases (D-IBEX-050), `saturating_sub` sites (D-IBEX-044/045), transfer `(TransferTarget, mode)` matrix returns `Err` not panic (D-IBEX-010), power-bank counter counts only PowerBank attacks (D-IBEX-043), config-redaction pin (A7), threat-classify smoke (current behavior snapshot, informational). | `cargo test-host` green with the pin set; each D-row below names its pin |

---

## 3. Workstream C — dead-code removal (supplanted only)

**The rule (operator-stated):** remove only what is dead because **supplanted**; keep what is dead because **not yet implemented**. Concretely: a construct is removable iff (a) **zero call sites** (verified by crate-wide search at removal time, not from memory) AND (b) an ADR marks it **superseded**. Everything else stays.

| ID | Task | Detail | Breaking |
|---|---|---|---|
| **P0.C1** | Remove orphaned squad missions | `SquadAssaultMission` (`missions/squad_assault.rs`, 592 L) + `SquadHarassMission` (`missions/squad_harass.rs`, 357 L) + their `MissionData::SquadAssault`/`SquadHarass` variants (`missions/data.rs:33–34`, match arms `:66–67`/`:104–105`, `mission_type!` registrations `:260–261`) + `missions/mod.rs:22/:24` mod decls. Zero `::build` call sites (re-verified against the working tree by the plan review); no per-variant serialization registration exists (`game_loop.rs` serializes `MissionData` storage whole), so data.rs + mod.rs is the complete edit — non-exhaustive-match errors make the arms unforgettable. Superseded by `AttackMission` force plans (ADR 0008 DELETE rows; ~950 L gone). **Variant removal shifts positional-bincode discriminants (7 later variants) → Memory/format**: lands **after BASELINE-0 is recorded**; the break is absorbed by the next `bootstrap --reset` (the BASELINE-1 bring-up). See the §0 MMO guard. | **Memory/format** (absorbed by the BASELINE-1 reset; MMO guard applies) |
| **P0.C2** | Bounded supplanted-sweep | One pass applying the rule above to: remaining commented-out code blocks (keep those documenting intent, e.g. `raid.rs`'s deliberately-disabled registrations), unused imports/`#[allow(dead_code)]` items whose owner was deleted by C1, and `todo.md` grooming (check off the refuted IBEX-022 body-sizing item and the DONE-marked planner items). Anything ambiguous → **keep + note**, do not expand scope. | None |

**Explicit KEEP list** (dead-but-planned; each has an ADR owner — do NOT remove):
`BoostQueue` + `Boosted*` compositions ([0010](../design/0010-boost-lab-factory-pipeline.md) wires them) · `total_energy_invested` economy-abort ([0008](../design/0008-combat-and-squad-architecture.md)/[0014](../design/0014-empire-strategy-and-posture.md) wire it) · rover `Follow`/`desired_offset` ([0003](../design/0003-behavior-modeling.md) §B.2 builds on it) · `duo_sk_farmer` (gap G-8 SK farming) · power-bank attack machinery ([0013](../design/0013-power-economy-and-power-creeps.md)) · `TransferTarget::Nuker` deposit plumbing (gap G-4 nukes) · `repair_entity_integrity` (deleted only at Increment 5, [0001](../design/0001-entity-model.md) A3) · `DefendMission`'s observer role (KEEP per 0008 inventory).

---

## 4. Workstream D — critical & obviously-incorrect fixes

Exactly the **Group A** set from [proposed-fixes.md](../plans/proposed-fixes.md) (the fix specs — before/after sketches, exact lines — live there; this table sequences and binds each to its pin test). Group B (needs-design: IBEX-018/021/040/048/049) is **explicitly out of Phase 0** — those ride their ADR increments.

| Order | Fix | One-liner | Severity | Pin test (B3) |
|---|---|---|---|---|
| D1 | **IBEX-013 (interim)** | `const` disjointness assert (`COMPONENT_SEGMENTS` ∌ `COST_MATRIX_SEGMENT`) + shrink `COMPONENT_SEGMENTS` to 50–54 + chunk-count watermark log — stops `serialize_world` wiping the cost-matrix segment **every tick** | **Critical** | the `const` assert (compile-time = the regression test); validation run: **re-deploy identical code** (a global/heap reset that *preserves* segments — `bootstrap --reset` wipes them, so it cannot be the probe) then assert `load_cost_matrix_cache` non-empty. This is the embryo of component-test-plans §1's single-owner `forced_reset_reloads_nonempty_cost_matrix` artifact |
| D2 | **IBEX-010** | Nuker-withdraw `panic!` → `Err(InvalidArgs)` + one-shot log (reachable whole-tick abort during raids) | **Critical-path panic** | `(TransferTarget, mode)` matrix test |
| D3 | **IBEX-043** | Power-bank concurrency `.filter(\|_\| true)` no-op → count only `AttackReason::PowerBank` | High-value one-liner | counter test |
| D4 | **IBEX-029 — DEFERRED to Increment 1** | Routing the 23 bare `squad_combat` intents through `action_flags` is behavior-preserving *only if* the pipeline pairing is exactly right — the verified site list has same-state adjacent pairs (`:248/:250`, `:478/:480`, `:693/:695`) where same-pipeline gating changes *which* call wins, and the only honest validation is the **intent differ** (shadow-dispatch), which lands Inc 1 per ADR 0015. Pipeline authority is **`jobs/actions.rs:22–42`** (`RANGED_HEAL` shares pipeline B with `RANGED_ATTACK`/`RANGED_MASS_ATTACK`; `HEAL` is its own pipeline C) — NOT the proposed-fixes pairing note, whose refinement flag is mandatory. | (rides Inc 1 with the differ) |
| D5 | **IBEX-044/045** | `saturating_sub` on `game::time()-t` cadence/timeout sites + `store.rs` free-capacity | Latent underflow→abort | boundary tests |
| D6 | **IBEX-050** | Extract single `creep_used/free_capacity` helper (6+ copies) — **fold IBEX-045's store change into it** (per proposed-fixes note) | Tech-debt w/ correctness edge | helper boundary test |
| D7 | **IBEX-009/019/020** | Latent unwrap/expect hardening: staticmine `if let`, attack.rs:615 explicit bind, `get_room()` sentinel | Latent (all currently guarded) | compile + existing-behavior smoke |
| D8 | **IBEX-046** | `debug_assert!(is_finite)` at priority sources + guard transfer divisors | Latent NaN | finiteness asserts under `test-host` |

Each fix: implement → pin test green (`cargo test-host`) → `cargo clippy -p screeps-ibex` clean (wasm target). **Smoke runs twice, not per-fix** — once after D1 (the Critical) and once after the final fix — because a full smoke is a nightly-LTO wasm build + a server run (tens of minutes at realistic tick rates), and per-fix smokes are exactly the per-change drag ADR 0015 §3 forbids. Fixes land as individual commits with their tests.

---

## 5. Sequencing, baselines & exit criteria

```
A1 ──→ A2 ──→ A3 ──→ A4 ──→ A5 ──→ A6 ─→ BASELINE-0 (current master, fresh world)
 B1 (parallel) ──→ B3 pins for pre-existing invariants
                                    then: C1 → C2  (cleanup; world reset absorbed)
                                          D1 → D8  (each with its pin test)
                                    A6 again ─→ BASELINE-1 (Phase-0 code, fresh world)
                                    compare summaries → docs/execution/baseline-0-report.md
```

Both baselines are **fresh-bootstrap runs of K ticks at the same tick duration**, so the comparison is apples-to-apples; the comparison report (committed) is the first artifact of the regression-diffing discipline ADR 0006 §7 builds out.

**Exit criteria (all must hold):**
1. `cd screeps-eval && cargo run -- smoke` is **one green command** from a cold Docker Desktop.
2. `cargo test-host` green, including the spawn-ordering and redaction pins.
3. Secrets: redaction pin passes; a manual sweep of `runs/` artifacts and harness logs finds no credential material; `.gitignore` covers `runs/`.
4. Supplanted code removed (C1/C2); KEEP list intact and documented above.
5. All eight D-rows landed, each with its pin test, clippy-clean on the wasm target.
6. BASELINE-0 and BASELINE-1 recorded; comparison report committed. **Gate only on hard zeros** (zero panics, zero deser failures, deploy succeeded, ticks observed > 0) — the metric comparison (CPU, creep counts) is **informational** in `baseline-0-report.md`, not a gate: a single-run exact metric gate is precisely the flake generator ADR 0015 rejects, and no N-seed machinery exists yet.
7. The one Memory/format break (C1) was absorbed by the sanctioned BASELINE-1 bring-up reset — **no unsanctioned drop of live/MMO state occurred** (eval-server `bootstrap --reset` is a state drop by design and doesn't count; the §0 MMO guard held).
8. **Operator mode works end-to-end (P0.A8):** cold-start → watch → `tick set` → pause/resume → tear down, each a single command, verified by the operator personally.

**Out of scope (deferred to their increments):** CpuGovernor/pathfinding facade (Inc 1), serialization version header + dedicated cost-matrix segment (Inc 2 — D1 is the *interim* only), all Group B fixes, scenario library/opponent bots, `screeps-testkit` fixtures beyond need, any ADR pillar implementation.

**Operator decision points:** **D-1 — RESOLVED:** in-repo now, extract to a submodule once stable (§1). **D-2 — RESOLVED (operator):** tick-rate **floor is 50 ms** — at/below that the server and UI start failing to keep up (operator experience). Defaults: smoke runs **100 ms** (safely above the floor), manual watching **1000 ms**; `tick set` (P0.A8) accepts anything ≥ 50 ms, warns below 100 ms. Wall-clock expectations still per the honesty note: the server may not sustain the configured rate once creeps exist, and first boot includes a multi-minute in-container npm install (A2's health-wait tolerates it). **D-3 (default unless vetoed):** K = 2,000 ticks for baselines — reaches **RCL2 + unreserved remote activity** on a fresh world ("first remotes" with reservers is *not* reachable at RCL2, whose 550 max spawn energy is below a CLAIM+MOVE reserver's 650 cost).
