//! Game-API access for the kit — a thin adapter over the SHARED
//! [`screeps_rest_api`] client (P0.A12: one API client, not N).
//!
//! The endpoint plumbing that used to live here (signin/rolling
//! X-Token adoption, game-time, memory segments, world-status,
//! register/check-username, place-spawn, the websocket URL) moved to
//! `screeps-rest-api`, where every shape is pinned with citations and
//! fixture-tested. This module only:
//! - builds a [`Client`] from the kit's [`ServerEndpoint`] (private
//!   server: no shard parameter, no courtesy delay needed), and
//! - maps a 401 signin rejection to the operator-actionable message
//!   (the server-side password diverged — `bootstrap` converges it).
//!
//! SECRETS: the password is re-wrapped into the client's
//! [`screeps_rest_api::AuthMode`] (both ends are `SecretString` —
//! redaction by construction, P0.A7).

use crate::config::ServerEndpoint;
use anyhow::{bail, Context, Result};
use screeps_rest_api::{ApiError, AuthMode, Client};
use secrecy::{ExposeSecret, SecretString};
use std::time::Duration;

/// Build the shared REST client for this endpoint. Private servers
/// take no shard parameter (`shard: None`) and need no courtesy delay
/// (`Duration::ZERO`) — both pinned in the shared crate's docs.
pub fn client(server: &ServerEndpoint) -> Result<Client> {
    Client::new(
        server.http_base(),
        None,
        AuthMode::UserPass {
            username: server.username.clone(),
            // SecretString is not Clone by design; re-wrap the exposed
            // value once, here.
            password: SecretString::from(server.password.expose_secret()),
        },
        Duration::ZERO,
    )
    .context("building the REST client")
}

/// Sign in, surfacing the one failure the operator can act on: a 401
/// means the server-side password does not match `.screeps.yaml`
/// (pinned live 2026-06-09 — passport replies HTTP 401 to a bad
/// password).
pub async fn signin(client: &Client, server: &ServerEndpoint) -> Result<()> {
    match client.sign_in().await {
        Ok(()) => Ok(()),
        Err(ApiError::Http { status: 401, .. }) => bail!(
            "signin as '{}' rejected (401) — the server-side password does not \
             match .screeps.yaml; run `bootstrap` to converge it",
            server.username
        ),
        Err(e) => Err(e).with_context(|| format!("signin as '{}'", server.username)),
    }
}

/// Client + signin in one step (the capture/run entry point).
pub async fn connect(server: &ServerEndpoint) -> Result<Client> {
    let client = client(server)?;
    signin(&client, server).await?;
    Ok(client)
}
