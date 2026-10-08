//! Boot-time configuration — port of `apps/server/src/env.ts` minus the
//! surfaces this crate doesn't serve (UI assets, OTEL, the servers
//! registry, the Devin DB — the driver binary opens that itself via
//! `SEPIA_DEVIN_DB`). Plane tunables `sepia-control`/`sepia-acp` read
//! directly (`SEPIA_LOCK_TTL_MS`, `SEPIA_HISTORY_LIMIT`,
//! `SEPIA_INHERIT_ENV`, `SEPIA_DEBUG`) deliberately don't ride `Env`.
//! `Env::parse` fails fast on bad values, like the TS schema.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8787;
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_SSE_KEEPALIVE_MS: u64 = 15_000;
pub const DEFAULT_HELD_WATCH_MS: u64 = 5_000;
pub const DEFAULT_IDLE_TTL_MS: u64 = 600_000;
pub const DEFAULT_SWEEP_MS: u64 = 30_000;

const DEFAULT_ORIGINS: [&str; 2] = ["http://localhost:3000", "http://127.0.0.1:3000"];

/// A boot-time configuration error — message mirrors the TS throw text.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct EnvError(pub String);

/// Parsed `SEPIA_*` environment — every field `env.ts`/`serve.ts` reads.
#[derive(Clone, Debug)]
pub struct Env {
    /// `SEPIA_PORT` (falls back to the TS `PORT`).
    pub port: u16,
    /// `SEPIA_HOST`.
    pub host: String,
    /// `SEPIA_TOKEN` — bearer auth; required on non-loopback binds.
    pub token: Option<String>,
    /// `SEPIA_META`/`SEPIA_META_PATH` — the overlay JSON file.
    pub meta_path: PathBuf,
    /// `SEPIA_HOME` — Sepia-owned data dir (node.json, meta.json, tokens).
    pub home: PathBuf,
    /// `SEPIA_NODE` — stable node identity file.
    pub node_path: PathBuf,
    /// `SEPIA_NAME` — display name reported by `GET /api/node`.
    pub node_name: String,
    /// `SEPIA_ORIGINS` — CORS allowlist (`*` echoes any origin).
    pub origins: Vec<String>,
    /// `SEPIA_CLINE_DIR` (default `~/.cline/data`) — the convert target.
    pub cline_dir: PathBuf,
    /// `SEPIA_CLAUDE_DIR` (default `~/.claude`) — file-history restore.
    pub claude_dir: PathBuf,
    /// `SEPIA_SSE_KEEPALIVE_MS` — keep-alive cadence; `0` disables.
    pub sse_keep_alive: Duration,
    /// `SEPIA_HELD_WATCH_MS` — held-session re-probe cadence; `0` disables.
    pub held_watch: Duration,
    /// `SEPIA_IDLE_TTL_MS` — idle live-session detach (plane option).
    pub idle_ttl: Duration,
    /// `SEPIA_SWEEP_MS` — idle sweep cadence (plane option).
    pub sweep: Duration,
    /// `<home>/pair-code` — written by `sepia pair`, consumed on read.
    pub pair_code_path: PathBuf,
    /// `<home>/tokens.json` — sha256 hashes of issued pair credentials.
    pub tokens_path: PathBuf,
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn hostname_or_default() -> String {
    nix::unistd::gethostname()
        .ok()
        .map(|h| h.to_string_lossy().into_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// `serve.ts`'s bind guard — loopback binds may go tokenless.
pub fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host == "::1" || host == "[::1]" || host.starts_with("127.")
}

/// `SEPIA_SSE_KEEPALIVE_MS`/`SEPIA_HELD_WATCH_MS` style parse: absent →
/// default; a non-finite or negative value falls back to the default;
/// `0` disables.
fn ms_or_default(raw: Option<&String>, default: u64) -> u64 {
    let Some(raw) = raw else { return default };
    if raw.is_empty() {
        return default;
    }
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => v.floor() as u64,
        _ => default,
    }
}

fn millis_u64(raw: Option<&String>, default: u64) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok()).unwrap_or(default)
}

impl Env {
    /// Parse from the process environment; fails fast on bad values.
    ///
    /// # Errors
    /// `EnvError` on a bad port or a non-loopback bind without
    /// `SEPIA_TOKEN`.
    pub fn parse() -> Result<Self, EnvError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Test seam — parse from a key/value map instead of `std::env`.
    pub fn from_map(vars: &HashMap<String, String>) -> Result<Self, EnvError> {
        Self::from_lookup(|key| vars.get(key).cloned())
    }

    /// Shared parser behind [`Env::parse`]/[`Env::from_map`].
    ///
    /// # Errors
    /// See [`Env::parse`].
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, EnvError> {
        let home_dir = home_dir();

        // TS reads PORT; the Rust surface prefers the namespaced var.
        let raw_port = get("SEPIA_PORT").or_else(|| get("PORT"));
        let port: u16 = match raw_port.as_deref() {
            None | Some("") => DEFAULT_PORT,
            Some(raw) => match raw.parse::<f64>() {
                Ok(v) if v.fract() == 0.0 && (1.0..=65535.0).contains(&v) => v as u16,
                _ => {
                    return Err(EnvError(format!(
                        "PORT must be an integer between 1 and 65535, got \"{raw}\""
                    )));
                }
            },
        };

        let origins: Vec<String> = get("SEPIA_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let origins = if origins.is_empty() {
            DEFAULT_ORIGINS.iter().map(|s| (*s).to_string()).collect()
        } else {
            origins
        };

        let home =
            get("SEPIA_HOME").map_or_else(|| home_dir.join(".local/share/sepia"), PathBuf::from);

        let host = get("SEPIA_HOST").unwrap_or_else(|| DEFAULT_HOST.to_string());
        let token = get("SEPIA_TOKEN");

        // serve.ts's bind guard, enforced at parse so a bad deployment
        // fails fast at construction rather than mid-request.
        if !is_loopback_host(&host) && token.as_deref().unwrap_or("").is_empty() {
            return Err(EnvError(format!(
                "sepia-server refuses to bind {host} without SEPIA_TOKEN. Set SEPIA_TOKEN or bind a loopback address (SEPIA_HOST=127.0.0.1)."
            )));
        }

        let node_name = get("SEPIA_NAME")
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(hostname_or_default);

        Ok(Self {
            port,
            host,
            token,
            meta_path: get("SEPIA_META_PATH")
                .or_else(|| get("SEPIA_META"))
                .map_or_else(|| home.join("meta.json"), PathBuf::from),
            node_path: get("SEPIA_NODE").map_or_else(|| home.join("node.json"), PathBuf::from),
            node_name,
            origins,
            cline_dir: get("SEPIA_CLINE_DIR")
                .map_or_else(|| home_dir.join(".cline/data"), PathBuf::from),
            claude_dir: get("SEPIA_CLAUDE_DIR")
                .map_or_else(|| home_dir.join(".claude"), PathBuf::from),
            sse_keep_alive: Duration::from_millis(ms_or_default(
                get("SEPIA_SSE_KEEPALIVE_MS").as_ref(),
                DEFAULT_SSE_KEEPALIVE_MS,
            )),
            held_watch: Duration::from_millis(ms_or_default(
                get("SEPIA_HELD_WATCH_MS").as_ref(),
                DEFAULT_HELD_WATCH_MS,
            )),
            idle_ttl: Duration::from_millis(millis_u64(
                get("SEPIA_IDLE_TTL_MS").as_ref(),
                DEFAULT_IDLE_TTL_MS,
            )),
            sweep: Duration::from_millis(millis_u64(
                get("SEPIA_SWEEP_MS").as_ref(),
                DEFAULT_SWEEP_MS,
            )),
            pair_code_path: home.join("pair-code"),
            tokens_path: home.join("tokens.json"),
            home,
        })
    }
}
