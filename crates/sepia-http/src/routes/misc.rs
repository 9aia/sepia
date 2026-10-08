//! `routes/misc.ts` + `routes/fs.ts` + `routes/node.ts` — health probe,
//! directory listing for the cwd picker, node descriptor, user info.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::json;

use crate::AppState;
use crate::node::PROTOCOL_VERSION;
use crate::routes::{json_response, not_found, query_param};

/// `GET /api/health` — deliberately ahead of the auth gate. The TS
/// health check is a 1.5s-bounded session list.
const HEALTH_TIMEOUT: Duration = Duration::from_millis(1_500);

pub async fn health(
    State(state): State<AppState>,
    method: Method,
    _req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let healthy = match tokio::time::timeout(HEALTH_TIMEOUT, state.plane.list_sessions(false)).await
    {
        Ok(Ok(_)) => true,
        Ok(Err(_)) | Err(_) => false,
    };
    if healthy {
        json_response(json!({ "ok": true, "db": true }), StatusCode::OK)
    } else {
        json_response(
            json!({ "ok": false, "db": false }),
            StatusCode::SERVICE_UNAVAILABLE,
        )
    }
}

/// `GET /api/fs?path=/abs/dir` — direct children of a directory, for the
/// composer's working-directory picker. Directories only, capped at 200.
pub async fn fs(State(_state): State<AppState>, method: Method, req: Request<Body>) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let path = query_param(req.uri().query(), "path").unwrap_or_default();
    if !path.starts_with('/') {
        return json_response(
            json!({ "error": "path must be absolute" }),
            StatusCode::BAD_REQUEST,
        );
    }
    let read = std::fs::read_dir(&path);
    match read {
        Err(_) => json_response(
            json!({ "error": "Cannot read that directory" }),
            StatusCode::BAD_REQUEST,
        ),
        Ok(entries) => {
            let mut dirs: Vec<String> = entries
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path().to_string_lossy().into_owned())
                .collect();
            dirs.sort();
            dirs.truncate(200);
            json_response(json!({ "dirs": dirs }), StatusCode::OK)
        }
    }
}

/// `process.platform`/`process.arch` under Node's names.
fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => match other {
            "linux" => "linux",
            "freebsd" => "freebsd",
            "openbsd" => "openbsd",
            "netbsd" => "netbsd",
            "android" => "android",
            o => o,
        },
    }
}

fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "arm" => "arm",
        "x86" => "ia32",
        other => match other {
            "riscv64" => "riscv64",
            "powerpc64" => "ppc64",
            "s390x" => "s390x",
            o => o,
        },
    }
}

fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .map(|h| h.to_string_lossy().into_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// `GET /api/user` — the OS user the node runs as (`os.userInfo()`).
pub async fn user(State(_state): State<AppState>, method: Method, _req: Request<Body>) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let info = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
        .ok()
        .flatten();
    let (username, homedir, shell) = info.map_or_else(
        || {
            (
                std::env::var("USER").unwrap_or_default(),
                std::env::var("HOME").unwrap_or_default(),
                std::env::var("SHELL").unwrap_or_default(),
            )
        },
        |u| {
            (
                u.name,
                u.dir.to_string_lossy().into_owned(),
                u.shell.to_string_lossy().into_owned(),
            )
        },
    );
    json_response(
        json!({
            "user": {
                "username": username,
                "homedir": homedir,
                "shell": shell,
                "hostname": hostname(),
                "platform": node_platform(),
                "arch": node_arch(),
            }
        }),
        StatusCode::OK,
    )
}

/// `GET /api/node` — the node descriptor every federated client hits
/// first (docs/protocol.md).
pub async fn node(State(state): State<AppState>, method: Method, _req: Request<Body>) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let agents: Vec<String> = state
        .plane
        .list_agents()
        .await
        .into_iter()
        .map(|a| a.id)
        .collect();
    let mut capabilities = vec![
        "sessions", "projects", "push", "events", "export", "transfer",
    ];
    if state.pairing.is_some() {
        capabilities.push("pairing");
    }
    json_response(
        json!({
            "id": state.node.id,
            "name": state.node.name,
            "version": state.node.version,
            "protocol": PROTOCOL_VERSION,
            "agents": agents,
            "capabilities": capabilities,
        }),
        StatusCode::OK,
    )
}
