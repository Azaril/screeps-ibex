//! screeps-eval — private-server execution, deployment, and evaluation
//! harness for screeps-ibex.
//!
//! Library-first: every capability the CLI exposes is a library
//! function, so integration tests (and later `screeps-testkit`) drive
//! the same code paths the operator does. See `docs/execution/phase-0.md`
//! Workstream A for the task map (P0.A1–A8).
//!
//! Module map (Phase-0 build-out order):
//! - [`config`]        — fixed-path loading: creds from `../.screeps.yaml`,
//!   eval settings (incl. `bots:`/`image:`) from `config/local.yml`;
//!   secrets policy                                          (A1/A7/A9/A10)
//! - [`server_config`] — launcher-config merge -> `target/runtime/`    (A2)
//! - [`docker`]        — bollard lifecycle of launcher/mongo/redis;
//!   launcher image pull-or-build                                 (A2/A9d)
//! - [`server`]        — server-CLI client: multi-bot bootstrap, tick
//!   control                                                  (A3/A8/A10)
//! - [`api`]           — shared-client construction + signin
//!   diagnostics (the endpoints themselves live in the shared
//!   `screeps-rest-api` crate)                                     (A3/A12)
//! - [`deploy`]        — js_tools/deploy.js wrapper                    (A4)
//! - [`capture`]       — console capture + metrics -> runs/ (the
//!   websocket protocol lives in `screeps-rest-api`)               (A5/A12)
//! - [`smoke`]         — up -> bootstrap -> deploy -> run -> gates     (A6)

pub mod api;
pub mod capture;
pub mod config;
pub mod deploy;
pub mod docker;
pub mod server;
pub mod server_config;
pub mod smoke;
