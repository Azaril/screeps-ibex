//! Live console tail (operator view): subscribe to a bot's console
//! websocket (`user:<id>/console`) and stream lines to stdout. This is
//! the lightweight counterpart to [`crate::capture`]: no artifacts, no
//! metrics, no gate counters — just the bot's live `console.log`/error
//! output, for the manual "what is my bot saying right now" loop.
//!
//! It reuses the shared `screeps-rest-api` console protocol verbatim
//! (the same `ConsoleSocket` handshake/auth/subscribe the capture task
//! uses), so there is no second protocol implementation to drift.

use crate::config::KitConfig;
use anyhow::{Context, Result};
use screeps_rest_api::{console_lines, ConsoleSocket};
use std::time::Duration;

/// Tail `cfg.server`'s console until Ctrl-C (or `seconds`, if set),
/// printing each line. `grep`, when set, keeps only lines containing it
/// (case-insensitive) — handy for narrowing to one subsystem
/// (e.g. `--grep ClaimOp`).
pub async fn tail(cfg: &KitConfig, seconds: Option<u64>, grep: Option<&str>) -> Result<()> {
    // Sign in AS this identity, mint a websocket token (rolling tokens —
    // a fresh one for the socket, same policy as capture), resolve the
    // user id for the channel name.
    let api = crate::api::connect(&cfg.server).await?;
    let user = api.me().await?;
    let token = api.fresh_token().await?;
    let ws_url = api.ws_url();
    let mut socket = ConsoleSocket::connect(&ws_url, token, &user.id)
        .await
        .with_context(|| format!("connecting console websocket for '{}'", cfg.server.username))?;

    eprintln!(
        "tailing console for '{}' (user:{}/console){} — Ctrl-C to stop",
        cfg.server.username,
        user.id,
        match grep {
            Some(g) => format!(", filter \"{g}\""),
            None => String::new(),
        }
    );

    let needle = grep.map(|g| g.to_lowercase());

    // A deadline future: a real sleep when `seconds` is set, otherwise a
    // future that never completes (tail until Ctrl-C).
    let deadline = async {
        match seconds {
            Some(s) => tokio::time::sleep(Duration::from_secs(s)).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = &mut deadline => break,
            event = socket.next_event() => {
                let event = event.context("console websocket")?;
                if !event.channel.ends_with("/console") {
                    continue;
                }
                for payload_line in console_lines(&event.payload) {
                    let keep = needle
                        .as_ref()
                        .map(|n| payload_line.line.to_lowercase().contains(n))
                        .unwrap_or(true);
                    if keep {
                        println!("{}", payload_line.line);
                    }
                }
            }
        }
    }
    Ok(())
}
