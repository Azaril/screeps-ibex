# screeps-pack ⇄ deploy.js parity evidence (P0.A13 cutover gate)

- **Date:** 2026-06-10 · **Commit:** `d7d0d7a` (working tree) · **Host:** Windows 11, x86_64-msvc
- **Pipelines compared:** `node js_tools/deploy.js` (wasm-pack 0.14.0, node v20.20.0, rollup/babel, terser off) vs `screeps-pack` (this crate)
- **Gate (phase-0.md P0.A13):** module-map diff + wasm byte-compare (versions pinned equal) + a green 600-tick smoke on a screeps-pack-deployed bot. **Verdict: ALL GREEN — cutover executed.**

## 1. Toolchain version equivalence

| Component | deploy.js pipeline | screeps-pack | Equal |
|---|---|---|---|
| cargo invocation | wasm-pack 0.14: `cargo build --lib --release --target wasm32-unknown-unknown` + `configs.wasm-pack-options` | identical argv (pinned by `cargo_build::tests::release_argv_passes_flags_through_verbatim`) | ✔ |
| wasm-bindgen | 0.2.108 (wasm-pack resolves from `Cargo.lock`) | 0.2.108 (same lockfile resolution; prebuilt CLI `wasm-bindgen-0.2.108-x86_64-pc-windows-msvc.tar.gz`) | ✔ |
| binaryen / wasm-opt | version 116 (wasm-pack found `~/.cargo/bin/wasm-opt.exe` = 116; wasm-pack 0.14's bundled `wasm-opt` crate is also 0.116) | version 116 (prebuilt `binaryen-version_116-x86_64-windows.tar.gz`, pinned `opt::DEFAULT_BINARYEN_VERSION`) | ✔ |
| wasm-opt args | `[package.metadata.wasm-pack.profile.*] wasm-opt` | same tables, read from the same manifest | ✔ |

Pre-condition proven before building the tool: `wasm-bindgen --target web`
and `--target nodejs` emit **byte-identical** `_bg.wasm` (3,777,120 B on
this commit), so the JS-target switch cannot perturb the wasm; and
binaryen-116 `wasm-opt -O4 --signext-lowering` over that bindgen output
reproduced wasm-pack's release artifact **byte-for-byte** before any
screeps-pack code existed.

## 2. wasm byte-compare (same input, both modes)

| Mode | deploy.js artifact (`dist/screeps_ibex.wasm`) | screeps-pack artifact (`…/dist/private-server/<mode>/screeps_ibex_bg.wasm`) | Result |
|---|---|---|---|
| release | 2,278,952 B, sha256 `c6df2aa2…0855784e` | 2,278,952 B, sha256 `c6df2aa2…0855784e` | **byte-identical** |
| debug | 8,430,156 B, sha256 `4f53b961…7b02e2cb` | 8,430,156 B, sha256 `4f53b961…7b02e2cb` | **byte-identical** |

Builds were interleaved in both orders (deploy.js → pack → deploy.js →
pack) with unchanged hashes — no `dist/`/`pkg/` cross-contamination in
either direction (screeps-pack writes only under
`<target>/screeps-pack/`).

## 3. Module-map diff (release)

| | deploy.js | screeps-pack |
|---|---|---|
| `main` | 60,679 B — rollup bundle (loader + inlined bindgen glue + polyfill, babel-down-leveled) | 3,895 B — the loader only |
| `screeps_ibex` | `{binary}` wasm (base64 3,038,604 B) | 65,247 B — patched bindgen glue + polyfill (own CJS module) |
| `screeps_ibex_bg` | — | `{binary}` wasm (base64 3,038,604 B) |
| total vs 5 MiB | 2.96 MiB (59.11%) | 2.96 MiB (59.28%) |

Exactly the expected delta (investigation §3): 2 modules → 3, bundled
JS → loader + glue, binary payload identical. **Size delta from
dropping the bundler: +8,463 B of JS (+13.9% JS, +0.26% of the
upload).** JS semantics verified equivalent: both `main` modules carry
the bucket gate (`BUCKET_BOOT_THRESHOLD` 1500), the staged
bytes→compile→instantiate boot, the `console.error` shim, the
`loading complete` line, and the wasm-bindgen#3130 `running` +
`Game.cpu.halt()` trap (6/6 behavior markers each; the loader template
is a line-faithful port of `js_src/main.js` with `initSync({module})`
replaced by the glue's deferred `__instantiate(module)`).

## 4. Live smoke on screeps-pack-deployed code (600 ticks, hard zeros)

Fresh world each time (`bootstrap --reset`, identity `private-server`/
user `ibex`, spawn W7N4, tick 100 ms).

**Run A — standalone CLI deploy** (`screeps-pack deploy --server
private-server`, upload ok at 05:59:50; artifacts
`runs/pack-parity-d7d0d7a-20260610-060003`):
ticks 357→967, **610 observed** · **0 panics** · **0 deser failures** ·
0 error-level lines · creeps 1→10 · cpu avg 1.52 / max 2.86 · bucket
pegged 10000. Bot fully alive (claim/mining operations logging).

**Run B — post-cutover library path** (`screeps-ibex-eval smoke --ticks
600`; deploy = server-kit → screeps-pack as a library, 13 s warm;
artifacts `runs/smoke-d7d0d7a-20260610-060345`):
**`smoke: PASS (all hard-zero gates green)`** — 610 ticks observed ·
0 panics · 0 deser failures · 1 non-gating error-level line (the bot's
own `UpgradeJob exceeded 20 transitions` state-machine guard —
pre-existing bot behavior, unrelated to deployment) · creeps 1→6 ·
deserialize/spawn/operations all running from the first captured tick.

## 5. MMO guard

No token-auth server was deployed to at any point. The `mmo` entry was
exercised by `screeps-pack check --server mmo` only (config resolution:
token auth + `--features mmo` flag concatenation — no build, no
upload).

## Conclusion

Identical wasm bytes in both build modes, the predicted 3-module map,
equivalent loader semantics, and two green 600-tick runs (standalone
CLI + library seam). The deploy.js shell-out was deleted from
`screeps-server-kit/src/deploy.rs` at this gate; `js_tools/deploy.js`
remains in-repo only as an optional user-customization escape hatch.
