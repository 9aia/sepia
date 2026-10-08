//! Mock ACP agent for tests — speaks enough of the protocol to exercise
//! the connection end to end: initialize, session/new, session/list,
//! session/prompt (emits an echo update + a permission request when the
//! prompt says "perm"), session/cancel.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn respond(id: u64, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn notify(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
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
                let _ = writeln!(
                    out,
                    "{}",
                    respond(
                        id,
                        json!({
                            "protocolVersion": 1,
                            "agentCapabilities": {
                                "loadSession": true,
                                "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false },
                                "sessionCapabilities": { "list": {}, "delete": {} }
                            }
                        })
                    )
                );
            }
            ("session/new", Some(id)) => {
                let _ = writeln!(
                    out,
                    "{}",
                    respond(id, json!({ "sessionId": "mock-session" }))
                );
            }
            ("session/load", Some(id)) | ("session/delete", Some(id)) => {
                let _ = writeln!(out, "{}", respond(id, json!({})));
            }
            ("session/list", Some(id)) => {
                let _ = writeln!(
                    out,
                    "{}",
                    respond(
                        id,
                        json!({
                            "sessions": [{
                                "sessionId": "s1",
                                "cwd": "/work",
                                "title": "Mock session",
                                "updatedAt": "2024-01-01T00:00:00Z"
                            }]
                        })
                    )
                );
            }
            ("session/prompt", Some(id)) => {
                let text = msg["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let _ = writeln!(
                    out,
                    "{}",
                    notify(
                        "session/update",
                        json!({
                            "sessionId": "mock-session",
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": format!("echo: {text}") }
                            }
                        })
                    )
                );
                if text == "perm" {
                    let _ = writeln!(
                        out,
                        "{}",
                        json!({
                            "jsonrpc": "2.0", "id": 900,
                            "method": "session/request_permission",
                            "params": {
                                "sessionId": "mock-session",
                                "toolCall": { "toolCallId": "tc-1", "title": "Run it" },
                                "options": [{ "optionId": "allow", "name": "Allow", "kind": "allow_once" }]
                            }
                        })
                    );
                }
                let _ = writeln!(out, "{}", respond(id, json!({ "stopReason": "end_turn" })));
            }
            ("session/request_permission", _) => {} // responses to our requests — no method
            ("session/cancel", _) => {}             // notification — no reply
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
