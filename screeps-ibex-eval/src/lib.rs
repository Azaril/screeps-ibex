//! screeps-ibex-eval — the evaluation POLICY for screeps-ibex (Phase 0 plan,
//! P0.A14): what "correct/healthy" means for THIS bot.
//!
//! All mechanism — Docker stack lifecycle, world bootstrap, deploy
//! orchestration, console/metrics capture — lives in the generic
//! [`screeps_server_kit`] crate. This crate supplies the ibex-specific
//! pieces and the thin `smoke`/`run` CLI:
//!
//! - [`gates`] — the smoke-gate markers pinned against the bot crate's
//!   sources (panic-hook output, deserialization-failure log lines, the
//!   fern log format, the live-stats segment), packaged as the
//!   [`screeps_server_kit::capture::CaptureSpec`] the kit consumes.
//! - [`smoke`] — the one-command loop: server up → bootstrap --reset →
//!   deploy → run --ticks K → hard-zero gate verdict.
//!
//! Later (ADR 0006): baseline comparison, the colony-health score, and
//! regression diffing land here — they are ibex policy, not mechanism.

pub mod gates;
pub mod scenario;
pub mod score;
pub mod smoke;
