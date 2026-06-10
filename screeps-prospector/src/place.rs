//! Spawn placement: confirmation gates and the placement request
//! description (P0.P4).
//!
//! MMO SAFETY MODEL (hard requirements from the Phase-0 plan, Workstream
//! P): recommend-first, ALWAYS. Nothing is placed without an explicit
//! `--yes`. Against an OFFICIAL server entry (token auth or a
//! screeps.com host — see [`crate::config::ProspectorConfig::is_official`]):
//! - `auto` is REFUSED OUTRIGHT — no flag combination unlocks it
//!   ([`gate_auto`] checks officialness before anything else);
//! - `place` additionally requires `--i-understand-this-is-mmo` on top
//!   of `--yes`.
//!
//! The gates are pure functions so the refusal matrix is unit-testable
//! offline; the CLI calls them BEFORE constructing a client, so a
//! refused command performs zero network I/O.

use thiserror::Error;

/// Default spawn name when the operator does not pass one.
pub const DEFAULT_SPAWN_NAME: &str = "Spawn1";

/// Why a placement command was refused. The Display strings are the
/// operator-facing messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum GateRefusal {
    #[error(
        "refusing to place without explicit --yes (recommend-first: \
         nothing touches the server until you confirm)"
    )]
    NeedsYes,
    #[error(
        "this server entry is OFFICIAL (token auth / screeps.com): `place` \
         additionally requires --i-understand-this-is-mmo"
    )]
    NeedsMmoAck,
    #[error(
        "`auto` is refused outright against official servers (token auth / \
         screeps.com) — run `recommend`, review it, then `place --yes \
         --i-understand-this-is-mmo` if you really mean it"
    )]
    AutoForbiddenOnOfficial,
}

/// Gate for `place`. `--yes` is always required; official servers
/// additionally require the MMO acknowledgement flag.
pub fn gate_place(official: bool, yes: bool, mmo_ack: bool) -> Result<(), GateRefusal> {
    if !yes {
        return Err(GateRefusal::NeedsYes);
    }
    if official && !mmo_ack {
        return Err(GateRefusal::NeedsMmoAck);
    }
    Ok(())
}

/// Gate for `auto`. Official servers are refused FIRST — before the
/// `--yes` check — so no flag combination ever auto-places on MMO.
pub fn gate_auto(official: bool, yes: bool) -> Result<(), GateRefusal> {
    if official {
        return Err(GateRefusal::AutoForbiddenOnOfficial);
    }
    if !yes {
        return Err(GateRefusal::NeedsYes);
    }
    Ok(())
}

/// Everything a placement is about to do, printable BEFORE any gate or
/// network call ("prints exactly what it will do first").
#[derive(Debug, Clone)]
pub struct PlacementRequest {
    pub server_name: String,
    pub base_url: String,
    pub shard: Option<String>,
    pub official: bool,
    pub room: String,
    pub x: u32,
    pub y: u32,
    pub name: String,
}

impl PlacementRequest {
    /// Multi-line, operator-facing description of the exact API call.
    pub fn describe(&self) -> String {
        let shard = self
            .shard
            .as_deref()
            .map(|s| format!(" (shard {s})"))
            .unwrap_or_default();
        let mut out = format!(
            "About to place a spawn:\n\
             \x20 server: {} -> {}{}\n\
             \x20 call:   POST /api/game/place-spawn\n\
             \x20 room:   {}\n\
             \x20 tile:   ({}, {})\n\
             \x20 name:   {}",
            self.server_name, self.base_url, shard, self.room, self.x, self.y, self.name
        );
        if self.official {
            out.push_str(
                "\n  WARNING: this is an OFFICIAL server entry — the placement is real and public.",
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full refusal matrix for `place`.
    #[test]
    fn place_gate_matrix() {
        // Private server: --yes is the only requirement.
        assert_eq!(gate_place(false, false, false), Err(GateRefusal::NeedsYes));
        assert_eq!(gate_place(false, false, true), Err(GateRefusal::NeedsYes));
        assert_eq!(gate_place(false, true, false), Ok(()));
        assert_eq!(gate_place(false, true, true), Ok(()));
        // Official: --yes AND the MMO acknowledgement.
        assert_eq!(gate_place(true, false, false), Err(GateRefusal::NeedsYes));
        assert_eq!(gate_place(true, false, true), Err(GateRefusal::NeedsYes));
        assert_eq!(gate_place(true, true, false), Err(GateRefusal::NeedsMmoAck));
        assert_eq!(gate_place(true, true, true), Ok(()));
    }

    /// The full refusal matrix for `auto`: officialness beats every
    /// other flag — there is NO combination that auto-places on MMO.
    #[test]
    fn auto_gate_matrix() {
        assert_eq!(
            gate_auto(true, true),
            Err(GateRefusal::AutoForbiddenOnOfficial)
        );
        assert_eq!(
            gate_auto(true, false),
            Err(GateRefusal::AutoForbiddenOnOfficial)
        );
        assert_eq!(gate_auto(false, false), Err(GateRefusal::NeedsYes));
        assert_eq!(gate_auto(false, true), Ok(()));
    }

    #[test]
    fn describe_states_the_exact_call_and_flags_official() {
        let mut request = PlacementRequest {
            server_name: "private-server".to_owned(),
            base_url: "http://127.0.0.1:21025".to_owned(),
            shard: None,
            official: false,
            room: "W9N9".to_owned(),
            x: 24,
            y: 21,
            name: DEFAULT_SPAWN_NAME.to_owned(),
        };
        let text = request.describe();
        assert!(text.contains("place-spawn"));
        assert!(text.contains("W9N9"));
        assert!(text.contains("(24, 21)"));
        assert!(text.contains("Spawn1"));
        assert!(text.contains("http://127.0.0.1:21025"));
        assert!(!text.contains("OFFICIAL"), "private servers get no warning");

        request.official = true;
        request.shard = Some("shard3".to_owned());
        let text = request.describe();
        assert!(text.contains("OFFICIAL"));
        assert!(text.contains("shard3"));
    }
}
