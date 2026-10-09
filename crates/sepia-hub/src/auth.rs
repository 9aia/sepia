//! Browser → hub auth — the hub's own token surface. Node bearer tokens
//! stay server-side (`SEPIA_NODES` / `SEPIA_NODE_TOKEN`); this gate
//! covers everything the hub itself serves.
//!
//! When `SEPIA_HUB_TOKEN` is set, every non-asset route requires it:
//! `Authorization: Bearer`, a `?token=` query (EventSource can't set
//! headers — a valid query token also plants the cookie), or the
//! httpOnly `sepia_hub` cookie that `GET /login?token=…` (or any
//! `?token=` page load) sets. Static assets — `style.css`,
//! `manifest.json`, `sw.js`, `icon.svg`, `/pkg/*` — stay open so the
//! PWA shell and service worker load before any credential exists.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

use crate::HubState;

/// Cookie name — the value is the hub token itself (percent-encoded).
pub const HUB_COOKIE: &str = "sepia_hub";

/// Pre-auth paths: the asset surface (`/pkg/*` is the wasm bundle) plus
/// `/login`, which is its own credential bootstrap.
fn is_open_path(path: &str) -> bool {
    matches!(
        path,
        "/login" | "/style.css" | "/manifest.json" | "/sw.js" | "/icon.svg"
    ) || path.starts_with("/pkg/")
}

/// `url.searchParams.get(key)` — form-decoded.
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query.and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    })
}

/// The cookie is stored percent-encoded, so compare encodings — no
/// decoder needed (`byte_serialize` is deterministic).
fn cookie_matches(req: &Request, expected: &str) -> bool {
    let encoded = form_urlencoded::byte_serialize(expected.as_bytes()).collect::<String>();
    let Some(cookie) = req
        .headers()
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
    else {
        return false;
    };
    cookie
        .split(';')
        .map(str::trim_start)
        .any(|part| part.strip_prefix("sepia_hub=") == Some(encoded.as_str()))
}

/// Whether the request arrived over TLS — the absolute-form URI scheme
/// or `X-Forwarded-Proto` stands in (same convention as `sepia-http`).
fn request_is_https(req: &Request) -> bool {
    req.uri().scheme_str() == Some("https")
        || req
            .headers()
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            == Some("https")
}

/// Plant `sepia_hub=<token>; HttpOnly; …` on a response.
fn set_hub_cookie(res: &mut Response, token: &str, https: bool) {
    let secure = if https { "; Secure" } else { "" };
    let value = format!(
        "{HUB_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000{secure}",
        form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>()
    );
    if let Ok(v) = HeaderValue::from_str(&value) {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
}

fn unauthorized() -> Response {
    let mut res = (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response();
    res.headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    res
}

/// The browser gate — middleware over the whole router. No configured
/// token (loopback-only deployments) means everything passes.
pub async fn auth_gate(State(state): State<HubState>, req: Request<Body>, next: Next) -> Response {
    let Some(expected) = state.hub_token.as_deref().filter(|t| !t.is_empty()) else {
        return next.run(req).await;
    };
    if is_open_path(req.uri().path()) {
        return next.run(req).await;
    }
    let bearer_ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|p| p == expected);
    if bearer_ok || cookie_matches(&req, expected) {
        return next.run(req).await;
    }
    // `?token=` — a valid query credential authenticates the request and
    // plants the cookie, so `https://hub/?token=…` is a one-time step.
    if query_param(req.uri().query(), "token").is_some_and(|t| t == expected) {
        let https = request_is_https(&req);
        let mut res = next.run(req).await;
        set_hub_cookie(&mut res, expected, https);
        return res;
    }
    unauthorized()
}

/// `GET /login?token=…` — validates, plants the cookie, lands on `/`.
/// The path is pre-auth; the query token is the credential.
pub async fn login(State(state): State<HubState>, req: Request<Body>) -> Response {
    let provided = query_param(req.uri().query(), "token");
    match state.hub_token.as_deref().filter(|t| !t.is_empty()) {
        // No gate configured — a cookie would grant nothing; just land.
        None => Redirect::to("/").into_response(),
        Some(expected) if provided.as_deref() == Some(expected) => {
            let https = request_is_https(&req);
            let mut res = Redirect::to("/").into_response();
            set_hub_cookie(&mut res, expected, https);
            res
        }
        Some(_) => unauthorized(),
    }
}
