#![allow(clippy::pedantic)]

//! Mock ACP agent for tests — speaks enough of the protocol to exercise
//! the connection and control plane end to end.
//!
//! Environment-driven behavior:
//! - `MOCK_CAPS`: comma-separated capability flags to advertise
//!   (`load,list,delete,image,audio,resource`; default
//!   `load,list,delete,image`).
//! - `MOCK_SESSIONS`: `;`-separated `id|cwd|title|updatedAt|locked|pid`
//!   rows `session/list` reports (locked/pid optional).
//! - `MOCK_LOAD_FAIL` / `MOCK_NEW_FAIL` / `MOCK_PROMPT_FAIL` /
//!   `MOCK_DELETE_FAIL`: the method returns that message as an error.
//! - `MOCK_LOAD_FAIL_ONCE`: `session/load` fails once, then succeeds.
//! - `MOCK_PERM_TEXT`: the prompt text that triggers a
//!   `session/request_permission` request (default `perm`).
//! - `MOCK_EXIT_AFTER_INIT`: exit(0) right after initialize.
//! - `MOCK_EXIT_AFTER_PROMPT`: exit(1) instead of answering the first
//!   session/prompt — mid-run agent death.
//! - `MOCK_BAD_UPDATES=1`: send a garbage update line + a truncated
//!   JSON line before answering the first session/prompt.
//! - `MOCK_LOAD_SLOW_MS` / `MOCK_LIST_SLOW_MS`: per-call delay.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn respond(id: u64, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error(id: u64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
        .to_string()
}

fn notify(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn caps() -> Vec<String> {
    env("MOCK_CAPS")
        .map(|v| v.split(',').map(str::trim).map(str::to_string).collect())
        .unwrap_or_else(|| {
            ["load", "list", "delete", "image"]
                .iter()
                .map(ToString::to_string)
                .collect()
        })
}

fn sessions() -> Value {
    let rows = env("MOCK_SESSIONS")
        .map(|v| {
            v.split(';')
                .filter(|row| !row.is_empty())
                .map(|row| {
                    let cols: Vec<&str> = row.split('|').collect();
                    let mut session = json!({
                        "sessionId": cols.first().copied().unwrap_or_default(),
                        "cwd": cols.get(1).copied().unwrap_or_default(),
                        "title": cols.get(2).copied().unwrap_or_default(),
                        "updatedAt": cols.get(3).copied().unwrap_or_default(),
                    });
                    if cols.get(4) == Some(&"locked") {
                        session["locked"] = json!(true);
                        if let Some(pid) = cols.get(5).and_then(|p| p.parse::<u64>().ok()) {
                            session["_meta"] = json!({
                                "cognition.ai/isLocked": true,
                                "cognition.ai/lockHolderPid": pid,
                            });
                        }
                    }
                    session
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            vec![json!({
                "sessionId": "s1",
                "cwd": "/work",
                "title": "Mock session",
                "updatedAt": "2024-01-01T00:00:00Z"
            })]
        });
    json!(rows)
}

fn sleep_ms(env_key: &str) {
    if let Some(ms) = env(env_key).and_then(|v| v.parse::<u64>().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let capabilities = caps();
    let perm_text = env("MOCK_PERM_TEXT").unwrap_or_else(|| "perm".into());
    let mut load_failed_once = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = msg["method"].as_str().unwrap_or_default();
        let id = msg["id"].as_u64();
        let mut out = stdout.lock();
        match (method, id) {
            ("initialize", Some(id)) => {
                let has = |cap: &str| capabilities.iter().any(|c| c == cap);
                let mut session_caps = json!({});
                if has("list") {
                    session_caps["list"] = json!({});
                }
                if has("delete") {
                    session_caps["delete"] = json!({});
                }
                let _ = writeln!(
                    out,
                    "{}",
                    respond(
                        id,
                        json!({
                            "protocolVersion": 1,
                            "agentCapabilities": {
                                "loadSession": has("load"),
                                "promptCapabilities": {
                                    "image": has("image"),
                                    "audio": has("audio"),
                                    "embeddedContext": has("resource")
                                },
                                "sessionCapabilities": session_caps
                            }
                        })
                    )
                );
                if env("MOCK_EXIT_AFTER_INIT").is_some() {
                    return;
                }
            }
            ("session/new", Some(id)) => {
                let line = match env("MOCK_NEW_FAIL") {
                    Some(message) => error(id, &message),
                    None => respond(
                        id,
                        json!({ "sessionId": env("MOCK_SESSION_ID").unwrap_or_else(|| "mock-session".into()) }),
                    ),
                };
                let _ = writeln!(out, "{line}");
            }
            ("session/load", Some(id)) => {
                sleep_ms("MOCK_LOAD_SLOW_MS");
                let fail = env("MOCK_LOAD_FAIL")
                    .filter(|_| !load_failed_once || env("MOCK_LOAD_FAIL_ONCE").is_none());
                let line = match (env("MOCK_LOAD_FAIL_ONCE"), fail) {
                    (Some(message), _) if !load_failed_once => {
                        load_failed_once = true;
                        error(id, &message)
                    }
                    (None, Some(message)) => error(id, &message),
                    _ => respond(id, json!({})),
                };
                let _ = writeln!(out, "{line}");
            }
            ("session/delete", Some(id)) => {
                let line = match env("MOCK_DELETE_FAIL") {
                    Some(message) => error(id, &message),
                    None => respond(id, json!({})),
                };
                let _ = writeln!(out, "{line}");
            }
            ("session/list", Some(id)) => {
                sleep_ms("MOCK_LIST_SLOW_MS");
                let _ = writeln!(out, "{}", respond(id, json!({ "sessions": sessions() })));
            }
            ("session/prompt", Some(id)) => {
                sleep_ms("MOCK_PROMPT_SLOW_MS");
                if env("MOCK_BAD_UPDATES").is_some() {
                    let _ = writeln!(out, "{{ this is not json");
                    let _ = writeln!(
                        out,
                        "{}",
                        notify(
                            "session/update",
                            json!({
                                "sessionId": msg["params"]["sessionId"],
                                "update": { "sessionUpdate": "agent_message_chunk",
                                            "content": 42 }
                            })
                        )
                    );
                    let _ = writeln!(out, r#"{{"partial": "#);
                }
                if env("MOCK_EXIT_AFTER_PROMPT").is_some() {
                    let _ = out.flush();
                    std::process::exit(1);
                }
                let text = msg["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let session_id = msg["params"]["sessionId"]
                    .as_str()
                    .unwrap_or("s1")
                    .to_string();
                let _ = writeln!(
                    out,
                    "{}",
                    notify(
                        "session/update",
                        json!({
                            "sessionId": session_id,
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": format!("echo: {text}") }
                            }
                        })
                    )
                );
                if text == perm_text {
                    let _ = writeln!(
                        out,
                        "{}",
                        json!({
                            "jsonrpc": "2.0", "id": 900,
                            "method": "session/request_permission",
                            "params": {
                                "sessionId": session_id,
                                "toolCall": { "toolCallId": "tc-1", "title": "Run it" },
                                "options": [{ "optionId": "allow", "name": "Allow", "kind": "allow_once" }]
                            }
                        })
                    );
                }
                let line = match env("MOCK_PROMPT_FAIL") {
                    Some(message) => error(id, &message),
                    None => respond(id, json!({ "stopReason": "end_turn" })),
                };
                let _ = writeln!(out, "{line}");
            }
            ("session/cancel", _) => {} // notification — no reply
            (_, Some(id)) => {
                let _ = writeln!(
                    out,
                    "{}",
                    json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("unknown {method}") }
                    })
                );
            }
            _ => {}
        }
        let _ = out.flush();
    }
}
