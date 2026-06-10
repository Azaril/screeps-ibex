# Rust-Native Deploy Investigation (P0.A11)

- **Status:** Investigation complete — **verdict: GO** (§6)
- **Date:** 2026-06-09
- **Task:** [phase-0.md](../execution/phase-0.md) P0.A11 (operator directive; community-share candidate)
- **Constraint:** `js_tools/deploy.js` remains the working deploy path until a prototype proves parity (§5). This doc changes no code.
- **Reference clones (sanctioned, at `C:\code`):** `cargo-screeps` (rustyscreeps), `trunk` (trunk-rs); `screeps-engine`, `screeps-driver`, `screeps-game-api`, `screeps-launcher` were already present.

---

## 1. Current-pipeline anatomy

The whole pipeline is `npm run deploy -- --server <name> [--mode debug|release] [--dryrun]` → `js_tools/deploy.js`. Steps, with citations:

1. **Config** — parses `.screeps.yaml` ([SS3 unified credentials format](https://github.com/screepers/screepers-standards/blob/master/SS3-Unified_Credentials_File.md)): the `servers.<name>` entry (host/port/secure/token *or* username+password, `branch`, default `default`) and two `configs` sections: `terser` (per-server minify toggle, default off) and `wasm-pack-options` (per-server extra args, `'*'` concatenated with server-specific) — `deploy.js:37–74`, `.example-screeps.yaml:47–73`.
2. **Clean** — empties `dist/` and `pkg/` (`deploy.js:77–81`).
3. **wasm build** — `spawnSync('wasm-pack', ['build', '--target', 'web', --dev|--release, 'screeps-ibex', '--out-dir', '../pkg', ...extra_options])` (`deploy.js:84–88`). The extra options that **must carry over** to any replacement (`.example-screeps.yaml:62`):
   - `--config build.rustflags=['-Ctarget-cpu=mvp']` — clamp to MVP wasm feature set (Screeps servers' V8 rejects post-MVP opcodes; see [screeps-game-api#391](https://github.com/rustyscreeps/screeps-game-api/issues/391))
   - `-Z build-std=std,panic_abort` — rebuild std with those rustflags; **requires nightly** (`rust-toolchain.toml` pins `channel = "nightly"`) + the `rust-src` component
   - per-server feature flags: `mmo`/`ptr` add `--features mmo` (`.example-screeps.yaml:69–70`)
   - wasm-pack internally: reads `Cargo.lock` for the `wasm-bindgen` version (workspace dep is `wasm-bindgen = "0.2"`, `screeps-ibex/Cargo.toml:17`), downloads the **matching** `wasm-bindgen-cli` prebuilt, runs it with `--target web`, then downloads binaryen and runs `wasm-opt` with the args from `[package.metadata.wasm-pack.profile.*]` (`screeps-ibex/Cargo.toml:46–59`): dev = `--signext-lowering`, release = `-O4 --signext-lowering`. (Under `-Ctarget-cpu=mvp` the signext-lowering pass is redundant but harmless — kept for parity.)
   - Output: `pkg/screeps_ibex.js` (ESM wasm-bindgen loader, exports `initSync`, `setup`, `game_loop`) + `pkg/screeps_ibex_bg.wasm`.
4. **rollup bundle** (`deploy.js:91–118`) — input `js_src/main.js`, output `dist/main.js`, `format: 'cjs'`. What rollup actually does here:
   - **ESM→CJS conversion + bundling**: inlines `pkg/screeps_ibex.js` (ESM) and the `fastestsmallesttextencoderdecoder-encodeinto` polyfill package (the only runtime npm dep, `package.json:14–16`; imported at `js_src/main.js:2`) into one CommonJS file. *This is the only structurally necessary transform, and it exists solely because `--target web` output is ESM.*
   - **babel `preset-env` targeting node 12** — down-levels syntax for the servers' older V8. Empirically near-unnecessary: cargo-screeps ships un-transpiled bindgen output to the same runtime (§2a).
   - **terser** — optional minify, off by default (`.example-screeps.yaml:52`).
   - **copy/rename**: `pkg/screeps_ibex_bg.wasm` → `dist/screeps_ibex.wasm` (`deploy.js:104–110`).
5. **Module map** (`deploy.js:121–146`) — every `dist/` file becomes a module keyed by stripped filename: `.js` files as **plain strings**, `.wasm` files as **`{ binary: <base64> }`**. The exact uploaded map today:
   ```json
   { "main": "<bundled JS string>", "screeps_ibex": { "binary": "<base64 wasm>" } }
   ```
   plus a 5 MiB size check.
6. **Upload** (`deploy.js:150–160`) — `ScreepsAPI.fromConfig(server)` then `api.code.set(branch, modules)` = **`POST {prefix}/api/user/code`** with body `{ branch, modules }`; auth via `X-Token` (official) or signin-derived token (private servers). Confirmed identically by cargo-screeps (`src/run.rs:81–82`, `src/upload.rs:98–109`) and the python reference ([screeps_api.py](https://raw.githubusercontent.com/Qionglu735/screeps_tool/master/screeps_api.py) `post_code` → `/api/user/code`, `X-Token` header).

**What the JS glue does at runtime** (`js_src/main.js`, 88 lines — this is the part a Rust tool must template):
- TextEncoder/TextDecoder polyfill import (line 2) — the isolate has no `util` and no global TextDecoder.
- Bucket-gated boot (`Game.cpu.bucket < 1500` → defer, line 67) with **staged, multi-tick loading**: tick A `require("screeps_ibex")` (bytes), then `new WebAssembly.Module(bytes)`, then `bot.initSync({ module })` (lines 73–75); frees the bytes + require cache afterwards.
- `console.error` shim per tick → log + `Game.notify` (lines 15–34).
- The **`running` flag + `Game.cpu.halt()` trap**: if the previous tick's `game_loop()` never returned (error/CPU-kill), halt the IVM for a fresh environment — the [wasm-bindgen#3130](https://github.com/rustwasm/wasm-bindgen/issues/3130) workaround (lines 30–56).
- Calls `bot.setup()` once and `bot.game_loop()` per tick (exports defined at `screeps-ibex/src/lib.rs:44–51`).

**Server-side ground truth** (why this works — `C:\code\screeps-engine`, `C:\code\screeps-driver`):
- The runtime is **CommonJS-only**, evaluated in **isolated-vm** (`screeps-driver/lib/runtime/runtime-driver.js:21`, `package.json` dep `isolated-vm`). No ES modules, no `util`, no TextDecoder in the isolate. Lodash 3 is injected as global `_` (main.js relies on it).
- **Binary modules**: `require(name)` of a `{binary}` module returns a **Buffer of the decoded bytes** — `screeps-engine/src/game/game.js:561–563`:
  ```js
  if(_.isObject(this.codeModules[moduleName]) && this.codeModules[moduleName].binary !== undefined) {
      this.globals.require.cache[moduleName] = driver.bufferFromBase64(this.codeModules[moduleName].binary);
  }
  ```
  This is the API contract `{ binary: base64 }` rides on. (deploy.js uploads padded base64, cargo-screeps `STANDARD_NO_PAD` — both accepted; `bufferFromBase64` tolerates either.)

**npm dep surface being replaced** (`package.json`): rollup + 4 plugins, babel preset-env, screeps-api, yamljs, yargs, fs-extra (dev) and the polyfill package (runtime). Also an npm-coupling footnote: `deploy.js:32` reads `process.env.npm_package_name`, so it only runs under `npm run`.

---

## 2. Prior art

### 2a. cargo-screeps (rustyscreeps) — *the idea already exists, ~80% of the design, stale*

Clone: `C:\code\cargo-screeps` (v0.5.2; last commit **2024-02-27** — ~2.3 years stale). Verdict: **right shape, wrong era; revive the design, not the code.**

- **Build**: links **`wasm-pack = "0.12"` as a library** (`Cargo.toml`), calling `wasm_pack::command::build::Build::try_from_opts(BuildOptions{ target: Target::Nodejs, ... }).run()` (`src/build/world.rs:5,34–46`). So wasm-pack-as-a-crate is proven — it inherits wasm-pack's version-matched wasm-bindgen-cli download and wasm-opt handling for free, but freezes you to wasm-pack 0.12's behavior and its heavy dep tree.
- **JS glue**: post-processes the `--target nodejs` bindgen output with **one big regex** (`src/build/world.rs:89–131`): strips the `require('util')` TextEncoder/Decoder import and the `fs.readFileSync(path.join(__dirname, '*.wasm'))` tail, prepends the inline CC-0 [FastestSmallestTextEncoderDecoder](https://github.com/anonyco/FastestSmallestTextEncoderDecoder) polyfill (the *same* polyfill the ibex pipeline bundles from npm), and replaces the tail with `const bytes = require('<name>_bg'); const wasmModule = new WebAssembly.Module(bytes); module.exports.initialize_instance = ...` — i.e. **deferred instantiation under loader control**. Its own error message admits the fragility: *"'wasm-pack' generated unexpected JS output! This means it's updated without 'cargo screeps' also having updated"* (`world.rs:102–110`).
- **Upload** (`src/upload.rs`): walks configured dirs, `.js` → string, `.wasm` → `{ "binary": base64 }` (lines 41–66), POSTs `{ modules, branch }` to `…/api/user/code` (`run.rs:81–82`), auth `X-Token` header or HTTP basic (`upload.rs:138–149`), 5 MiB warn. **Byte-for-byte the same wire format as deploy.js.**
- **Why the rustyscreeps starter dropped it** (starter README: "not used in 0.22+"): the regex patcher kept breaking on wasm-bindgen output drift, and the starter needed a richer loader (bucket gating, staged init, the #3130 halt trap) than cargo-screeps' generated tail — so they retreated to wasm-pack+rollup+a hand-written main.js. The lesson is **not** "native is unviable"; it's "the glue patcher must be version-aware and the loader must be a real template, not a regex suffix."

### 2b. trunk-rs — *the modern orchestration reference*

Clone: `C:\code\trunk`. Patterns worth stealing (web-app parts ignored):

- **cargo build as subprocess + artifact discovery**: plain `cargo build --target=wasm32-unknown-unknown ...` then a second `--message-format=json` invocation parsed with `cargo_metadata::Message::parse_stream` to locate the produced `.wasm` exactly (`src/pipelines/rust/mod.rs:406–537`). No guessing at target-dir layout.
- **Version-locked tool acquisition**: wasm-bindgen version resolved **config override → `Cargo.lock` (via the `cargo_lock` crate) → `Cargo.toml`** (`src/pipelines/rust/wasm_bindgen.rs:58–90`), then the **matching prebuilt CLI** is downloaded from GitHub releases — `wasm-bindgen-{version}-x86_64-pc-windows-msvc.tar.gz` is a first-class artifact (`src/tools.rs:177–191`) — into a content-addressed cache dir with offline-mode and retry handling (`src/tools.rs:253–376,431+`). Binaryen/wasm-opt acquired the same way (`tools.rs:196–199`) and invoked as a subprocess with `-O<level>` + extra args (`src/pipelines/rust/mod.rs:901–956`).
- **Version-gated output handling**: `WasmBindgenFeatures::from_version` flips glue behavior per bindgen version (e.g. `init_with_object` for ≥0.2.93, [wasm-bindgen#3995](https://github.com/rustwasm/wasm-bindgen/pull/3995)) (`wasm_bindgen.rs:92–118`). **This is the antidote to cargo-screeps' regex rot**: detect the version you bindgen'd with, select the matching patch/template.
- Notably trunk does **not** use `wasm-bindgen-cli-support` as a library — deliberate, because of the version lock (§3, footgun).

### 2c. Building blocks verified

- **`wasm-bindgen-cli-support`** is a real published library (0.2.123 current): `Bindgen::new().input_path(..).nodejs(true)?.out_name(..).typescript(false).generate(out_dir)` — full builder API confirmed on [docs.rs](https://docs.rs/wasm-bindgen-cli-support/latest/wasm_bindgen_cli_support/struct.Bindgen.html) (also `input_bytes`, `generate_output() -> Output` for in-memory use). **The footgun**: the bindgen *schema version* embedded in the `.wasm` by the user's `wasm-bindgen` crate must **exactly match** the cli-support version (`verify_schema_matches` — error: *"the Rust project used to create this wasm file was linked against version of wasm-bindgen that uses a different bindgen format than this binary"*; [wasm-bindgen#1587](https://github.com/rustwasm/wasm-bindgen/issues/1587), [discussion#3684](https://github.com/wasm-bindgen/wasm-bindgen/discussions/3684), [trunk#195](https://github.com/trunk-rs/trunk/issues/195)). Linking it fixes the supported version at *tool*-compile time — acceptable for a single repo, **wrong default for a community tool**, where the user's lockfile decides.
- **`wasm-opt` crate** ([brson/wasm-opt-rs](https://github.com/brson/wasm-opt-rs), 0.116.1 = binaryen 116, Mar 2024): real Rust bindings via cxx, builds binaryen from source — works on Windows MSVC (C++17 compiler, no CMake) but adds minutes of cold build time and trails binaryen releases. Trunk-style **prebuilt binaryen download** is lighter, matches what wasm-pack does today, and keeps the binaryen version a config knob. Use download as default; the crate is the "pure cargo, no network" alternative.
- **Upload API**: triple-confirmed (`deploy.js`/screeps-api, cargo-screeps `upload.rs`, screeps_tool python) — `POST /api/user/code`, JSON `{ branch, modules }`, wasm as `{ binary: base64 }`, auth `X-Token` or basic; engine-side decode confirmed at `screeps-engine/src/game/game.js:561–563`.

---

## 3. Proposed architecture

**Crate: `screeps-pack`** — library + CLI, started as a workspace-excluded peer dir (`screeps-pack/`, the `screeps-eval`/`screeps-prospector` lifecycle: in-repo → extract to own remote when stable). Binary doubles as a cargo subcommand (`cargo screeps-pack deploy …` via the `cargo-screeps-pack` shim) — but plain `screeps-pack deploy` is the primary UX, mirroring deploy.js.

Pipeline (each step = a library module; the CLI is a thin clap wrapper, same pattern as `screeps-eval`):

| Step | Mechanism | Source of truth |
|---|---|---|
| **config** | `serde_yaml` over the **same `.screeps.yaml`**: `servers.<name>` (SS3) + the existing `configs.wasm-pack-options` / `configs.terser` keys honored as-is for drop-in parity (a `configs.screeps-pack` section can alias them later). Secrets in `secrecy::SecretString` per the A7 discipline. | `.example-screeps.yaml`; deploy.js parity |
| **build** | subprocess `cargo build --target wasm32-unknown-unknown --release -p screeps-ibex` + the configured extra args (`--config build.rustflags=['-Ctarget-cpu=mvp'] -Z build-std=std,panic_abort`, per-server `--features`); artifact located via a `--message-format=json` pass (`cargo_metadata`). Nightly + `rust-src` presence checked up front with a clear error. | trunk `rust/mod.rs:406–537` |
| **bindgen** | wasm-bindgen version from **`Cargo.lock`** (`cargo_lock` crate); download the **matching** prebuilt `wasm-bindgen-cli` (`…-x86_64-pc-windows-msvc.tar.gz`; linux/mac variants for the community) to `%LOCALAPPDATA%\screeps-pack\cache`, run `wasm-bindgen --target nodejs --out-dir <work> --out-name <crate_name_underscored> --no-typescript <artifact.wasm>`. *Optional fast path:* link `wasm-bindgen-cli-support` and use it **only when** lockfile version == compiled-in version (zero downloads); fall back to download on mismatch. The version-lock footgun (§2c) is thereby handled the trunk/wasm-pack way, never trusted to a static pin. | trunk `tools.rs:177–191`, `wasm_bindgen.rs:58–90` |
| **opt** | download binaryen release (version pinned in config, default = whatever wasm-pack currently pins for parity), run `wasm-opt` with args from the existing `[package.metadata.wasm-pack.profile.*.wasm-opt]` tables (`screeps-ibex/Cargo.toml:46–59`) so behavior is identical; dev profile keeps `--signext-lowering` only. | trunk `tools.rs:196–199`, `rust/mod.rs:901–956`; wasm-pack metadata compat |
| **glue** | (a) **Patch** the `--target nodejs` bindgen JS with *anchored, version-gated* replacements (trunk's `WasmBindgenFeatures` pattern, not cargo-screeps' single regex): drop the `require('util')` line → polyfill prepend (vendored CC-0 FastestSmallestTextEncoderDecoder — already the exact polyfill both current pipelines use); drop the `fs`/`path` wasm-load tail → `module.exports.__compile = () => new WebAssembly.Module(require('<name>_bg')); module.exports.__instantiate = (m) => { … }` (deferred, loader-controlled — cargo-screeps' `initialize_instance` idea, split to preserve ibex's per-tick staging). Unsupported bindgen version ⇒ **hard error naming the supported range**, not silent corruption. (b) **Template** `main.js` from an embedded askama/format template = the genericized `js_src/main.js`: bucket gate (threshold configurable), staged require→compile→instantiate, `console_error`/`Game.notify`, the #3130 `running`+`halt()` trap, `setup`/`game_loop` export names configurable. | cargo-screeps `world.rs:89–131`; `js_src/main.js`; trunk `wasm_bindgen.rs:92–118` |
| **upload** | `reqwest` (rustls): private servers `POST /api/auth/signin` → token; official servers config token; then `POST {scheme}://{host}:{port}{prefix}/api/user/code` `{ branch, modules }` with `X-Token`; `.js` → string, `.wasm` → `{ binary: base64 }`; 5 MiB check; `--dryrun` prints the map summary. | deploy.js:121–160; cargo-screeps `upload.rs`; screeps_api.py |

Resulting module map (3 modules vs today's 2 — rollup inlined the bindgen JS into `main`; we upload it as its own CJS module instead, which the engine's `requireFn` handles natively):

```json
{ "main": "<loader template>", "screeps_ibex": "<patched bindgen JS>", "screeps_ibex_bg": { "binary": "<base64>" } }
```

**Dropped, deliberately:** rollup (no ESM anywhere once `--target nodejs` is used), babel (cargo-screeps shipped un-transpiled bindgen output to the same isolated-vm runtime for years; bindgen's emitted syntax is ES2017-era), terser (off by default today; revisit only if the 5 MiB ceiling threatens), and the `npm_package_name` env coupling (name from `cargo metadata`).

**Windows-first:** every element verified Windows-viable — bindgen/binaryen publish `x86_64-pc-windows-msvc` archives (trunk `tools.rs:177,196`), tar.gz extraction is in-process (`flate2`+`tar`), cache under `%LOCALAPPDATA%`, no bash, no npm, no symlinks.

## 4. Community-share design

Target user: **any crate** that links `screeps-game-api` + `wasm-bindgen` with `crate-type = ["cdylib"]` — no starter clone, no `js_src/`, no `package.json`.

- **Zero-config convention:** `screeps-pack deploy --server x` from a crate root assumes: module name = crate name underscored; entry exports `setup` (once) and `game_loop` (per tick) — the rustyscreeps-starter convention ibex already follows (`lib.rs:44–51`); creds from `./.screeps.yaml` (SS3 — shared with every other screepers tool); release profile; bucket threshold 1500.
- **Overrides** in `[package.metadata.screeps]` (Cargo-native, travels with the crate): export names, bucket threshold, per-server cargo flags/features (the `wasm-pack-options` equivalent), wasm-opt args, binaryen pin. `.screeps.yaml`'s `configs:` section remains honored for starter/ibex compatibility.
- **`js_src/` becomes embedded templates** inside the tool: the loader (`main.js` genericized) + the vendored polyfill. A crate-local `screeps-pack/loader.js` override hook covers bots needing custom glue.
- **Relationship to `screeps-game-api`:** none at the dependency level (cargo-screeps's README makes the same point — "does not depend on" it, they just "go together well"). The tool only assumes wasm-bindgen; game-api is what makes the wasm useful. The local fork at `C:\code\screeps-game-api` needs nothing from this tool.
- **Upstreaming venue:** the **rustyscreeps org** — either as `cargo-screeps`' successor (an issue proposing revival-by-replacement; its config/UX users are exactly the audience, and v0.5.2's regex patcher is the acknowledged weak point) or a fresh repo linked from `screeps-starter-rust`'s README as the npm-free alternative path. Decide after the prototype proves parity; until then it lives here.

## 5. Parity-validation plan

Gate the deploy-path switch on all of:

1. **Same-input builds**: same commit, same flags, debug + release, via deploy.js and `screeps-pack build --dryrun`.
2. **wasm byte-compare** (expected-stable when versions are pinned equal): pin `screeps-pack` to the same wasm-bindgen-cli (lockfile-derived — automatically equal) and the same binaryen version wasm-pack used; then `screeps_ibex(_bg).wasm` must be byte-identical. Mismatch ⇒ explain or fix before proceeding; if binaryen versions can't be matched, fall back to size-delta < 1% + `wasm-objdump` section diff.
3. **Module-map diff** (`--dryrun` artifacts): expected delta is exactly {2 modules → 3, bundled JS → patched JS + loader}; binary payload identical per (2); total size within budget.
4. **Smoke on the eval server** (the P0.A4/A6 harness): `bootstrap --reset` → deploy via prototype → `run --ticks 2000`; assert the A6 hard zeros (deploy success, "loading complete" line, ticks observed > 0, zero panics/deser failures) and bucket-gated boot + a forced-error `halt()` probe behaving as today.
5. **Cross-check both directions**: deploy.js build runs after a prototype build and vice versa (no dist/pkg cross-contamination).
6. **MMO untouched** throughout (Phase-0 MMO guard).

## 6. Verdict: **GO** — with the prototype gated behind Workstream-A stability

Viability is not in doubt; every risky claim is verified against working code:

- **JS-glue crux — answered yes**: `--target nodejs` output post-processed by string patching is *proven on this exact game* by cargo-screeps; the engine runtime is plain CJS + `{binary}`→Buffer require (engine source, §1); rollup's only essential transform (ESM→CJS) disappears with the nodejs target; babel/terser are dispensable. The loader (`js_src/main.js`) is 88 lines of self-contained template material.
- **Version-lock — answered**: resolve from `Cargo.lock`, download the matching `wasm-bindgen-cli` (trunk's exact mechanism, Windows archives published); never static-pin `wasm-bindgen-cli-support` for community use.
- **Upload — answered**: one `reqwest` POST, format triple-confirmed.

**Effort**: prototype to first green smoke ≈ **3–5 focused days** (config/upload ~1d, build+tool-acquisition ~1d, glue patcher+templates ~1–2d, parity runs ~1d — most patterns are direct ports from the two reference clones). Community polish (linux/mac paths, version-range CI against fresh wasm-bindgen releases, docs, upstream conversation): ~1–2 weeks elapsed, not blocking internal use.

**Top 3 risks**
1. **Bindgen-output drift** breaking the glue patcher (the thing that killed cargo-screeps' UX). Mitigation: version-gated anchored patches + a hard supported-range error + a CI canary that bindgens a fixture crate against the latest wasm-bindgen release.
2. **Silent flag loss**: dropping `-Ctarget-cpu=mvp`/`build-std` (or wasm-opt's `--signext-lowering`) yields wasm that loads in dev but is rejected by older server V8s. Mitigation: parity step 2's byte-compare, plus a post-build wasm feature scan (reject post-MVP opcodes) as a tool-native check deploy.js never had.
3. **Long-tail upload/auth variance** (private-server auth mods, branch auto-creation semantics, base64 padding). Low — all three references agree — but the parity smoke runs against the real launcher stack before any switch.

## 7. Non-goals

- MMO **season** specifics (feature-flag pass-through suffices; no season build variants).
- Asset pipelines of any kind (trunk's HTML/CSS/SASS machinery is explicitly *not* being ported).
- Screeps **Arena** support (cargo-screeps has it; out of scope until World parity ships).
- Watch/serve/hot-reload modes; JS minification; replacing `js_tools/deploy.js` before §5 passes — it remains the deploy path of record.
