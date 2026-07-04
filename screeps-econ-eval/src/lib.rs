//! # screeps-econ-eval
//!
//! The offline **economy eval harness** (ADR 0040 §D7, milestone M1) over the
//! [`screeps_econ_engine`] M0+M1 kernel — the rover-eval of the economy sim. It exists to prove
//! ONE thing before any cure is designed: that the sim reproduces the live collapse disease
//! (**THE REPRO GATE**, [`metrics::repro_gate_verdict`]) — the transcribed current-bot policy
//! leaks repair energy under refill deficit (`repair_leak_e > 0` on every bait run) and that leak
//! measurably delays recovery (paired bait-vs-control ΔT_recover with a bootstrap 95% CI
//! excluding zero).
//!
//! Layout: [`layout`] realizes a captured foreman layout "as of RCL R" (+ the M2 greenfield
//! variant and the plan build schedule); [`movement`] is the ANALYTIC fast movement tier
//! (fatigue-exact single-creep traces from rover-eval's oracle machinery, memoized — contention
//! ignored BY DESIGN); [`baseline`] is the citation-transcribed live policy (K1 demand / K2
//! selection / K3 repair admission / K4 spawning — since M2 including the upgrader and builder
//! missions/jobs, each line cited to its source) plus the S1-allowance arm; [`workers`] the
//! harvester/hauler/upgrader/builder FSM shells; [`scenario`] the Family-C catalog with per-bait
//! NO-BAIT controls plus the M2 Families G (greenfield rush), D (downgrade pressure), and S
//! (steady-state guard rail); [`oracle`] the T* dependency-chain lower bounds (T_recover +
//! T*_RCL); [`metrics`] T_recover + diagnostics + the deadlock sentinel + the gate; [`runner`]
//! drives one scenario end-to-end (goal-switched per family, with the M2 construction pass).
//! `bin/econ_bench` is the CLI.
//!
//! Determinism (EP-6.13): BTreeMap/sorted boundaries everywhere, integer or exact-rational
//! comparisons in every selection kernel, the seeded kernel RNG only — the fence
//! (`tests/determinism.rs`) proves 5-run spread 0 + intent-permutation invariance.

pub mod baseline;
pub mod layout;
pub mod metrics;
pub mod movement;
pub mod oracle;
pub mod runner;
pub mod scenario;
pub mod workers;
