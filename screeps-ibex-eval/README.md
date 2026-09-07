# screeps-ibex-eval

The evaluation **policy** for [screeps-ibex](..): what a correct/healthy
run of THIS bot looks like. All mechanism — Docker stack lifecycle,
world bootstrap, deploy, console/metrics capture — comes from the
generic [`screeps-server-kit`](../screeps-server-kit) (the P0.A14
mechanism/policy split; see [`docs/execution/phase-0.md`](../docs/execution/phase-0.md)
row P0.A14 and [ADR 0006](../docs/design/0006-eval-and-iteration-harness.md)).

## Usage

```
cd screeps-ibex-eval
cargo run -- smoke               # full loop, 600 ticks
cargo run -- smoke --ticks 2000  # baseline-length smoke
cargo run -- run --ticks 2000 --scenario baseline-1   # capture only
```

Setup, stack management (`server up`/`down`/`status`/`logs`), bootstrap,
deploy, the server-CLI REPL, and tick control are the kit's commands —
see [`screeps-server-kit`'s README](../screeps-server-kit/README.md).
Configuration is shared: credentials in the repo-root `.screeps.yaml`,
stack settings in `../screeps-server-kit/config/local.yml`. `smoke` and
`run` act as the kit's resolved identity — an explicit `--server-name`,
otherwise the **first `bots:` entry** (so with `bots: [ibex, ibex-2]`
the smoke deploys and captures bot `ibex`).

`smoke` is the one-command loop: **server up → bootstrap --reset →
deploy → run --ticks K → summary + gate verdict**, exiting nonzero only
on the **hard-zero gates** (phase-0.md §5 criterion 6):

1. deploy failure (the deploy step errors),
2. zero ticks observed (simulation not advancing),
3. any console line matching the panic marker,
4. any console line matching the deserialization-failure markers.

Every metric (CPU, creep counts, error-line counts) is printed but
**never gates** — single-run metric gates are the flake generator ADR
0015 rejects. Note `smoke` resets the world by design (`bootstrap
--reset` wipes all data including memory segments). Run artifacts land
in the repo-root `runs/` tree (gitignored), exactly as before the split.

## The H5 parity oracle (`src/parity.rs`, ADR 0006 §B.4)

The sim-vs-server conformance lane. The pure schema/replay/diff is
`screeps_combat_engine::parity` (its layer-1 gate is
`screeps-combat-engine/tests/conformance.rs`, ZERO tolerance); this
crate owns the catalog (`parity/<name>.json`, golden vectors WITHOUT
frames), the seeding/capture/report orchestration and the budget.

**Docker-free** (no stack needed):

```
cargo run -- parity list                     # catalog entries
cargo run -- parity synth --scenario <name>  # replay through the sim → tests/conformance/<name>.json
                                             # as a PLACEHOLDER (provenance says so)
cargo run -- parity check <vector.json>      # replay a vector / catalog entry and print the diff
```

**Layer 1 — scripted capture** (the warm private world; both owners in
the bed, `ibex` and `ibex-2`, must be `bots:` entries in
`config/local.yml`):

```
cargo run -- parity capture --scenario <name> [--room R] [--activate-rooms]
```

Seeds the entry's creeps/structures/towers into the (neutral) bed room
through the kit's `cmd_insert_*` builders, arms the bot's driver
(`screeps-ibex::eval_parity`: `Memory.parity_script` +
`Memory._features.eval.parity_script`) for both owners with one
absolute start tick, captures the per-tick `PV1 ` console lines
(creeps, labelled structures/towers, the absent/gone roster), builds
the frames, writes the vector with server provenance over the
placeholder, and prints the immediate sim-vs-capture verdict.
`--activate-rooms` flags frozen neutral rooms active and restarts the
stack (the flag is read at boot) — prefer batching that into the one
restart WS-CLOSE D8 already schedules. Nothing here wipes the world.

**Layer 2 — the unscripted bed** (report-only until
`parity/parity-budget.json` says `gating`; promotion is an operator
decision under ADR 0015's earned-promotion rule):

```
cargo run -- parity report --scenario <bed> [--run <dir>]   # one bed (or grade an existing run dir)
cargo run -- parity nightly                                 # every catalog bed
cargo test -p screeps-ibex-eval -- --ignored parity_nightly # the same, as the #[ignore] test lane
```

The live side is the driver in `trace` mode (frames only; the bot's own
systems decide); the sim side is `IbexAgent` vs `IbexAgent` seeded from
the live first-contact frame over the identical world; the diff from
contact is graded against the budget and written to
`runs/parity-<sha>-<stamp>/report-<bed>.json`.

`smoke` and `scenario` take `--keep-world` to skip `bootstrap --reset`;
the parity bed commands keep the world by default (`--reset-world` to
wipe first — WS-CLOSE D8 says don't).

## The gates (`src/gates.rs`)

The canonical ibex-specific strings, pinned by tests against the bot
crate's sources:

| Marker | Value | Source |
|---|---|---|
| panic | `panicked at` | the panic hook logs std `PanicHookInfo` Display via `log::error!` (`screeps-ibex/src/panic.rs`) |
| deser failure | `Failed deserialization:` · `Failed to decode stats history` | `game_loop.rs:556`, `stats_history.rs:200` (serialize-side errors deliberately do NOT gate) |
| error-line prefix | `(ERROR)` | the fern console format (`logging.rs:32`) |
| live-stats segment | 99 | `segments.rs` `LIVE_STATS_SEGMENT` (the seg-99 stats JSON the CPU summary reads) |

`gates::capture_spec()` packages these as the kit's `CaptureSpec`;
`smoke`/`run` pass it to `screeps_server_kit::capture::run`, and the
gate verdict is `summary.gate_failures(&spec.markers)`.

## Baselines

Baselines are fresh-bootstrap `run`s at the standard 100 ms tick rate
(`run --ticks 2000 --scenario baseline-N` after a reset + deploy; plan
D-3: 2 000 ticks reaches RCL2 + unreserved-remote activity). BASELINE-0
= master before Phase-0 changes; BASELINE-1 repeats it after the
Phase-0 fixes; the comparison feeds
`docs/execution/baseline-0-report.md`. Later ADR-0006 work — the
colony-health score, regression diffing — lands in this crate too: it
is ibex policy, not mechanism.

## Lifecycle

Workspace-excluded, host-native, same D-1 lifecycle as its dependency:
`screeps-server-kit` is a path dep while both live in-repo and becomes
a git dep when the kit extracts to its own remote. This crate itself
stays with the ibex repo — it is the part that is NOT a community-share
candidate, by definition.
