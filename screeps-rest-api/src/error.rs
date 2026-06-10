//! Error envelope classification for the Screeps API.
//!
//! Both private servers and screeps.com report application-level errors
//! IN-BAND as `{"error": "..."}` — often with HTTP 200 — so the envelope
//! check must run before any typed parse. HTTP 429 carries the official
//! server's "Rate limit exceeded, retry after <n>ms" message
//! (<https://docs.screeps.com/auth-tokens.html>).

use serde::de::DeserializeOwned;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    /// The application-level error envelope `{"error": "..."}` — both
    /// private servers and screeps.com return it, often with HTTP 200.
    #[error("server error: {message}")]
    Server { message: String },
    /// HTTP 429 — "Rate limit exceeded, retry after <n>ms"
    /// (<https://docs.screeps.com/auth-tokens.html>).
    #[error("rate limited: {message}")]
    RateLimited { message: String },
    #[error("http status {status}: {body}")]
    Http { status: u16, body: String },
    #[error("decoding {context}: {source}")]
    Decode {
        context: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    /// Console-websocket transport failure (boxed: tungstenite's error
    /// is large and would bloat every `Result` in the crate).
    #[error("websocket: {0}")]
    Socket(#[source] Box<tokio_tungstenite::tungstenite::Error>),
    /// The server rejected the websocket `auth <token>` frame.
    #[error("websocket auth failed (token rejected)")]
    SocketAuthFailed,
    /// No `auth ok`/`auth failed` reply within the handshake deadline.
    #[error("websocket auth timed out")]
    SocketAuthTimeout,
    /// The server closed the console websocket.
    #[error("console websocket closed by the server")]
    SocketClosed,
    #[error("{0}")]
    Other(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for ApiError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        ApiError::Socket(Box::new(e))
    }
}

/// Classify an HTTP response into the typed result. Pure — unit-tested
/// against literal fixtures (no network in tests).
pub(crate) fn parse_api_response<T: DeserializeOwned>(
    status: u16,
    body: &str,
    context: &'static str,
) -> Result<T, ApiError> {
    if status == 429 {
        return Err(ApiError::RateLimited {
            message: body.trim().to_owned(),
        });
    }
    // The error envelope can arrive with HTTP 200 — check it first.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(message) = value.get("error").and_then(|e| e.as_str()) {
            return Err(ApiError::Server {
                message: message.to_owned(),
            });
        }
    }
    if !(200..300).contains(&status) {
        let mut body = body.trim().to_owned();
        body.truncate(500);
        return Err(ApiError::Http { status, body });
    }
    serde_json::from_str(body).map_err(|source| ApiError::Decode { context, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OkResponse;

    /// RECORDED SHAPE: the `{"error": ...}` envelope — arrives with
    /// HTTP 200 and must beat the typed parse.
    #[test]
    fn error_envelope_beats_typed_parse() {
        let err = parse_api_response::<OkResponse>(200, r#"{"error":"invalid room"}"#, "ack")
            .unwrap_err();
        match err {
            ApiError::Server { message } => assert_eq!(message, "invalid room"),
            other => panic!("expected Server, got {other:?}"),
        }
    }

    /// HTTP 429 carries the official server's retry message.
    #[test]
    fn http_429_maps_to_rate_limited() {
        let err = parse_api_response::<OkResponse>(
            429,
            r#"{"error":"Rate limit exceeded, retry after 743ms"}"#,
            "ack",
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::RateLimited { .. }), "got {err:?}");
    }

    #[test]
    fn non_2xx_without_envelope_maps_to_http() {
        let err = parse_api_response::<OkResponse>(502, "Bad Gateway", "ack").unwrap_err();
        match err {
            ApiError::Http { status, .. } => assert_eq!(status, 502),
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn garbage_body_maps_to_decode() {
        let err = parse_api_response::<OkResponse>(200, "<html>nope</html>", "ack").unwrap_err();
        assert!(matches!(err, ApiError::Decode { .. }), "got {err:?}");
    }
}
