//! Bearer auth — port of `app.ts`'s `isAuthorized`/`unauthorizedResponse`
//! plus `routes/auth.ts` (login/logout cookie exchange) and
//! `routes/pair.ts` (`POST /api/pair` code redemption).

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::routes::{json_response, read_json_body};

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
pub fn accepts_credential(state: &AppState, provided: &str) -> bool {
    match &state.token {
        None => false,
        Some(token) => {
            token_matches(provided, token)
                || state.pairing.as_ref().is_some_and(|p| p.accepts(provided))
        }
    }
}

/// TS `isAuthorized` — no configured token means everything is allowed.
fn is_authorized(state: &AppState, req: &Request) -> bool {
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
pub async fn auth_gate(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
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

/// `POST /api/pair` — pairing (docs/protocol.md): deliberately
/// unauthenticated — the code, not a bearer token, authorizes it.
pub async fn pair(State(state): State<AppState>, req: Request<Body>) -> Response {
    if req.method() != Method::POST {
        return crate::routes::not_found();
    }
    let Some(pairing) = &state.pairing else {
        return json_response(
            serde_json::json!({ "error": "Pairing is not configured on this server" }),
            StatusCode::NOT_IMPLEMENTED,
        );
    };
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let code = body
        .as_ref()
        .and_then(|b| b.as_object())
        .and_then(|o| o.get("code"))
        .and_then(|c| c.as_str())
        .filter(|c| !c.trim().is_empty());
    let Some(code) = code else {
        return json_response(
            serde_json::json!({ "error": "code is required" }),
            StatusCode::BAD_REQUEST,
        );
    };
    // One 404 for unknown/expired/used — don't leak which case it was.
    match pairing.redeem(code) {
        Some(token) => json_response(serde_json::json!({ "token": token }), StatusCode::OK),
        None => json_response(
            serde_json::json!({ "error": "Invalid or expired pairing code" }),
            StatusCode::NOT_FOUND,
        ),
    }
}

/// Whether the request arrived over TLS — Bun knows the socket scheme;
/// here the absolute-form URI scheme or `X-Forwarded-Proto` stands in.
fn request_is_https(req: &Request) -> bool {
    req.uri().scheme_str() == Some("https")
        || req
            .headers()
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            == Some("https")
}

/// `POST /api/auth/login` — exchange a presented token for an httpOnly
/// cookie; `POST /api/auth/logout` — expire it.
pub async fn handle(State(state): State<AppState>, req: Request<Body>) -> Response {
    let path = req.uri().path().to_string();
    let https = request_is_https(&req);
    if req.method() == Method::POST && path == "/api/auth/login" {
        let body = match read_json_body(req).await {
            Ok(body) => body,
            Err(res) => return *res,
        };
        let token = body
            .as_ref()
            .and_then(|b| b.as_object())
            .and_then(|o| o.get("token").and_then(|t| t.as_str()).map(str::to_string));
        let Some(provided) = token.filter(|t| !t.trim().is_empty()) else {
            return json_response(
                serde_json::json!({ "error": "token is required" }),
                StatusCode::BAD_REQUEST,
            );
        };
        // No auth configured → the cookie grants nothing anyway; accept so
        // the client's flow doesn't branch on deployment mode.
        let ok = state.token.is_none() || accepts_credential(&state, &provided);
        if !ok {
            return json_response(
                serde_json::json!({ "error": "Invalid token" }),
                StatusCode::UNAUTHORIZED,
            );
        }
        let secure = if https { "; Secure" } else { "" };
        let res = json_response(serde_json::json!({ "ok": true }), StatusCode::OK);
        return with_set_cookie(res, &provided, secure);
    }
    if req.method() == Method::POST && path == "/api/auth/logout" {
        let res = json_response(serde_json::json!({ "ok": true }), StatusCode::OK);
        return with_expired_cookie(res);
    }
    crate::routes::not_found()
}

fn with_set_cookie(res: Response, token: &str, secure: &str) -> Response {
    let mut res = res;
    let value = format!(
        "{AUTH_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000{secure}",
        form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>()
    );
    if let Ok(v) = HeaderValue::from_str(&value) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

fn with_expired_cookie(res: Response) -> Response {
    let mut res = res;
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("sepia_token=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"),
    );
    res
}
