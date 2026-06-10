//! screeps-rest-api — the ONE Screeps HTTP/websocket API client shared
//! by the host-side tooling (`screeps-server-kit`, `screeps-ibex-eval`, `screeps-prospector`, the
//! future rust-native deploy tool), per Phase-0 task P0.A12: one API
//! client, not N — one place where endpoint shapes are pinned, with
//! source citations, so the tooling stays honest about API interaction.
//!
//! Works against private servers (screepsmod-auth user/password) and
//! screeps.com (auth tokens, shard-aware). Pure library — no binary, no
//! config-file I/O: callers resolve credentials themselves and hand in
//! an [`AuthMode`].
//!
//! Module map:
//! - [`error`]  — [`ApiError`]: the in-band `{"error": ...}` envelope,
//!   HTTP 429 rate limits, transport/decode classification
//! - [`types`]  — typed response shapes for every endpoint, each pinned
//!   to a cited source and fixture-tested
//! - [`code`]   — the `POST /api/user/code` module map (string + binary
//!   modules) for code upload
//! - [`quota`]  — the official per-endpoint quota table (pinned from
//!   node-screeps-api), `X-RateLimit-*` header parsing, 429 retry-after
//!   parsing, and the [`QuotaTracker`] pacing math
//! - [`client`] — [`Client`]: auth (token / user+pass with rolling
//!   X-Token adoption), shard injection, courtesy rate limit +
//!   official-server quota pacing with 429 backoff-and-resume, and the
//!   typed endpoint methods
//! - [`socket`] — the console websocket: frame parsing, the
//!   auth/subscribe handshake ([`ConsoleSocket`]), console-payload
//!   flattening
//!
//! SECRETS POLICY (Phase-0 P0.A7, applied crate-wide): credentials
//! (passwords, tokens) live in [`secrecy::SecretString`] — `Debug`/
//! `Display` redact by construction (pinned by tests). Secrets are
//! exposed only into request bodies / auth headers / the websocket
//! `auth` frame, never into logs or error text.
//!
//! NETWORK POLICY: unit tests parse recorded/literal fixtures only —
//! nothing under `#[cfg(test)]` performs I/O. Live verification happens
//! through the consumers' operator flows (the eval-server gauntlet).

pub mod client;
pub mod code;
pub mod error;
pub mod quota;
pub mod socket;
pub mod types;

pub use client::{
    is_official_target, AuthMode, Client, DEFAULT_MIN_DELAY_MS, DEFAULT_REQUEST_TIMEOUT,
};
pub use code::{CodeModule, CodeModules};
pub use error::ApiError;
pub use quota::{
    endpoint_quota, parse_rate_limit_headers, parse_retry_after_ms, EndpointQuota, QuotaTracker,
    RateLimitInfo, NO_RATE_LIMIT_URL,
};
pub use socket::{
    console_lines, parse_socket_frame, ws_url_from_http_base, ConsoleEvent, ConsoleLineKind,
    ConsolePayloadLine, ConsoleSocket, SocketFrame,
};
pub use types::{
    enumerate_room_names, MapStatsResponse, MemorySegmentResponse, OkResponse, RoomMapStats,
    RoomObjectsResponse, RoomOwner, RoomStatusEntry, RoomStatusResponse, RoomTerrainResponse,
    TerrainEntry, TimeResponse, UserInfo, WorldSizeResponse, WorldStatusResponse,
};
