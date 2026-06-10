# screeps-eval

Private-server execution, deployment, and evaluation harness for
[screeps-ibex](..) — a **library** (driven by tests and automation) plus a
**CLI** for the operator's manual iteration loop.

Task map and acceptance criteria: [`docs/execution/phase-0.md`](../docs/execution/phase-0.md)
(Workstream A, P0.A1–A8). Design: [ADR 0006](../docs/design/0006-eval-and-iteration-harness.md).

## Status

| Capability | Task | Status |
|---|---|---|
| Crate scaffold, config + secrets policy, CLI surface | P0.A1 | ✅ |
| Server lifecycle (bollard ↔ Docker Desktop) | P0.A2 | ⬜ |
| World bootstrap (server CLI) | P0.A3 | ⬜ |
| Deploy (deploy.js interim → native API) | P0.A4 | ⬜ |
| Console/metrics capture → `runs/` | P0.A5 | ⬜ |
| Smoke loop + baselines | P0.A6 | ⬜ |
| Secrets sweep & pins | P0.A7 | ✅ (pin) / ⬜ (sweep) |
| Operator mode (cli/tick/open/status) | P0.A8 | ⬜ |

## Usage

This crate is **workspace-excluded** and **host-native**. Always invoke
from inside this directory (cargo config discovery is CWD-based; from the
repo root you'd inherit the wasm32 default target):

```
cd screeps-eval
cargo run -- --help
cargo run -- config         # resolved config, secrets redacted
cargo run -- server up      # (P0.A2) start launcher+mongo+redis
cargo run -- bootstrap --reset
cargo run -- deploy
cargo run -- run --ticks 200
cargo run -- smoke
cargo run -- tick set 100   # floor: 50 ms (operator-established)
cargo test                  # includes the secrets-redaction pin
```

## Configuration & secrets

Reads the repo's `.screeps.yaml` (gitignored; template:
`.example-screeps.yaml`) — the same unified config the deploy tooling
uses — selecting the `private-server` entry by default. Overrides:
`SCREEPS_EVAL_HOST`, `SCREEPS_EVAL_PORT`, `SCREEPS_EVAL_STEAM_KEY`.

**Secrets policy (P0.A7):** credentials are `secrecy::SecretString` from
the moment of parsing — `Debug` output redacts by construction, enforced
by a pin test (`config::tests::debug_output_redacts_secrets`). The Steam
key is injected into the launcher container as env, never written to the
vendored server config template. Never log raw server-CLI payloads
(`setPassword(...)`).

## Lifecycle

Starts **in-repo** (workspace-excluded). Extracts to a submodule with its
own remote once the crate stabilizes (Phase 0 decision D-1) — hence: no
workspace-crate dependencies, no repo-relative path assumptions beyond
the `.screeps.yaml` walk-up discovery documented above.
