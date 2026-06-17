//! The console websocket: frame parsing, the auth/subscribe handshake,
//! and console-payload flattening.
//!
//! ## Pinned socket protocol (live private server 2026-06-09 +
//! container sources, screeps-eval P0.A5 bring-up)
//!
//! - Endpoint: `ws://host:port/socket/websocket` — the sockjs **raw
//!   websocket** transport of the backend socket server
//!   (`@screeps/backend lib/game/socket/server.js`:
//!   `socketServer.installHandlers(server, {prefix: '/socket'})`);
//!   messages are plain text frames with no sockjs `a[...]` framing.
//!   Same endpoint the python client family uses.
//! - On connect the server writes `time <unix-ms>` then `protocol 14`
//!   (socket/server.js `conn.write('time ...')`; observed live).
//! - Client sends `auth <token>` (a token from `POST /api/auth/signin`,
//!   or an official auth token); server replies `auth ok <fresh-token>`
//!   or `auth failed` (socket/server.js auth handler —
//!   `authlib.checkToken` then `genToken`). The fresh token is
//!   **dropped at parse time** and never reaches logs or artifacts
//!   (P0.A7).
//! - Client sends `subscribe user:<userId>/console`; the server rejects
//!   `user:`-prefixed channels that do not match the authed user.
//! - Events are text frames of `JSON.stringify([channel, data])`:
//!   - `["user:<id>/console", {"messages":{"log":[...],"results":[...]}}]`
//!     — one per tick, **also when empty** (a useful liveness signal);
//!   - `["user:<id>/console", {"error":"..."}]` — runtime errors.
//!
//!   Source: `@screeps/driver lib/index.js:368` (`sendConsoleMessages`)
//!   and `:409` (`sendConsoleError`); the backend strips the `userId`
//!   field before emitting (`socket/user.js:28-33`).
//! - `gz:`-prefixed deflate frames exist only after a client sends
//!   `gzip on` (socket/server.js); this client never does, so frames
//!   stay plaintext.

use crate::error::ApiError;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

/// How long the server gets to answer the `auth <token>` frame.
pub const AUTH_DEADLINE: Duration = Duration::from_secs(10);

/// Derive the websocket endpoint from an HTTP base URL
/// (`http://h:p` -> `ws://h:p/socket/websocket`; https -> wss).
pub fn ws_url_from_http_base(http_base: &str) -> String {
    let ws_base = if let Some(rest) = http_base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = http_base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        // Already a ws(s) URL or schemeless — pass through unchanged.
        http_base.to_string()
    };
    format!("{ws_base}/socket/websocket")
}

// ===================================================================
// frame parsing (pure)
// ===================================================================

#[derive(Debug, PartialEq)]
pub enum SocketFrame {
    /// `time <unix-ms>` greeting.
    Time(u64),
    /// `protocol <n>` greeting.
    Protocol(u32),
    /// `auth ok <token>` — the refreshed token is DROPPED here, by
    /// construction: it must never reach logs or artifacts (P0.A7).
    AuthOk,
    AuthFailed,
    /// `["<channel>", <payload>]` event.
    Channel {
        channel: String,
        payload: Value,
    },
    /// Heartbeats/unrecognized — ignored.
    Other,
}

pub fn parse_socket_frame(text: &str) -> SocketFrame {
    if let Some(rest) = text.strip_prefix("time ") {
        if let Ok(ms) = rest.trim().parse() {
            return SocketFrame::Time(ms);
        }
    }
    if let Some(rest) = text.strip_prefix("protocol ") {
        if let Ok(v) = rest.trim().parse() {
            return SocketFrame::Protocol(v);
        }
    }
    if text.starts_with("auth ok") {
        return SocketFrame::AuthOk;
    }
    if text.trim() == "auth failed" {
        return SocketFrame::AuthFailed;
    }
    if text.starts_with('[') {
        if let Ok(Value::Array(mut arr)) = serde_json::from_str::<Value>(text) {
            if arr.len() == 2 {
                if let Value::String(channel) = arr.remove(0) {
                    return SocketFrame::Channel {
                        channel,
                        payload: arr.remove(0),
                    };
                }
            }
        }
    }
    SocketFrame::Other
}

// ===================================================================
// console-payload flattening (pure)
// ===================================================================

/// Where a console line came from within the channel payload.
/// Serializes lowercase (`"log"` / `"result"` / `"error"`) — consumers
/// embed this in their artifact records.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleLineKind {
    /// `messages.log[]` — `console.log` output (all bot logging).
    Log,
    /// `messages.results[]` — console-command evaluation results.
    Result,
    /// the `error` field — runtime errors/aborts from the engine.
    Error,
}

/// One flattened console line (kind + text), in payload order.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsolePayloadLine {
    pub kind: ConsoleLineKind,
    pub line: String,
}

/// Decode the HTML entities the official server emits in console output.
///
/// `console.log` text is HTML-escaped for the web client, so e.g. `->` arrives
/// over the wire as `-&#x3E;`, `<` as `&lt;`, `&` as `&amp;`. Consumers want the
/// original text (matching what the in-game console renders), so decode the
/// named entities the sanitizer emits plus decimal/hex numeric character
/// references. A malformed/unknown `&...;` run is left verbatim, and a line
/// with no `&` (the private-server case, never escaped) returns unchanged.
fn decode_html_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..]; // starts with '&'
        // A valid entity is `&...;`; bound the body so stray '&' in prose
        // (e.g. "a & b") doesn't swallow the rest of the line.
        let semi = after[1..]
            .find(';')
            .map(|p| p + 1)
            .filter(|&p| (2..=12).contains(&p));
        if let Some(semi) = semi {
            if let Some(ch) = decode_entity(&after[1..semi]) {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

/// Decode the body of one entity (the text between `&` and `;`).
fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let num = entity.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Flatten a console-channel payload (`{"messages": {"log": [...],
/// "results": [...]}}` / `{"error": "..."}`) into individual lines.
/// Shape source: `@screeps/driver lib/index.js:368/:409` (see the
/// module docs). Log text is HTML-unescaped ([`decode_html_entities`]).
pub fn console_lines(payload: &Value) -> Vec<ConsolePayloadLine> {
    let mut out = Vec::new();
    let mut push = |kind: ConsoleLineKind, v: &Value| {
        let line = match v {
            Value::String(s) => decode_html_entities(s),
            other => other.to_string(),
        };
        out.push(ConsolePayloadLine { kind, line });
    };
    if let Some(messages) = payload.get("messages") {
        for v in messages
            .get("log")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            push(ConsoleLineKind::Log, v);
        }
        for v in messages
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            push(ConsoleLineKind::Result, v);
        }
    }
    if let Some(err) = payload.get("error") {
        push(ConsoleLineKind::Error, err);
    }
    out
}

// ===================================================================
// the live subscription
// ===================================================================

/// One `["<channel>", <payload>]` event from a subscribed channel.
#[derive(Debug)]
pub struct ConsoleEvent {
    pub channel: String,
    pub payload: Value,
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A connected, authenticated console subscription. Construct via
/// [`ConsoleSocket::connect`]; drain with
/// [`next_event`](ConsoleSocket::next_event). Dropping it closes the
/// socket.
pub struct ConsoleSocket {
    sink: SplitSink<WsStream, Message>,
    stream: SplitStream<WsStream>,
}

impl ConsoleSocket {
    /// Connect to `ws_url`, authenticate, and subscribe to
    /// `user:<user_id>/console`.
    ///
    /// SECRETS: this is the ONLY exposure of the websocket token —
    /// straight into the `auth` frame, never into logs or errors
    /// (P0.A7). The `auth ok <fresh-token>` reply is dropped inside
    /// [`parse_socket_frame`]. Private-server tokens are
    /// rolling/consumable, so pass a token minted for this socket
    /// ([`crate::Client::fresh_token`]), not one an HTTP client is
    /// still using.
    pub async fn connect(
        ws_url: &str,
        token: SecretString,
        user_id: &str,
    ) -> Result<Self, ApiError> {
        let (ws, _) = connect_async(ws_url).await?;
        let (mut sink, mut stream) = ws.split();

        sink.send(Message::text(format!("auth {}", token.expose_secret())))
            .await?;

        // Greeting frames (`time`, `protocol`) arrive first; wait for
        // the auth verdict.
        let deadline = tokio::time::Instant::now() + AUTH_DEADLINE;
        loop {
            let msg = tokio::time::timeout_at(deadline, stream.next())
                .await
                .map_err(|_| ApiError::SocketAuthTimeout)?
                .ok_or(ApiError::SocketClosed)??;
            match msg {
                Message::Text(t) => match parse_socket_frame(&t) {
                    SocketFrame::AuthOk => break,
                    SocketFrame::AuthFailed => return Err(ApiError::SocketAuthFailed),
                    _ => continue,
                },
                Message::Ping(p) => sink.send(Message::Pong(p)).await?,
                Message::Close(_) => return Err(ApiError::SocketClosed),
                _ => continue,
            }
        }

        sink.send(Message::text(format!("subscribe user:{user_id}/console")))
            .await?;
        Ok(ConsoleSocket { sink, stream })
    }

    /// The next channel event. Greeting/heartbeat frames are skipped
    /// and pings answered internally; a server-side close (or a closed
    /// stream) returns [`ApiError::SocketClosed`].
    ///
    /// Cancellation: safe to race in `tokio::select!` — the underlying
    /// frame read is cancel-safe. (A cancellation that lands mid-pong
    /// can drop one pong; the server tolerates that.)
    pub async fn next_event(&mut self) -> Result<ConsoleEvent, ApiError> {
        loop {
            let msg = self.stream.next().await.ok_or(ApiError::SocketClosed)??;
            match msg {
                Message::Text(t) => {
                    if let SocketFrame::Channel { channel, payload } = parse_socket_frame(&t) {
                        return Ok(ConsoleEvent { channel, payload });
                    }
                }
                Message::Ping(p) => self.sink.send(Message::Pong(p)).await?,
                Message::Close(_) => return Err(ApiError::SocketClosed),
                _ => {}
            }
        }
    }
}

// ===================================================================
// tests — pure parts against literal fixtures (live shapes 2026-06-09)
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_derivation() {
        assert_eq!(
            ws_url_from_http_base("http://127.0.0.1:21025"),
            "ws://127.0.0.1:21025/socket/websocket"
        );
        assert_eq!(
            ws_url_from_http_base("https://screeps.example:443"),
            "wss://screeps.example:443/socket/websocket"
        );
    }

    /// Literal greeting frames observed live.
    #[test]
    fn parses_greeting_frames() {
        assert_eq!(
            parse_socket_frame("time 1781061591846"),
            SocketFrame::Time(1781061591846)
        );
        assert_eq!(parse_socket_frame("protocol 14"), SocketFrame::Protocol(14));
    }

    /// P0.A7 pin: the refreshed token in `auth ok <token>` is dropped at
    /// parse time — the parsed frame carries no token material.
    #[test]
    fn auth_ok_token_is_dropped_at_parse_time() {
        let fake_token = "ffffffffffffffffffffffffffffffffffffffff";
        let frame = parse_socket_frame(&format!("auth ok {fake_token}"));
        assert_eq!(frame, SocketFrame::AuthOk);
        assert!(
            !format!("{frame:?}").contains(fake_token),
            "token leaked through SocketFrame"
        );
        assert_eq!(parse_socket_frame("auth failed"), SocketFrame::AuthFailed);
    }

    /// The exact (empty) console frame observed live post-subscribe.
    #[test]
    fn parses_live_console_frame() {
        let frame = parse_socket_frame(
            r#"["user:6a28d4d7d9592a0060be10ef/console",{"messages":{"log":[],"results":[]}}]"#,
        );
        let SocketFrame::Channel { channel, payload } = frame else {
            panic!("expected a channel frame");
        };
        assert_eq!(channel, "user:6a28d4d7d9592a0060be10ef/console");
        assert!(console_lines(&payload).is_empty());
    }

    #[test]
    fn unknown_frames_are_other() {
        assert_eq!(parse_socket_frame("h"), SocketFrame::Other);
        assert_eq!(parse_socket_frame(""), SocketFrame::Other);
        assert_eq!(parse_socket_frame("[1,2,3]"), SocketFrame::Other);
        assert_eq!(parse_socket_frame("[not json"), SocketFrame::Other);
    }

    #[test]
    fn console_payload_flattens_log_results_and_error() {
        let payload: Value = serde_json::from_str(
            r#"{"messages":{"log":["(INFO) screeps_ibex: tick","(ERROR) x: boom"],"results":["undefined"]}}"#,
        )
        .unwrap();
        let lines = console_lines(&payload);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].kind, ConsoleLineKind::Log);
        assert_eq!(lines[0].line, "(INFO) screeps_ibex: tick");
        assert_eq!(lines[2].kind, ConsoleLineKind::Result);

        // @screeps/driver lib/index.js:409 — sendConsoleError shape.
        let payload: Value =
            serde_json::from_str(r#"{"error":"Error: wasm trap\n  at ..."}"#).unwrap();
        let lines = console_lines(&payload);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, ConsoleLineKind::Error);
        assert!(lines[0].line.starts_with("Error: wasm trap"));
    }

    /// The official server HTML-escapes console output; decoding restores
    /// the original text. Named entities, decimal + hex numeric refs, and
    /// the literal-`&` / unknown-entity pass-through cases.
    #[test]
    fn decodes_html_entities() {
        // The live shape that motivated the fix: `->` arrives as `-&#x3E;`.
        assert_eq!(
            decode_html_entities("create_construction_site failed -&#x3E; RclNotEnough"),
            "create_construction_site failed -> RclNotEnough"
        );
        assert_eq!(
            decode_html_entities("&lt;tag&gt; &amp; &quot;q&quot; &apos;a&apos;"),
            "<tag> & \"q\" 'a'"
        );
        // Decimal and hex numeric references.
        assert_eq!(decode_html_entities("&#39;&#x41;&#65;"), "'AA");
        // No entities (private-server lines): unchanged, allocation-light.
        assert_eq!(decode_html_entities("(INFO) plain line"), "(INFO) plain line");
        // Stray '&' in prose and unknown entities are left verbatim.
        assert_eq!(decode_html_entities("a & b"), "a & b");
        assert_eq!(decode_html_entities("&unknown; &;"), "&unknown; &;");
        // `&amp;lt;` decodes once to `&lt;`, not to `<` (single pass).
        assert_eq!(decode_html_entities("&amp;lt;"), "&lt;");
    }

    /// console_lines unescapes log/result text end-to-end.
    #[test]
    fn console_lines_unescape_log_text() {
        let payload: Value = serde_json::from_str(
            r#"{"messages":{"log":["(WARN) p: Spawn at (16,17) -&#x3E; RclNotEnough"],"results":["1 &lt; 2"]}}"#,
        )
        .unwrap();
        let lines = console_lines(&payload);
        assert_eq!(lines[0].line, "(WARN) p: Spawn at (16,17) -> RclNotEnough");
        assert_eq!(lines[1].line, "1 < 2");
    }

    /// Consumers embed the kind in JSONL artifact records — pin the
    /// lowercase wire form.
    #[test]
    fn console_line_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ConsoleLineKind::Log).unwrap(),
            r#""log""#
        );
        assert_eq!(
            serde_json::to_string(&ConsoleLineKind::Result).unwrap(),
            r#""result""#
        );
        assert_eq!(
            serde_json::to_string(&ConsoleLineKind::Error).unwrap(),
            r#""error""#
        );
    }
}
