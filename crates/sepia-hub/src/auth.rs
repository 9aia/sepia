//! Browser → hub auth — the hub's own token surface. Node bearer tokens
//! stay server-side (`SEPIA_NODES` / `SEPIA_NODE_TOKEN`); this gate
//! covers everything the hub itself serves.
//!
//! When `SEPIA_HUB_TOKEN` is set, every non-asset route requires it:
//! `Authorization: Bearer`, a `?token=` query (EventSource can't set
//! headers — a valid query token also plants the cookie), or the
//! httpOnly `sepia_hub` cookie. Static assets — `style.css`,
//! `manifest.json`, `sw.js`, `icon.svg`, `/pkg/*` — stay open so the
//! PWA shell and service worker load before any credential exists.
//!
//! Browsers get a login flow, not bare 401s:
//! - HTML page navigations that fail auth redirect to `/login?next=…`.
//! - `/login` itself is the Leptos page; its form `POST`s the token
//!   back here — valid → cookie + 303 to `next`, invalid → 303 back
//!   to `/login?error=1`.
//! - `GET /login?token=<valid>` stays the one-time-link bootstrap:
//!   the gate plants the cookie and lands on `/`.
//! - `/api/*` and `/hub/*` keep the JSON 401 — non-page callers see
//!   an API-shaped error, never a redirect.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
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

/// Percent-encode one query/form value.
fn encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// The cookie is stored percent-encoded, so compare encodings — no
/// decoder needed (`byte_serialize` is deterministic).
fn cookie_matches(req: &Request, expected: &str) -> bool {
    let encoded = encode(expected);
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
        encode(token)
    );
    if let Ok(v) = HeaderValue::from_str(&value) {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
}

/// A page navigation — GET/HEAD on a non-API path asking for HTML.
/// Those earn the `/login` redirect; API, server-fn (`/hub/*`), and
/// non-HTML callers keep the JSON 401.
fn wants_html(req: &Request) -> bool {
    matches!(*req.method(), Method::GET | Method::HEAD)
        && !req.uri().path().starts_with("/api/")
        && !req.uri().path().starts_with("/hub/")
        && req
            .headers()
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/html"))
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
    let path = req.uri().path();
    // `GET /login?token=<valid>` — the credential bootstrap. Checked
    // before the open-path branch so a good token never reaches the
    // login page: plant the cookie, land on `/`. A *bad* token falls
    // through to the page, which reads the param as its error state.
    if path == "/login"
        && req.method() == Method::GET
        && query_param(req.uri().query(), "token").is_some_and(|t| t == expected)
    {
        let https = request_is_https(&req);
        let mut res = Redirect::to("/").into_response();
        set_hub_cookie(&mut res, expected, https);
        return res;
    }
    if is_open_path(path) {
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
    if wants_html(&req) {
        // Carry the destination so a successful login lands back here.
        let target = req.uri().path_and_query().map_or("/", |pq| pq.as_str());
        return Redirect::to(&format!("/login?next={}", encode(target))).into_response();
    }
    unauthorized()
}

/// `next` must be a local path — reject absolute/authority-relative
/// URLs and control characters so the post-login redirect can't leave
/// the origin (or smuggle a header).
fn is_safe_next(next: &str) -> bool {
    next.starts_with('/') && !next.starts_with("//") && !next.chars().any(char::is_control)
}

/// `POST /login` — the form's credential exchange. Accepts
/// `application/x-www-form-urlencoded` (`token`, optional `next`) or
/// JSON `{token, next?}`; a valid token plants `sepia_hub` and 303s to
/// `next` (default `/`), an invalid one 303s back to
/// `/login?error=1` with `next` preserved.
pub async fn login(State(state): State<HubState>, req: Request<Body>) -> Response {
    let https = request_is_https(&req);
    let is_json = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("json"));
    let bytes = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    let (provided, next) = if is_json {
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        (
            body.get("token")
                .and_then(|t| t.as_str())
                .map(str::to_string),
            body.get("next")
                .and_then(|t| t.as_str())
                .map(str::to_string),
        )
    } else {
        let get = |key: &str| {
            form_urlencoded::parse(&bytes)
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        (get("token"), get("next"))
    };
    match state.hub_token.as_deref().filter(|t| !t.is_empty()) {
        // No gate configured — a cookie would grant nothing; just land.
        None => Redirect::to("/").into_response(),
        Some(expected) if provided.as_deref() == Some(expected) => {
            let target = next
                .filter(|n| is_safe_next(n))
                .unwrap_or_else(|| "/".to_string());
            let mut res = Redirect::to(&target).into_response();
            set_hub_cookie(&mut res, expected, https);
            res
        }
        Some(_) => {
            let mut to = "/login?error=1".to_string();
            if let Some(n) = next.filter(|n| is_safe_next(n)) {
                to.push_str("&next=");
                to.push_str(&encode(&n));
            }
            Redirect::to(&to).into_response()
        }
    }
}
