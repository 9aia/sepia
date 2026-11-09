//! views — the driver's manifest: every ViewSpec plus the query
//! resolvers that feed them.
//!
//! Read path: every query is one or two calls against `sepia serve`
//! through [`crate::data::call`] — the session plane's own wire, so
//! the driver sees exactly what orchestrators see and can never
//! mutate anything (only read-side methods are issued). A failed
//! call degrades to an `{"error": …}` payload inside an `ok:true`
//! response, matching the sibling drivers' contract.
//!
//! Surface mapping (what `sepia serve` actually exposes — see
//! `crates/sepia-cli/src/serve.rs` for the method table):
//!
//! | pavo view      | serve methods                          |
//! |----------------|----------------------------------------|
//! | `sepia:status` | `manifest` + `sessions/agents/projects.list` counts |
//! | `sepia:sessions` | `sessions.list` (`withLocks`)        |
//! | `sepia:session`  | `sessions.summary` + `sessions.history` |
//! | `sepia:agents`   | `agents.list` + `config.list`        |
//! | `sepia:projects` | `projects.list` + `sessions.list`    |
//!
//! Mutations (`attach`, `prompt`, `patch_meta`, `import`, …) stay on
//! the `sepia` CLI / `sepia serve` consumers — this driver is
//! strictly read-only.

use pavo_driver_kit as kit;
use pavo_protocol::{DriverManifest, ViewNode, ViewSpec};
use serde_json::{Map, Value, json};

use crate::data;

// ---------------------------------------------------------------- helpers

fn col(title: &str, children: Vec<ViewNode>) -> ViewNode {
    kit::column(kit::base().title(title), children)
}

fn kv(title: &str, query: &str) -> ViewNode {
    kit::kv(kit::base().title(title).query(query), Vec::new())
}

fn table(title: &str, query: &str, columns: &[(&str, &str)]) -> ViewNode {
    kit::sortable_table(
        kit::base().title(title).query(query),
        columns
            .iter()
            .map(|(key, t)| kit::table_column(key, t))
            .collect(),
        Vec::new(),
    )
}

fn spec(
    id: &str,
    title: &str,
    route: &str,
    description: &str,
    queries: &[&str],
    root: ViewNode,
) -> ViewSpec {
    let mut view = kit::view(id, title, route, root).description(description);
    for q in queries {
        view = view.query(kit::binding(q, "query"));
    }
    view.build()
}

// ---------------------------------------------------------------- manifest

const SESSION_COLS: &[(&str, &str)] = &[
    ("id", "ID"),
    ("title", "Title"),
    ("agent", "Agent"),
    ("status", "Status"),
    ("cwd", "Cwd"),
    ("updated", "Updated"),
    ("source", "Source"),
];

const HISTORY_COLS: &[(&str, &str)] = &[
    ("role", "Role"),
    ("time", "Time"),
    ("node", "Node"),
    ("tool", "Tool"),
    ("content", "Content"),
];

const AGENT_COLS: &[(&str, &str)] = &[
    ("id", "ID"),
    ("label", "Label"),
    ("runtime", "Runtime"),
    ("skills", "Skills"),
    ("rules", "Rules"),
    ("commands", "Commands"),
    ("hooks", "Hooks"),
    ("mcpServers", "MCP"),
    ("error", "Error"),
];

const PROJECT_COLS: &[(&str, &str)] = &[("id", "ID"), ("name", "Name"), ("sessions", "Sessions")];

/// Every view the driver declares — static (no sepia state needed;
/// `manifest`/`views` must answer anywhere).
pub fn specs() -> Vec<ViewSpec> {
    vec![
        spec(
            "status",
            "Status",
            "/sepia/status",
            "Plane digest — node identity, protocol version, and live counts of sessions, agents, and projects.",
            &["summary"],
            col("Sepia status", vec![kv("Plane", "summary")]),
        ),
        spec(
            "sessions",
            "Sessions",
            "/sepia/sessions",
            "Every session the plane knows — store rows plus live attaches — with busy/overlay flags. Lock-holder probing stays off (`sessions.list` without `withLocks`) so the view answers inside a host's request budget; the probe path can hit a store driver's call timeout. Filter with --param agent=.",
            &["sessions"],
            table("Sessions", "sessions", SESSION_COLS),
        ),
        spec(
            "session",
            "Session",
            "/sepia/session",
            "One session — summary fields plus the paged message history as an events timeline. Requires --param id=<session-id> (--param agent= when ids collide across stores).",
            &["session", "history"],
            col(
                "Session",
                vec![
                    kv("Session", "session"),
                    table("History", "history", HISTORY_COLS),
                ],
            ),
        ),
        spec(
            "agents",
            "Agents",
            "/sepia/agents",
            "Runnable ACP agents plus their config-IR item counts (skills/rules/commands/hooks/MCP servers) from `config.list`.",
            &["agents"],
            table("Agents", "agents", AGENT_COLS),
        ),
        spec(
            "projects",
            "Projects",
            "/sepia/projects",
            "Overlay projects with the count of sessions tagged to each.",
            &["projects"],
            table("Projects", "projects", PROJECT_COLS),
        ),
    ]
}

/// The `manifest` op result.
pub fn manifest() -> DriverManifest {
    // `kit::contribution` defaults priority to 100 — the chrome order
    // this driver wants is explicit, so set it on the public field.
    let mut navbar = kit::contribution("sessions", "Sepia", "/sepia/sessions");
    navbar.priority = 20;
    let mut dashboard = kit::contribution("status", "Sepia status", "/sepia/status");
    dashboard.priority = 30;
    let mut statusbar = kit::contribution("status", "Sepia sessions", "/sepia/status");
    statusbar.priority = 40;

    let mut m = kit::Manifest::new("sepia", "Sepia")
        .version(env!("CARGO_PKG_VERSION"))
        .icon("🐙")
        .navbar(navbar)
        .dashboard(dashboard)
        .statusbar(statusbar);
    for spec in specs() {
        m = m.view(spec);
    }
    m.build()
}

// ---------------------------------------------------------------- queries

/// Every query name the driver answers — the `query` op rejects
/// anything outside this set with `ok:false`.
pub fn known_query(name: &str) -> bool {
    matches!(
        name,
        "summary" | "sessions" | "session" | "history" | "agents" | "projects"
    )
}

/// `{"op":"query","query":name,"params":{…}}` → the data payload.
/// Failures degrade to `{"error": …}` objects, never `ok:false`.
pub fn query_data(name: &str, params: &Map<String, Value>) -> Value {
    match name {
        "summary" => summary(params),
        "sessions" => sessions_rows(params),
        "session" => session_detail(params),
        "history" => history_rows(params),
        "agents" => agents_rows(params),
        "projects" => projects_rows(params),
        other => json!({"error": format!("unknown query '{other}'")}),
    }
}

/// One serve call, degraded to an error payload on failure.
fn serve(method: &str, params: Value, ctx: &Map<String, Value>) -> Result<Value, Value> {
    data::call(method, params, ctx).map_err(|e| json!({"error": e}))
}

/// `agent`/`dir` view params forwarded into serve params when the
/// method accepts them.
fn agent_params(ctx: &Map<String, Value>) -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(agent) = data::agent_param(ctx) {
        params.insert("agent".to_string(), json!(agent));
    }
    params
}

/// `summary` query — the status view's kv object: plane identity
/// from `manifest` plus live row counts. Individual count failures
/// degrade to their error string rather than failing the panel.
fn summary(ctx: &Map<String, Value>) -> Value {
    let manifest = match serve("manifest", json!({}), ctx) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let count = |method: &str| -> Value {
        serve(method, json!({}), ctx).map_or_else(
            |e| e["error"].clone(),
            |v| json!(v.as_array().map_or(0, Vec::len)),
        )
    };
    let mut out = Map::new();
    out.insert("name".into(), manifest["name"].clone());
    out.insert("version".into(), manifest["version"].clone());
    out.insert("protocol".into(), manifest["protocol"].clone());
    out.insert("node".into(), manifest["node"].clone());
    out.insert(
        "methods".into(),
        json!(manifest["methods"].as_array().map_or(0, Vec::len)),
    );
    out.insert("sessions".into(), count("sessions.list"));
    out.insert("agents".into(), count("agents.list"));
    out.insert("projects".into(), count("projects.list"));
    Value::Object(out)
}

/// Fold a session summary's flags into one status word — the table
/// reads better with `locked`/`busy`/`archived` spelled out than a
/// wall of booleans.
fn session_status(row: &Value) -> String {
    let flag = |key: &str| row[key].as_bool().unwrap_or(false);
    let mut parts = Vec::new();
    if flag("locked") {
        parts.push("locked");
    }
    if flag("busy") {
        parts.push("busy");
    }
    if flag("archived") {
        parts.push("archived");
    }
    if flag("pinned") {
        parts.push("pinned");
    }
    if parts.is_empty() {
        "idle".to_string()
    } else {
        parts.join("+")
    }
}

/// One `sessions.list` row → table row. Pure — unit-tested with
/// synthetic JSON.
fn session_row(s: &Value) -> Value {
    json!({
        "id": s["id"],
        "title": s["title"],
        "agent": s["agent"],
        "status": session_status(s),
        "cwd": s["cwd"],
        "updated": s["updatedAt"],
        "source": s["source"],
    })
}

/// `sessions` query — `sessions.list`. Lock probing (`withLocks`)
/// stays off: it fans a `session/list` probe out to every registered
/// agent and a slow store driver can hold the call past a host's
/// request timeout — `busy` (live attach state) and the overlay
/// flags still come through.
fn sessions_rows(ctx: &Map<String, Value>) -> Value {
    let params = agent_params(ctx);
    match serve("sessions.list", Value::Object(params), ctx) {
        Ok(Value::Array(rows)) => Value::Array(rows.iter().map(session_row).collect()),
        Ok(other) => other,
        Err(e) => e,
    }
}

/// `session` query — the detail kv. The row comes from
/// `sessions.list` (the `SessionSummary` shape — agent, overlay
/// flags, project tags) matched on `id` (+ `agent` when given, since
/// ids collide across stores); sessions the list doesn't know fall
/// back to `sessions.summary`, which returns the metadata-only
/// session IR.
fn session_detail(ctx: &Map<String, Value>) -> Value {
    let Some(id) = ctx.get("id").and_then(Value::as_str) else {
        return json!({"error": "missing required param 'id' (e.g. --param id=<session-id>)"});
    };
    let agent = data::agent_param(ctx);
    let list = serve("sessions.list", json!({}), ctx);
    if let Ok(Value::Array(rows)) = &list {
        let found = rows.iter().find(|s| {
            s["id"].as_str() == Some(id)
                && agent.as_deref().is_none_or(|a| {
                    s["agent"].as_str() == Some(a) || s["agentId"].as_str() == Some(a)
                })
        });
        if let Some(s) = found {
            let projects = s["projectIds"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            return json!({
                "id": s["id"],
                "title": s["title"],
                "agent": s["agent"],
                "cwd": s["cwd"],
                "status": session_status(s),
                "updated": s["updatedAt"],
                "source": s["source"],
                "locked": s["locked"],
                "busy": s["busy"],
                "pinned": s["pinned"],
                "archived": s["archived"],
                "projects": projects,
                "parentSession": s["parentSessionId"],
                "agentId": s["agentId"],
                "canRewind": s["canRewind"],
            });
        }
    }
    // Not in the merged list — try the store-level summary (the
    // session IR, metadata-only: no `nodes`/`promptHistory`).
    let mut params = agent_params(ctx);
    params.insert("id".to_string(), json!(id));
    match serve("sessions.summary", Value::Object(params), ctx) {
        Ok(s) if s.is_object() => json!({
            "id": s["id"],
            "title": s["title"],
            "backend": s["backendType"],
            "agentMode": s["agentMode"],
            "model": s["model"],
            "cwd": s["workingDirectory"],
            "created": fmt_time(&json!(s["createdAt"].as_f64().unwrap_or(0.0) * 1000.0)),
            "lastActivity": fmt_time(&json!(s["lastActivityAt"].as_f64().unwrap_or(0.0) * 1000.0)),
            "parentSession": s["parentSessionId"],
            "agentId": s["agentId"],
            "checkpoints": s["checkpoints"].as_array().map_or(0, Vec::len),
        }),
        Ok(other) => other,
        Err(e) => e,
    }
}

/// Epoch-milliseconds → RFC 3339 for the timeline's Time column;
/// unparseable input passes through as the raw number's string.
fn fmt_time(ms: &Value) -> Value {
    let Some(ms_f) = ms.as_f64() else {
        return ms.clone();
    };
    let secs = (ms_f / 1000.0) as i64;
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|dt| {
            dt.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .map_or_else(|| json!(ms_f), Value::String)
}

/// Collapse a message body to one table cell — newlines become `⏎`,
/// long transcripts truncate.
const CELL_MAX: usize = 200;

fn cell_text(content: &Value) -> String {
    let raw = content.as_str().unwrap_or_default();
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > CELL_MAX {
        let cut: String = flat.chars().take(CELL_MAX).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

/// One `sessions.history` message → timeline row. Pure.
fn history_row(m: &Value) -> Value {
    json!({
        "role": m["role"],
        "time": fmt_time(&m["createdAt"]),
        "node": m["nodeId"],
        "tool": m["toolName"],
        "content": cell_text(&m["content"]),
    })
}

/// `history` query — `sessions.history` (paged message window).
/// `limit` passes through; default is the plane's own
/// `SEPIA_HISTORY_LIMIT`.
fn history_rows(ctx: &Map<String, Value>) -> Value {
    let Some(id) = ctx.get("id").and_then(Value::as_str) else {
        return json!({"error": "missing required param 'id' (e.g. --param id=<session-id>)"});
    };
    let mut params = agent_params(ctx);
    params.insert("id".to_string(), json!(id));
    if let Some(limit) = ctx.get("limit").and_then(Value::as_u64) {
        params.insert("limit".to_string(), json!(limit));
    }
    match serve("sessions.history", Value::Object(params), ctx) {
        // The table binding reads a bare array of row objects — the
        // page envelope (`total`/`start`) stays on the wire, not in
        // the view.
        Ok(page) if page.is_object() => page["messages"].as_array().map_or_else(
            || json!([]),
            |msgs| Value::Array(msgs.iter().map(history_row).collect()),
        ),
        Ok(other) => other,
        Err(e) => e,
    }
}

/// `agents` query — `agents.list` rows merged with `config.list`
/// item counts by agent id. A `config.list` failure degrades the
/// counts columns, not the whole table.
fn agents_rows(ctx: &Map<String, Value>) -> Value {
    let agents = match serve("agents.list", json!({}), ctx) {
        Ok(Value::Array(rows)) => rows,
        Ok(other) => return other,
        Err(e) => return e,
    };
    let mut config_params = agent_params(ctx);
    if let Some(dir) = data::dir_param(ctx) {
        config_params.insert("dir".to_string(), json!(dir));
    }
    let configs = serve("config.list", Value::Object(config_params), ctx)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    Value::Array(
        agents
            .iter()
            .map(|a| {
                let id = a["id"].as_str().unwrap_or_default();
                let cfg = configs.iter().find(|c| c["agent"].as_str() == Some(id));
                let counts = cfg.and_then(|c| c.get("counts"));
                let count = |key: &str| counts.map_or(Value::Null, |c| c[key].clone());
                json!({
                    "id": a["id"],
                    "label": a["label"],
                    "runtime": if a["capabilities"].is_null() { "—" } else { "acp" },
                    "skills": count("skills"),
                    "rules": count("rules"),
                    "commands": count("commands"),
                    "hooks": count("hooks"),
                    "mcpServers": count("mcpServers"),
                    "error": cfg.and_then(|c| c.get("error")).cloned().unwrap_or(Value::Null),
                })
            })
            .collect(),
    )
}

/// `projects` query — `projects.list` rows with session counts
/// tallied from `sessions.list` (`projectIds` membership). A
/// sessions failure degrades the count column to null.
fn projects_rows(ctx: &Map<String, Value>) -> Value {
    let projects = match serve("projects.list", json!({}), ctx) {
        Ok(Value::Array(rows)) => rows,
        Ok(other) => return other,
        Err(e) => return e,
    };
    let sessions = serve("sessions.list", json!({}), ctx)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    Value::Array(
        projects
            .iter()
            .map(|p| {
                let id = p["id"].as_str().unwrap_or_default();
                let count = sessions
                    .iter()
                    .filter(|s| {
                        s["projectIds"]
                            .as_array()
                            .is_some_and(|ids| ids.iter().any(|i| i.as_str() == Some(id)))
                    })
                    .count();
                json!({
                    "id": p["id"],
                    "name": p["name"],
                    "sessions": count,
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------- view op

/// `{"op":"view","view":id,"params":{…}}` → [`pavo_protocol::wire::ViewResult`].
pub fn view_result(
    view_id: &str,
    params: &Map<String, Value>,
) -> Result<pavo_protocol::wire::ViewResult, String> {
    let spec = specs()
        .into_iter()
        .find(|v| v.id == view_id)
        .ok_or_else(|| format!("unknown view: {view_id}"))?;

    let mut data_map = Map::new();
    for b in &spec.queries {
        data_map.insert(b.name.clone(), query_data(&b.name, params));
    }

    Ok(pavo_protocol::wire::ViewResult {
        view: spec,
        data: data_map,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn manifest_advertises_pavo_and_lists_every_view() {
        let m = manifest();
        assert!(m.verbs.iter().any(|v| v == "serve"));
        assert!(m.capabilities.iter().any(|c| c == "pavo"));
        assert_eq!(m.renderers.len(), 0);
        let ids: std::collections::BTreeSet<&str> = m.views.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids.len(), m.views.len(), "duplicate view ids");
        for want in ["status", "sessions", "session", "agents", "projects"] {
            assert!(ids.contains(want), "missing view {want}");
        }
    }

    #[test]
    fn every_binding_is_a_known_query() {
        for v in specs() {
            for b in &v.queries {
                assert!(known_query(&b.name), "{}: unknown query {}", v.id, b.name);
            }
        }
    }

    #[test]
    fn session_status_folds_flags() {
        assert_eq!(session_status(&json!({})), "idle");
        assert_eq!(
            session_status(&json!({"locked": true, "busy": true})),
            "locked+busy"
        );
        assert_eq!(session_status(&json!({"archived": true})), "archived");
        assert_eq!(
            session_status(&json!({"pinned": true, "locked": false})),
            "pinned"
        );
    }

    #[test]
    fn session_row_maps_wire_fields() {
        let row = session_row(&json!({
            "id": "s1", "title": "T", "agent": "devin", "cwd": "/tmp",
            "updatedAt": "2025-01-01T00:00:00Z", "source": "store",
            "locked": true, "busy": false, "pinned": true,
        }));
        assert_eq!(row["id"], "s1");
        assert_eq!(row["status"], "locked+pinned");
        assert_eq!(row["updated"], "2025-01-01T00:00:00Z");
    }

    #[test]
    fn history_row_flattens_message() {
        let row = history_row(&json!({
            "role": "assistant", "nodeId": 7, "createdAt": 1_735_689_600_000.0,
            "toolName": "bash", "content": "line one\nline two",
        }));
        assert_eq!(row["role"], "assistant");
        assert_eq!(row["node"], 7);
        assert_eq!(row["content"], "line one line two");
        assert!(row["time"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn cell_text_truncates() {
        let long = "x".repeat(500);
        let out = cell_text(&json!(long));
        assert!(out.chars().count() <= 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn id_gated_queries_reject_missing_id() {
        let params = Map::new();
        for name in ["session", "history"] {
            let v = query_data(name, &params);
            assert!(v["error"].as_str().unwrap().contains("'id'"), "{name}");
        }
    }
}
