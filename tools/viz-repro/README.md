# viz-repro — visual wire-format repro & client-parser replay harness

Forensic harness from the 2026-06-11 visual-corruption root-cause investigation (ADR 0016).
Not part of the workspace; build standalone.

- `src/lib.rs` — wasm crate (pinned to the bot's locked js-sys / wasm-bindgen / serde-wasm-bindgen,
  path-dep on `C:/code/screeps-game-api`) that runs the **real** `serde_wasm_bindgen::to_value`
  serialization path for every `Visual` / `MapVisualShape` primitive.
- `sim.js` / `sim2.js` — Node replay of the downstream pipeline: the engine's
  `JSON.stringify(data)+"\n"` append (console.js:65, strings appended **verbatim** — no newline),
  the backend's `""`-buffer + room-buffer concatenation (no separator), and **both client parsers**
  (room view: clearRect + unguarded per-line `JSON.parse`; map view: unguarded parse +
  `validate()` that never checks `s` + clear-then-draw with unguarded `s` derefs in an
  error-handler-less rxjs subscription). These are ports of the acceptance predicates extracted
  from the Steam client bundle (`build.min.js` + `app2/main.js.map`).

Findings ledger lives in ADR 0016 (failure-mode ground truth) — verified: one bad `""`-target
line blanks all rooms transiently; one bad map line kills map visuals **until client reload**;
NaN style field → JSON `null` → room-view `font: null.replace` TypeError. Refuted for the current
toolchain: shape divergence, ES-Map `"{}"`, packed-Position, `to_value` panic.

This is the seed for the ADR 0016 M0 render-acceptance probe (emit each primitive on the private
server → read `roomVisual:<uid>,<target>,<tick>` from Redis → replay through these parsers).
