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
//! - [`api`]           — game-API HTTP client (auth, time, segments)   (A3/A5)
//! - [`deploy`]        — js_tools/deploy.js wrapper                    (A4)
//! - [`capture`]       — console websocket + metrics -> runs/          (A5)
//! - [`smoke`]         — up -> bootstrap -> deploy -> run -> gates     (A6)

pub mod api;
pub mod capture;
pub mod config;
pub mod deploy;
pub mod docker;
pub mod server;
pub mod server_config;
pub mod smoke;
