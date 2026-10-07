//! Boot-time configuration — mirrors `apps/server/src/env.ts`: every value
//! is validated here and a bad one fails fast with a clear message.

use std::path::PathBuf;

const DEFAULT_ORIGINS: [&str; 2] = ["http://localhost:3000", "http://127.0.0.1:3000"];

#[derive(Debug, Clone)]
pub struct ServerEnv {
    pub db_path: PathBuf,
    pub port: u16,
    pub host: String,
    pub token: Option<String>,
    pub meta_path: PathBuf,
    /// Sepia-owned data dir — node.json, meta.json.
    pub home: PathBuf,
    /// Stable node identity file.
    pub node_path: PathBuf,
    /// Display name reported by GET /api/node; defaults to the hostname.
    pub node_name: String,
    /// Encrypted managed-server registry (gateway peers' credentials).
    pub servers_path: PathBuf,
    /// 256-bit hex key file encrypting `servers_path` (`SEPIA_SERVERS_KEY` overrides).
    pub servers_key_path: PathBuf,
    pub origins: Vec<String>,
    /// Static UI serving: the embedded web bundle, or `SEPIA_UI_DIR` to override.
    pub ui: UiConfig,
    pub otel: OtelConfig,
    /// Agent store overlays (read-only by default).
    pub cline_dir: PathBuf,
    pub claude_dir: PathBuf,
    pub cursor_dir: PathBuf,
    pub history_limit: usize,
    pub held_watch_ms: u64,
    pub idle_ttl_ms: u64,
    pub sweep_ms: u64,
    pub sse_keepalive_ms: u64,
    pub inherit_env: bool,
    pub debug: bool,
}

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub enabled: bool,
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct OtelConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub service_name: String,
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|v| v.trim().to_string())
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn env_num(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
    default: u64,
) -> Result<u64, String> {
    match lookup(name) {
        None => Ok(default),
        Some(raw) if raw.is_empty() => Ok(default),
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| format!("{name} must be a non-negative integer, got \"{raw}\"")),
    }
}

/// Parses and validates boot-time configuration; a bad value fails fast.
pub fn parse_env() -> Result<ServerEnv, String> {
    parse_env_from(|key| std::env::var(key).ok())
}

/// Same as `parse_env` but reads through `lookup` — tests inject maps.
pub fn parse_env_from(lookup: impl Fn(&str) -> Option<String>) -> Result<ServerEnv, String> {
    let home =
        lookup("SEPIA_HOME").map_or_else(|| home_dir().join(".local/share/sepia"), PathBuf::from);

    let db_path = lookup("SEPIA_DB").map_or_else(
        || home_dir().join(".local/share/devin/cli/sessions.db"),
        PathBuf::from,
    );
    if db_path.as_os_str().is_empty() {
        return Err("SEPIA_DB must not be empty".to_string());
    }

    let port: u16 = match lookup("PORT") {
        None => 8787,
        Some(raw) if raw.is_empty() => 8787,
        Some(raw) => raw
            .parse::<u16>()
            .map_err(|_| format!("PORT must be an integer between 1 and 65535, got \"{raw}\""))?,
    };

    let configured: Vec<String> = lookup("SEPIA_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
        .collect();

    let otel_endpoint = lookup("OTEL_EXPORTER_OTLP_ENDPOINT").map_or_else(
        || "http://localhost:4318".to_string(),
        |v| v.trim_end_matches('/').to_string(),
    );
    if lookup("SEPIA_OTEL").as_deref() != Some("0")
        && !otel_endpoint.starts_with("http://")
        && !otel_endpoint.starts_with("https://")
    {
        return Err(format!(
            "OTEL_EXPORTER_OTLP_ENDPOINT must be an http(s) URL, got \"{otel_endpoint}\""
        ));
    }

    // SEPIA_UI=off (or 0/false) makes this an API-only node; SEPIA_UI_DIR
    // points at an alternate web bundle on disk instead of the embedded one.
    let ui_flag = lookup("SEPIA_UI").map(|v| v.trim().to_lowercase());
    let xdg_config =
        lookup("XDG_CONFIG_HOME").map_or_else(|| home_dir().join(".config"), PathBuf::from);

    Ok(ServerEnv {
        db_path,
        port,
        host: lookup("SEPIA_HOST").unwrap_or_else(|| "127.0.0.1".to_string()),
        token: lookup("SEPIA_TOKEN").filter(|v| !v.is_empty()),
        meta_path: lookup("SEPIA_META").map_or_else(|| home.join("meta.json"), PathBuf::from),
        node_path: lookup("SEPIA_NODE").map_or_else(|| home.join("node.json"), PathBuf::from),
        node_name: lookup("SEPIA_NAME")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(hostname),
        servers_path: lookup("SEPIA_SERVERS")
            .map_or_else(|| home.join("servers.json"), PathBuf::from),
        servers_key_path: lookup("SEPIA_SERVERS_KEY_PATH")
            .map_or_else(|| xdg_config.join("sepia/servers.key"), PathBuf::from),
        origins: if configured.is_empty() {
            DEFAULT_ORIGINS.iter().map(|s| (*s).to_string()).collect()
        } else {
            configured
        },
        ui: UiConfig {
            enabled: !matches!(ui_flag.as_deref(), Some("0" | "off" | "false")),
            dir: lookup("SEPIA_UI_DIR").map(PathBuf::from),
        },
        otel: OtelConfig {
            enabled: lookup("SEPIA_OTEL").as_deref() != Some("0"),
            endpoint: otel_endpoint,
            service_name: lookup("OTEL_SERVICE_NAME").unwrap_or_else(|| "sepia-server".to_string()),
        },
        cline_dir: lookup("SEPIA_CLINE_DIR")
            .map_or_else(|| home_dir().join(".cline/data"), PathBuf::from),
        claude_dir: lookup("SEPIA_CLAUDE_DIR")
            .map_or_else(|| home_dir().join(".claude"), PathBuf::from),
        cursor_dir: lookup("SEPIA_CURSOR_DIR")
            .map_or_else(|| home_dir().join(".cursor"), PathBuf::from),
        history_limit: usize::try_from(env_num(&lookup, "SEPIA_HISTORY_LIMIT", 500)?)
            .map_err(|_| "SEPIA_HISTORY_LIMIT too large".to_string())?,
        held_watch_ms: env_num(&lookup, "SEPIA_HELD_WATCH_MS", 5_000)?,
        idle_ttl_ms: env_num(&lookup, "SEPIA_IDLE_TTL_MS", 300_000)?,
        sweep_ms: env_num(&lookup, "SEPIA_SWEEP_MS", 30_000)?,
        sse_keepalive_ms: env_num(&lookup, "SEPIA_SSE_KEEPALIVE_MS", 15_000)?,
        inherit_env: lookup("SEPIA_INHERIT_ENV").is_some_and(|v| v == "1"),
        debug: lookup("SEPIA_DEBUG").is_some_and(|v| v == "1"),
        home,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(pairs: &[(&str, &str)]) -> ServerEnv {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        parse_env_from(|k| map.get(k).cloned()).unwrap_or_else(|e| panic!("parse failed: {e}"))
    }

    #[test]
    fn defaults_are_loopback_and_devin_db() {
        let env = env_with(&[]);
        assert_eq!(env.port, 8787);
        assert_eq!(env.host, "127.0.0.1");
        assert!(env.ui.enabled);
        assert!(env.token.is_none());
        assert!(env.db_path.ends_with("devin/cli/sessions.db"));
        assert_eq!(
            env.origins,
            vec!["http://localhost:3000", "http://127.0.0.1:3000"]
        );
    }

    #[test]
    fn bad_port_fails() {
        let err = parse_env_from(|k| (k == "PORT").then(|| "abc".to_string()));
        assert!(err.is_err_and(|e| e.contains("PORT")));
    }

    #[test]
    fn non_loopback_requires_token_is_callers_check() {
        let env = env_with(&[("SEPIA_HOST", "0.0.0.0"), ("SEPIA_TOKEN", "t")]);
        assert_eq!(env.host, "0.0.0.0");
    }

    #[test]
    fn ui_flag_disables() {
        for flag in ["0", "off", "false"] {
            let env = env_with(&[("SEPIA_UI", flag)]);
            assert!(!env.ui.enabled);
        }
    }

    #[test]
    fn bad_otel_endpoint_fails() {
        let err =
            parse_env_from(|k| (k == "OTEL_EXPORTER_OTLP_ENDPOINT").then(|| "ws://x".to_string()));
        assert!(err.is_err_and(|e| e.contains("OTEL_EXPORTER_OTLP_ENDPOINT")));
    }

    #[test]
    fn numeric_envs_parse() {
        let env = env_with(&[
            ("SEPIA_HELD_WATCH_MS", "1234"),
            ("SEPIA_HISTORY_LIMIT", "42"),
        ]);
        assert_eq!(env.held_watch_ms, 1234);
        assert_eq!(env.history_limit, 42);
    }

    #[test]
    fn bad_numeric_env_fails() {
        let err = parse_env_from(|k| (k == "SEPIA_SWEEP_MS").then(|| "nope".to_string()));
        assert!(err.is_err_and(|e| e.contains("SEPIA_SWEEP_MS")));
    }
}
