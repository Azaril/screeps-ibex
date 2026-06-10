//! screeps-prospector — spawn-site selection for screeps-ibex (and any
//! other Screeps bot): scan a server for rooms open to spawning, fetch
//! terrain/objects into a foreman-bench-compatible cache, score rooms,
//! recommend, and place a spawn on explicit confirmation.
//!
//! Library-first: every capability the CLI exposes is a library
//! function, so `screeps-server-kit` bootstrap (the P0.P4 integration) and
//! tests drive the same code paths the operator does. See
//! `docs/execution/phase-0.md` Workstream P for the task map (P0.P1-P5).
//!
//! Module map (build-out order):
//! - [`config`] — `../.screeps.yaml` server selection, secrets policy,
//!   the official-server classification                           (P0.P1)
//! - REST client — the shared [`screeps_rest_api`] crate (P0.A12):
//!   auth, discovery, terrain/objects, place-spawn, respawn;
//!   courtesy rate limit; endpoint shapes pinned there with
//!   citations                                                    (P0.P1)
//! - [`cache`]  — file-backed room cache, bench-format contract   (P0.P2)
//! - [`ops`]    — scan/fetch flows over client + cache         (P0.P1/P2)
//! - [`score`]  — two-stage scoring: cheap heuristics -> offline
//!   foreman plan scoring with plan-derived spawn tiles           (P0.P3)
//! - [`place`]  — confirmation gates (MMO safety) + placement
//!   description                                                  (P0.P4)

pub mod cache;
pub mod config;
pub mod ops;
pub mod place;
pub mod score;
