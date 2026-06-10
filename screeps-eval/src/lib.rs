//! screeps-eval — private-server execution, deployment, and evaluation
//! harness for screeps-ibex.
//!
//! Library-first: every capability the CLI exposes is a library
//! function, so integration tests (and later `screeps-testkit`) drive
//! the same code paths the operator does. See `docs/execution/phase-0.md`
//! Workstream A for the task map (P0.A1–A8).
//!
//! Module map (Phase-0 build-out order):
//! - [`config`]        — `.screeps.yaml` + env loading, secrets policy (A1/A7)
//! - [`server_config`] — launcher-config merge -> `target/runtime/`    (A2)
//! - [`docker`]        — bollard lifecycle of launcher/mongo/redis     (A2)
//! - [`server`]        — server-CLI client: bootstrap, tick control    (A3/A8)
//! - `api`             — HTTP/websocket client: deploy, console, segments (A4/A5)
//! - `capture`         — runs/ artifact writing, summaries             (A5/A6)

pub mod config;
pub mod docker;
pub mod server;
pub mod server_config;

// Stubs land with their tasks (kept out until then so the crate carries
// no dead code from day one):
// pub mod api;      // P0.A4 / P0.A5
// pub mod capture;  // P0.A5 / P0.A6
