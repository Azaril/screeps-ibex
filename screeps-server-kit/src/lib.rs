//! screeps-server-kit — a local private-server toolkit for Screeps bot
//! development: bring up the screeps-launcher Docker stack, bootstrap a
//! world (users, spawns, tick rate), deploy a bot, and capture
//! console/metrics artifacts from a run.
//!
//! **Mechanism, not policy** (Phase 0 plan, P0.A14): everything in this
//! crate is bot-agnostic. What "correct/healthy" means for a particular
//! bot — smoke gates, log-marker patterns, scenario orchestration —
//! belongs to a consumer crate (this repo's consumer is `screeps-ibex-eval`);
//! the capture mechanism takes those markers as a [`capture::MarkerSpec`]
//! parameter instead of hardcoding any bot's log strings.
//!
//! Library-first: every capability the operator CLI exposes is a library
//! function, so automation (smoke loops, integration tests) drives the
//! same code paths the operator does.
//!
//! Module map:
//! - [`config`]        — fixed-path loading: creds from `../.screeps.yaml`,
//!   stack settings (incl. `bots:`/`image:`) from `config/local.yml`;
//!   secrets policy
//! - [`server_config`] — launcher-config merge -> `target/runtime/`
//! - [`docker`]        — bollard lifecycle of launcher/mongo/redis;
//!   launcher image pull-or-build
//! - [`server`]        — server-CLI client: multi-bot bootstrap, tick
//!   control
//! - [`api`]           — shared-client construction + signin
//!   diagnostics (the endpoints themselves live in the shared
//!   `screeps-rest-api` crate)
//! - [`deploy`]        — deploy orchestration: a library call into
//!   `screeps-pack` (cargo build -> wasm-bindgen -> glue -> wasm-opt
//!   -> upload via `screeps-rest-api`; P0.A13)
//! - [`capture`]       — console capture + metrics -> runs/ artifacts,
//!   parameterized by the caller's marker spec (the websocket protocol
//!   lives in `screeps-rest-api`)

pub mod api;
pub mod capture;
pub mod config;
pub mod console;
pub mod deploy;
pub mod docker;
pub mod server;
pub mod server_config;
