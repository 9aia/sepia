//! Bearer auth — port of `app.ts`'s `isAuthorized`/`unauthorizedResponse`.
//! The credential bootstrap routes (`POST /api/pair`, the
//! `/api/auth/login`/`logout` cookie exchange) are `hypha-http`'s —
//! this gate only *reads* the same `sepia_token` cookie, bearer
//! header, and `?access_token` query credential they mint.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::SessionState;

/// Cookie name — value is the credential itself (env token or paired).
pub const AUTH_COOKIE: &str = "sepia_token";

/// Hash both sides so the buffers are always equal length; a raw length
/// mismatch would leak the expected token's length (TS `tokenMatches`).
fn token_matches(provided: &str, expected: &str) -> bool {
    let a = Sha256::digest(provided.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    // Fixed-length constant-time compare.
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Read the auth cookie — `sepia_token=<credential>` — from a request.
fn cookie_token(req: &Request) -> Option<String> {
    let cookie = req.headers().get(header::COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim_start();
        if let Some(value) = part.strip_prefix("sepia_token=") {
            return Some(percent_decode(value));
        }
    }
    None
}

/// `decodeURIComponent` — %XX escapes; malformed sequences pass through
/// literally (`+` is a literal plus, unlike form decoding).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `?access_token` exists only because EventSource can't set headers —
/// the query credential is honored on just the two SSE GET routes (the
/// node event feed and the per-session stream). Everywhere else needs
/// the bearer header.
fn is_query_token_path(path: &str) -> bool {
    if path == "/api/events" {
        return true;
    }
    // `/api/sessions/<id>/stream` — id is one non-empty segment.
    if let Some(rest) = path.strip_prefix("/api/sessions/") {
        if let Some(id) = rest.strip_suffix("/stream") {
            return !id.is_empty() && !id.contains('/');
        }
    }
    false
}

/// Whether the presented credential authenticates — env token or an
/// issued pair credential (checked by hash: equivalent privilege, but
/// revocable-by-file-deletion and never stored in plaintext).
pub fn accepts_credential(state: &SessionState, provided: &str) -> bool {
    match &state.token {
        None => false,
        Some(token) => {
            token_matches(provided, token)
                || state.pairing.as_ref().is_some_and(|p| p.accepts(provided))
        }
    }
}

/// TS `isAuthorized` — no configured token means everything is allowed.
fn is_authorized(state: &SessionState, req: &Request) -> bool {
    let Some(expected) = state.token.as_deref().filter(|t| !t.is_empty()) else {
        return true;
    };
    let mut provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_string);
    // HttpOnly-cookie auth (POST /api/auth/login): the same credential,
    // held outside JS reach — Bearer stays authoritative when both ride.
    if provided.is_none() {
        provided = cookie_token(req);
    }
    if provided.is_none() && req.method() == Method::GET && is_query_token_path(req.uri().path()) {
        provided = crate::routes::query_param(req.uri().query(), "access_token");
    }
    match provided {
        None => false,
        Some(p) => {
            token_matches(&p, expected) || state.pairing.as_ref().is_some_and(|pa| pa.accepts(&p))
        }
    }
}

/// Routes checked ahead of the bearer gate: health (probes), pairing and
/// the cookie login/logout — the credential bootstraps themselves.
fn is_preauth(method: &Method, path: &str) -> bool {
    (method == Method::GET && path == "/api/health")
        || (method == Method::POST
            && matches!(path, "/api/pair" | "/api/auth/login" | "/api/auth/logout"))
}

fn unauthorized() -> Response {
    let mut res = (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({ "error": "Unauthorized" })),
    )
        .into_response();
    res.headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    res
}

/// The bearer gate — middleware mounted over the whole app; pre-auth
/// routes and non-`/api` paths pass through (the TS gate applies only
/// when the first segment is `api`).
pub async fn auth_gate(
    State(state): State<SessionState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if is_preauth(req.method(), path) {
        return next.run(req).await;
    }
    let gated = path == "/api" || path.starts_with("/api/");
    if gated && !is_authorized(&state, &req) {
        return unauthorized();
    }
    next.run(req).await
}

#[cfg(test)]
mod cov_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn req_with_cookie(cookie: &str) -> Request<Body> {
        Request::builder()
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn cookie_and_percent_decode_arms() {
        // No header / no sepia_token / hit.
        assert_eq!(
            cookie_token(&req_with_cookie("other=1; sepia_token=a%20b")).as_deref(),
            Some("a b")
        );
        assert_eq!(cookie_token(&req_with_cookie("other=1")), None);
        // percent_decode: uppercase hex, bad hex, lone %, '+', short tail.
        assert_eq!(percent_decode("%41"), "A");
        assert_eq!(percent_decode("%4z%5"), "%4z%5");
        assert_eq!(percent_decode("a+b%"), "a+b%");
        assert_eq!(percent_decode("%"), "%");
    }

    #[test]
    fn query_token_path_arms() {
        assert!(is_query_token_path("/api/events"));
        assert!(is_query_token_path("/api/sessions/s1/stream"));
        // Empty id / nested id / non-stream suffix → false.
        assert!(!is_query_token_path("/api/sessions//stream"));
        assert!(!is_query_token_path("/api/sessions/a/b/stream"));
        assert!(!is_query_token_path("/api/sessions/s1/other"));
        assert!(!is_query_token_path("/api/health"));
    }
}
