# screeps-pack — preserved internal design notes

> Snapshot of the pre-extraction `README.md` for `screeps-pack`, preserved when the crate
> was extracted to https://github.com/Azaril/screeps-pack with an external-facing README.
> These are internal design/provenance notes (some now historical). Relative links
> below refer to the original in-repo layout.

---

# screeps-pack

Build and deploy a Rust [Screeps](https://screeps.com) bot **without
npm, node, webpack, or a starter clone**. One tool takes your crate from
`cargo build` to running code on any server — official MMO/PTR/season
(token auth) or a private server (username/password):

```
screeps-pack deploy --server private-server
screeps-pack deploy --server mmo
```

The pipeline: `cargo build --target wasm32-unknown-unknown` →
`wasm-bindgen` (the **exact version your `Cargo.lock` pins**, prebuilt
CLI downloaded and cached automatically) → CommonJS glue the Screeps
isolate can `require()` → `wasm-opt` (binaryen, optional) → upload via
`POST /api/user/code`. A **library** with a thin CLI — harnesses (like
[`screeps-server-kit`](../screeps-server-kit)) call the same functions
programmatically.

## Requirements

- **Rust** with the `wasm32-unknown-unknown` target (`rustup target add
  wasm32-unknown-unknown`). If your build flags use `-Z build-std`
  (common for Screeps, see below), you also need **nightly** + the
  `rust-src` component — pin both in your `rust-toolchain.toml`.
- A bot crate with `crate-type = ["cdylib", ...]` that links
  `wasm-bindgen` and exports `setup()` (called once after load) and
  `game_loop()` (called per tick) — the
  [screeps-starter-rust](https://github.com/rustyscreeps/screeps-starter-rust)
  convention. No `js_src/`, no `package.json` needed.
- A [`.screeps.yaml`](https://github.com/screepers/screepers-standards/blob/master/SS3-Unified_Credentials_File.md)
  (the SS3 unified credentials file shared by screepers tooling).

## Usage

Run from your bot crate's directory (or its workspace root — the
workspace's single cdylib member is found automatically):

```
screeps-pack check  --server mmo     # print the resolved plan; build nothing
screeps-pack build  --server mmo     # build the full module map; upload nothing
screeps-pack deploy --server mmo     # build + upload
screeps-pack deploy --server ptr --debug    # debug build (no --release)
screeps-pack deploy --server mmo --dryrun   # build, print the map, skip upload
```

Defaults (override with the flags):

| What | Default | Override |
|---|---|---|
| Bot crate | `./Cargo.toml` (a workspace root resolves to its single cdylib member) | `--manifest-path <path>` |
| Credentials | `./.screeps.yaml` | `--creds <path>` |
| Branch | the entry's `branch:` (else `default`) | set `branch:` in the entry |

### .screeps.yaml

```yaml
servers:
  mmo:
    host: screeps.com
    secure: true
    token: your-auth-token        # official servers: token auth only
  ptr:
    host: screeps.com
    secure: true
    token: your-auth-token
    path: /ptr                    # URL prefix for ptr/season
  private-server:
    host: 127.0.0.1
    port: 21025
    username: you
    password: your-password       # private servers: signin auth

configs:
  # Extra args appended to `cargo build`, resolved per server:
  # '*' applies everywhere, per-server entries are CONCATENATED after it.
  wasm-pack-options:
    '*': ["--config", "build.rustflags=['-Ctarget-cpu=mvp']", "-Z", "build-std=std,panic_abort"]
    mmo: ["--features", "mmo"]
    ptr: ["--features", "mmo"]
```

The `configs.wasm-pack-options` section is honored exactly as the
classic deploy.js/wasm-pack pipeline resolved it, so existing configs
are drop-in. (The historical name is kept for that compatibility — the
args go to `cargo build` directly.) The `-Ctarget-cpu=mvp` +
`build-std` combination keeps the emitted wasm inside the MVP feature
set older Screeps server Node versions require — don't drop it without
knowing your target server's V8.

A `configs.terser` section, if present, is **ignored**: screeps-pack
does not minify (see the size note below).

### Crate metadata (optional)

```toml
# In your bot crate's Cargo.toml.

# wasm-opt args per build mode — the same tables wasm-pack reads, so
# existing crates are drop-in. Absent => "-O". `false` => skip wasm-opt.
[package.metadata.wasm-pack.profile.dev]
wasm-opt = ["--signext-lowering"]
[package.metadata.wasm-pack.profile.release]
wasm-opt = ["-O4", "--signext-lowering"]

# screeps-pack's own knobs.
[package.metadata.screeps-pack]
bucket-boot-threshold = 1500   # the loader's boot gate (default 1500)
```

### What gets uploaded

Three modules (vs the bundler pipeline's two — there is no bundler, so
the wasm-bindgen glue is uploaded as its own CommonJS module):

| Module | Content |
|---|---|
| `main` | the loader: bucket-gated boot, **staged multi-tick wasm init** (bytes → compile → instantiate on separate ticks if needed), a `console.error` shim, and the [wasm-bindgen#3130](https://github.com/rustwasm/wasm-bindgen/issues/3130) `Game.cpu.halt()` trap for wedged ticks |
| `<crate_name>` | the wasm-bindgen `--target nodejs` glue, patched for the isolate: a vendored CC0 TextEncoder/TextDecoder polyfill prepended (the isolate has neither), the `fs`/`__dirname` wasm load replaced by a deferred `__instantiate` under loader control |
| `<crate_name>_bg` | the wasm, uploaded as a `{binary: <base64>}` module |

Artifacts (the three files + a `manifest.json` with sizes and sha256
hashes) are written under
`<target>/screeps-pack/dist/<server>/<mode>/` for inspection.

### Size note (no minification)

screeps-pack drops the bundler entirely (rollup/babel/terser). The JS
payload is the raw bindgen glue + the ~3.9 KB loader — measured on a
real bot, **+8.5 KB of JS (+0.26% of total upload)** vs the un-minified
rollup bundle, while the wasm (the actual payload) is byte-identical.
If you previously enabled terser minification, expect roughly that JS
delta; against the 5 MiB code limit it is noise.

### Failure modes worth knowing

- **Unverified wasm-bindgen output**: the glue patcher is *anchored* —
  it refuses to patch JS shapes it hasn't been verified against
  (`src/glue.rs`, `VERIFIED_BINDGEN_OUTPUT`) rather than risk silent
  corruption. If your lockfile moves to a newer wasm-bindgen and the
  output drifted, you get a hard error naming the version; extending
  the patcher is a small, test-covered change.
- **wasm-opt unavailable** (download blocked, unsupported platform):
  the step is skipped with a warning — the upload still works, just
  bigger/slower wasm.
- **Over 5 MiB**: the map is uploaded anyway (matching classic deploy.js
  behavior) after a loud warning; the server will reject it. Debug
  builds of large bots commonly exceed the limit — that's the build
  mode, not the tool.

## Design

Each pipeline step is a library module; `main.rs` is a thin clap
wrapper. Provenance is cited inline — the architecture follows
`docs/design/rust-native-deploy-investigation.md` (this repo) and
steals deliberately from [trunk](https://github.com/trunk-rs/trunk) and
[cargo-screeps](https://github.com/rustyscreeps/cargo-screeps):

```
src/config.rs       .screeps.yaml (SS3) + configs.wasm-pack-options,
                    resolved per-server with deploy.js's exact
                    '*'-then-server concatenation; secrets in
                    secrecy::SecretString from the parse boundary
src/project.rs      bot-crate resolution (direct manifest or single
                    cdylib workspace member) + metadata knobs
src/cargo_build.rs  cargo build + artifact discovery from the
                    --message-format=json stream (trunk's pattern; no
                    target-dir guessing)
src/bindgen.rs      wasm-bindgen version from the bot's Cargo.lock ->
                    matching prebuilt CLI downloaded + cached under
                    <target>/screeps-pack/tools (NEVER a static
                    cli-support pin: the bindgen schema in the wasm
                    must match the CLI exactly, wasm-bindgen#1587)
src/glue.rs         anchored, version-gated patch of the nodejs-target
                    glue (deferred __instantiate) + the embedded loader
                    template + vendored polyfill — cargo-screeps' regex
                    patcher rotted with bindgen drift; anchors hard-error
                    instead
src/opt.rs          optional wasm-opt: binaryen release pinned (116 =
                    wasm-pack 0.14's bundled version; byte-parity
                    verified, see PARITY.md), PATH fallback, graceful skip
src/upload.rs       3-module map + manifest.json; POST /api/user/code
                    via the shared screeps-rest-api client (token or
                    user/pass; private-server token rotation handled)
templates/loader.js                 the genericized starter main.js
templates/text-encoder-decoder.min.js  vendored CC0 polyfill
```

Why `--target nodejs` instead of `--target web` + a bundler: the
Screeps runtime is CommonJS evaluated in isolated-vm — nodejs-target
output is already CJS, so the bundler's only structural job (ESM→CJS)
disappears, and the engine's native handling of `{binary}` modules
(`require()` returns a Buffer) replaces the file-system wasm load. The
produced `_bg.wasm` is byte-identical across the two bindgen targets
(verified — PARITY.md).

Secrets discipline: tokens/passwords live in `secrecy::SecretString`
from the YAML parse onward (Debug-redaction pinned by tests); they are
exposed only into auth headers/signin bodies inside
`screeps-rest-api`. Nothing secret reaches logs, dist files, or
`manifest.json`.

Lifecycle: in-repo and workspace-excluded for now; extracts to its own
repository once stable (the screeps-server-kit/screeps-rest-api
lifecycle). Upstreaming to the rustyscreeps org as the npm-free
deploy path is the stated goal of the investigation doc.

## Verification

`cargo test` (35 unit tests: flag passthrough, argv pins, artifact
discovery, glue anchors, loader render, module-map shape, secret
redaction) and `cargo clippy --all-targets` run clean. Live parity
against the classic deploy.js pipeline — module-map diff, byte-identical
wasm in both build modes, and a green 600-tick private-server smoke —
is recorded in [PARITY.md](PARITY.md).
