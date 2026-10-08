//! `POST /api/client/keypair` — port of `routes/client.ts`: a client on
//! a non-secure context (http:// over LAN) has no `crypto.subtle`, so it
//! can't generate its Ed25519 identity; the node mints one server-side.
//! Authenticated like every other /api route — the secret transits the
//! wire, so on plaintext LAN this is only as private as the transport.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;

use crate::AppState;
use crate::routes::{json_response, not_found};

/// Mint an Ed25519 keypair; returns `{algorithm, publicKey, secretKey}`
/// — the JWK `d` is the 32-byte private seed, base64url (no pad).
fn mint_ed25519() -> Response {
    let mut seed = [0u8; 32];
    // uuid v4 is OS-CSPRNG backed — two draws fill the 32-byte seed.
    seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let public = signing.verifying_key().to_bytes();
    json_response(
        json!({
            "algorithm": "Ed25519",
            "publicKey": URL_SAFE_NO_PAD.encode(public),
            "secretKey": URL_SAFE_NO_PAD.encode(seed),
        }),
        StatusCode::OK,
    )
}

/// TS tries Ed25519 then ECDSA P-256 — in Rust Ed25519 always works, so
/// the fallback branch is unreachable.
pub async fn keypair(
    State(_state): State<AppState>,
    method: Method,
    _req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    mint_ed25519()
}
