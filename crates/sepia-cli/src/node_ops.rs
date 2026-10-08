//! Port of `apps/sepia/src/node-commands.ts` — the node-op verbs; every
//! handler here talks to a running sepia node's REST API
//! (docs/protocol.md). Each takes `--node`/`--token` (defaults:
//! `SEPIA_NODE_URL` → http://127.0.0.1:8787, `SEPIA_TOKEN` → none).

use serde_json::{Map, Value, json};

use crate::CliError;
use crate::api::{self, NodeTarget};

/// `--node`/`--token`/`--json` — the shared flag block of node verbs.
#[derive(Clone, clap::Args)]
pub struct NodeArgs {
    /// Base URL of the sepia node (default: SEPIA_NODE_URL, else http://127.0.0.1:8787)
    #[arg(long, default_value_t = api::default_node_url())]
    pub node: String,
    /// Bearer token for the node (default: SEPIA_TOKEN)
    #[arg(long)]
    pub token: Option<String>,
    /// Print the raw API response as JSON
    #[arg(long)]
    pub json: bool,
}

/// `--node`/`--token` without `--json`.
#[derive(Clone, clap::Args)]
pub struct TargetArgs {
    /// Base URL of the sepia node (default: SEPIA_NODE_URL, else http://127.0.0.1:8787)
    #[arg(long, default_value_t = api::default_node_url())]
    pub node: String,
    /// Bearer token for the node (default: SEPIA_TOKEN)
    #[arg(long)]
    pub token: Option<String>,
}

impl NodeArgs {
    fn target(&self) -> NodeTarget {
        api::resolve_target(&self.node, self.token.as_deref())
    }
}

impl TargetArgs {
    fn target(&self) -> NodeTarget {
        api::resolve_target(&self.node, self.token.as_deref())
    }
}

impl From<api::ApiError> for CliError {
    fn from(e: api::ApiError) -> Self {
        CliError(e.message)
    }
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn row_flags(session: &api::SessionSummary) -> String {
    [
        session.locked.then_some("locked"),
        session.busy.then_some("busy"),
        (session.pinned == Some(true)).then_some("pinned"),
        (session.archived == Some(true)).then_some("archived"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",")
}

fn print_session(session: &api::SessionSummary) {
    let flags = row_flags(session);
    println!(
        "{}:{}\t{}\t{}\t{}{}",
        session.agent,
        session.id,
        session.title,
        session.cwd,
        session.updated_at,
        if flags.is_empty() {
            String::new()
        } else {
            format!("\t{flags}")
        }
    );
}

/// SSE frames land here — a named frame prints `kind {json}`, an unnamed
/// one just its data line.
fn print_frame(frame: &api::SseFrame) {
    match &frame.event {
        Some(event) => println!("{event} {}", frame.data),
        None => println!("{}", frame.data),
    }
}

/// `[ISO-8601] role(tool): content` — the `printHistoryMessage` format.
fn print_history_message(message: &Value) {
    let tool = message
        .get("toolName")
        .and_then(Value::as_str)
        .map_or(String::new(), |t| format!("({t})"));
    let at = iso_of_ms(
        message
            .get("createdAt")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
    );
    println!(
        "[{at}] {}{tool}: {}",
        message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
}

/// `new Date(ms).toISOString()` — `YYYY-MM-DDTHH:mm:ss.sssZ`.
fn iso_of_ms(ms: f64) -> String {
    if !ms.is_finite() {
        return "1970-01-01T00:00:00.000Z".into();
    }
    let nanos = (ms * 1_000_000.0) as i128;
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp_nanos(nanos) else {
        return "1970-01-01T00:00:00.000Z".into();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

// --- Top-level node verbs ----------------------------------------------------

/// `sepia health` — `GET /api/health`.
pub fn health(args: &NodeArgs) -> Result<(), CliError> {
    let health = api::get_health(&args.target())?;
    if args.json {
        print_json(&health);
        return Ok(());
    }
    println!(
        "{}",
        if health.get("ok").and_then(Value::as_bool) == Some(true) {
            "ok"
        } else {
            "unhealthy"
        }
    );
    Ok(())
}

/// `sepia node` — `GET /api/node`.
pub fn node(args: &NodeArgs) -> Result<(), CliError> {
    let descriptor = api::get_node(&args.target())?;
    if args.json {
        print_json(&descriptor);
        return Ok(());
    }
    let get = |k: &str| descriptor.get(k);
    let strings = |k: &str| {
        get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    println!(
        "{} ({})",
        get("name").and_then(Value::as_str).unwrap_or_default(),
        get("id").and_then(Value::as_str).unwrap_or_default()
    );
    println!(
        "  version:      {}",
        get("version").and_then(Value::as_str).unwrap_or_default()
    );
    println!(
        "  protocol:     {}",
        get("protocol").and_then(Value::as_i64).unwrap_or_default()
    );
    let agents = strings("agents");
    println!(
        "  agents:       {}",
        if agents.is_empty() { "none" } else { &agents }
    );
    println!("  capabilities: {}", strings("capabilities"));
    Ok(())
}

/// `sepia agents` — `GET /api/agents`.
pub fn agents(args: &NodeArgs) -> Result<(), CliError> {
    let agents = api::list_agents(&args.target())?;
    if args.json {
        print_json(&json!(agents));
        return Ok(());
    }
    for agent in &agents {
        println!(
            "{}\t{}{}",
            agent.id,
            agent.label,
            if agent.capabilities.is_none() {
                ""
            } else {
                "\tcapabilities probed"
            }
        );
    }
    Ok(())
}

/// `sepia user` — `GET /api/user`.
pub fn user(args: &NodeArgs) -> Result<(), CliError> {
    let data = api::get_user(&args.target())?;
    let user = data.get("user").cloned().unwrap_or(Value::Null);
    if args.json {
        print_json(&user);
        return Ok(());
    }
    let get = |k: &str| user.get(k).and_then(Value::as_str);
    println!(
        "{}@{}\t{}\t{}\t{}/{}",
        get("username").unwrap_or_default(),
        get("hostname").unwrap_or_default(),
        get("homedir").unwrap_or_default(),
        get("shell").unwrap_or("-"),
        get("platform").unwrap_or_default(),
        get("arch").unwrap_or_default()
    );
    Ok(())
}

/// `sepia fs <path>` — `GET /api/fs`.
pub fn fs(path: &str, args: &NodeArgs) -> Result<(), CliError> {
    let dirs = api::list_dirs(&args.target(), path)?;
    if args.json {
        print_json(&json!({ "dirs": dirs }));
        return Ok(());
    }
    for dir in &dirs {
        println!("{dir}");
    }
    Ok(())
}

/// `sepia events` — `GET /api/events` to stdout.
pub fn events(args: &TargetArgs) -> Result<(), CliError> {
    api::stream_sse(&args.target(), "GET", "/api/events", None, print_frame).map_err(CliError::from)
}

/// `sepia redeem <code>` — `POST /api/pair`.
pub fn redeem(code: &str, args: &TargetArgs) -> Result<(), CliError> {
    let token = api::pair_redeem(&api::resolve_target(&args.node, None), code)?;
    println!("{token}");
    eprintln!("Export it as SEPIA_TOKEN or pass it via --token.");
    Ok(())
}

// --- sessions -----------------------------------------------------------------

/// `sepia sessions list` — `GET /api/sessions`.
pub fn sessions_list(locks: bool, args: &NodeArgs) -> Result<(), CliError> {
    let sessions = api::list_sessions(&args.target(), locks)?;
    if args.json {
        print_json(&json!({ "sessions": sessions }));
        return Ok(());
    }
    for session in &sessions {
        print_session(session);
    }
    Ok(())
}

/// `sepia sessions create` — `POST /api/sessions`.
pub fn sessions_create(
    cwd: &str,
    agent: Option<&str>,
    title: Option<&str>,
    model: Option<&str>,
    fallback: &[String],
    args: &NodeArgs,
) -> Result<(), CliError> {
    let mut body = Map::new();
    body.insert("cwd".into(), json!(cwd));
    if let Some(agent) = agent {
        body.insert("agent".into(), json!(agent));
    }
    if let Some(title) = title {
        body.insert("title".into(), json!(title));
    }
    if let Some(model) = model {
        body.insert("model".into(), json!(model));
    }
    if !fallback.is_empty() {
        body.insert("fallbacks".into(), json!(fallback));
    }
    let created = api::create_session(&args.target(), &Value::Object(body))?;
    if args.json {
        print_json(&created);
        return Ok(());
    }
    println!(
        "Created session {} on agent {}",
        created
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        created
            .get("agentId")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    Ok(())
}

/// `sepia sessions attach <id>` — `POST /api/sessions/:id/attach`.
pub fn sessions_attach(
    session_id: &str,
    takeover: bool,
    model: Option<&str>,
    fallback: &[String],
    agent: Option<&str>,
    args: &NodeArgs,
) -> Result<(), CliError> {
    let result = api::attach(
        &args.target(),
        session_id,
        takeover,
        model,
        if fallback.is_empty() {
            None
        } else {
            Some(fallback)
        },
        agent,
    )?;
    if args.json {
        print_json(&serde_json::to_value(&result).unwrap_or_default());
        return Ok(());
    }
    let mode = if result.attached {
        if result.read_only {
            "read-only"
        } else {
            "writable"
        }
    } else {
        "not attached"
    };
    println!("Session {session_id}: {mode} ({})", result.agent_id);
    Ok(())
}

/// `sepia prompt <id> <text>` (also `sessions prompt`).
pub fn sessions_prompt(
    session_id: &str,
    text: &str,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    api::prompt(&args.target(), session_id, text, agent)?;
    println!("Prompt sent to session {session_id}");
    Ok(())
}

/// `sepia sessions cancel <id>` — `POST /api/sessions/:id/cancel`.
pub fn sessions_cancel(
    session_id: &str,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    api::cancel(&args.target(), session_id, agent)?;
    println!("Cancelled session {session_id}");
    Ok(())
}

/// `sepia sessions permission <id>` — answer a pending permission request.
pub fn sessions_permission(
    session_id: &str,
    request: &str,
    option: Option<&str>,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    api::respond_to_permission(&args.target(), session_id, request, option, agent)?;
    println!("Answered permission {request} on session {session_id}");
    Ok(())
}

/// `sepia sessions history <id>` — `GET /api/sessions/:id/history`.
pub fn sessions_history(
    session_id: &str,
    limit: Option<i64>,
    before: Option<i64>,
    agent: Option<&str>,
    args: &NodeArgs,
) -> Result<(), CliError> {
    let page = api::get_history(&args.target(), session_id, limit, before, agent)?;
    if args.json {
        print_json(&page);
        return Ok(());
    }
    for message in page
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        print_history_message(&message);
    }
    let start = page
        .get("start")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if start > 0 {
        eprintln!("… {start} earlier message(s); pass --before {start}");
    }
    Ok(())
}

/// `sepia sessions checkpoints <id>` — `GET /api/sessions/:id/checkpoints`.
pub fn sessions_checkpoints(
    session_id: &str,
    agent: Option<&str>,
    args: &NodeArgs,
) -> Result<(), CliError> {
    let checkpoints = api::get_checkpoints(&args.target(), session_id, agent)?;
    if args.json {
        print_json(&json!({ "checkpoints": checkpoints }));
        return Ok(());
    }
    for checkpoint in &checkpoints {
        let kind = checkpoint
            .kind
            .as_ref()
            .map_or(String::new(), |k| format!("\t{k}"));
        let runs = checkpoint
            .run_count
            .map_or(String::new(), |n| format!("\truns:{n}"));
        println!(
            "{}\t{}{}{}",
            checkpoint.r#ref,
            iso_of_ms(checkpoint.created_at),
            kind,
            runs
        );
    }
    Ok(())
}

/// `sepia sessions export <id>` — the complete session IR.
pub fn sessions_export(
    session_id: &str,
    out: Option<&str>,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    let session = api::export_session(&args.target(), session_id, agent)?;
    let json = format!(
        "{}\n",
        serde_json::to_string_pretty(&session).unwrap_or_default()
    );
    match out {
        None | Some("-") => println!("{}", json.trim_end()),
        Some(out_path) => {
            std::fs::write(out_path, &json)
                .map_err(|e| CliError(format!("Failed to write {out_path}: {e}")))?;
            println!("Exported session {session_id} to {out_path}");
        }
    }
    Ok(())
}

fn urlencode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// `sepia sessions stream <id>` — the live run's session events
/// (sepia-proto `SessionEvent` JSON frames) to stdout.
pub fn sessions_stream(
    session_id: &str,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    let query = agent.map_or(String::new(), |a| format!("?agent={}", urlencode(a)));
    api::stream_sse(
        &args.target(),
        "GET",
        &format!("/api/sessions/{}/stream{query}", urlencode(session_id)),
        None,
        print_frame,
    )
    .map_err(CliError::from)
}

/// `sepia sessions meta <id>` — the meta overlay PATCH.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub fn sessions_meta(
    session_id: &str,
    title: Option<&str>,
    pin: bool,
    unpin: bool,
    archive: bool,
    unarchive: bool,
    project: &[String],
    model: Option<&str>,
    clear_model: bool,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    if pin && unpin {
        return Err(CliError("--pin and --unpin conflict".into()));
    }
    if archive && unarchive {
        return Err(CliError("--archive and --unarchive conflict".into()));
    }
    if model.is_some() && clear_model {
        return Err(CliError("--model and --clear-model conflict".into()));
    }
    let mut patch = Map::new();
    if let Some(title) = title {
        patch.insert("title".into(), json!(title));
    }
    if pin {
        patch.insert("pinned".into(), json!(true));
    }
    if unpin {
        patch.insert("pinned".into(), json!(false));
    }
    if archive {
        patch.insert("archived".into(), json!(true));
    }
    if unarchive {
        patch.insert("archived".into(), json!(false));
    }
    if !project.is_empty() {
        patch.insert("projectIds".into(), json!(project));
    }
    if let Some(model) = model {
        patch.insert("model".into(), json!(model));
    }
    if clear_model {
        patch.insert("model".into(), Value::Null);
    }
    if patch.is_empty() {
        return Err(CliError("Nothing to patch — pass a flag to change".into()));
    }
    api::patch_session(&args.target(), session_id, &Value::Object(patch), agent)?;
    println!("Updated session {session_id}");
    Ok(())
}

/// `sepia sessions rename <id> <title>`.
pub fn sessions_rename(
    session_id: &str,
    title: &str,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    api::patch_session(
        &args.target(),
        session_id,
        &json!({ "title": title }),
        agent,
    )?;
    println!("Renamed session {session_id}");
    Ok(())
}

/// `sepia sessions delete <id>` — `DELETE /api/sessions/:id`.
pub fn sessions_delete(
    session_id: &str,
    agent: Option<&str>,
    args: &TargetArgs,
) -> Result<(), CliError> {
    api::delete_session(&args.target(), session_id, agent)?;
    println!("Deleted session {session_id}");
    Ok(())
}

/// `sepia sessions convert <id> --to cline|devin`.
pub fn sessions_convert(session_id: &str, to: &str, args: &TargetArgs) -> Result<(), CliError> {
    let converted = api::convert_session(&args.target(), session_id, to)?;
    println!("Converted session {session_id} → {to}: {converted}");
    Ok(())
}

/// `sepia sessions import <path>` — session IR JSON or a flat history
/// array into an agent store.
#[allow(clippy::too_many_arguments)]
pub fn sessions_import(
    path: &str,
    to: &str,
    cwd: Option<&str>,
    title: Option<&str>,
    model: Option<&str>,
    args: &NodeArgs,
) -> Result<(), CliError> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| CliError(format!("Cannot read {path}: {e}")))?;
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|_| CliError(format!("{path} is not valid JSON")))?;
    // A bare array is the flat history form; `{session: …}` is the
    // /export envelope; anything else is treated as the session IR.
    let mut body = Map::new();
    body.insert("agent".into(), json!(to));
    if parsed.is_array() {
        body.insert("history".into(), parsed);
    } else if let Some(session) = parsed
        .as_object()
        .and_then(|o| o.get("session"))
        .filter(|s| !s.is_null())
    {
        body.insert("session".into(), session.clone());
    } else {
        body.insert("session".into(), parsed);
    }
    if let Some(cwd) = cwd {
        body.insert("cwd".into(), json!(cwd));
    }
    if let Some(title) = title {
        body.insert("title".into(), json!(title));
    }
    if let Some(model) = model {
        body.insert("model".into(), json!(model));
    }
    let summary = api::import_session(&args.target(), &Value::Object(body))?;
    if args.json {
        print_json(&serde_json::to_value(&summary).unwrap_or_default());
        return Ok(());
    }
    print_session(&summary);
    Ok(())
}

const RESUME_PAGE_SIZE: i64 = 500;

/// `sepia sessions resume <id> --to cline|devin` — "Resume on…": pull a
/// session off the source node, write it into an agent store here.
#[allow(clippy::too_many_arguments)]
pub fn sessions_resume(
    session_id: &str,
    to: &str,
    source: &str,
    source_token: Option<&str>,
    from_agent: Option<&str>,
    cwd: Option<&str>,
    title: Option<&str>,
    model: Option<&str>,
    args: &NodeArgs,
) -> Result<(), CliError> {
    let source_target = api::resolve_target(source, source_token.or(args.token.as_deref()));
    // Prefer the full IR from /export; a node too old to serve it 404s
    // and falls back to paging the flat /history projection.
    let exported = match api::export_session(&source_target, session_id, from_agent) {
        Ok(v) => Some(v),
        Err(e) if e.status == Some(404) => None,
        Err(e) => return Err(e.into()),
    };
    let mut body = Map::new();
    body.insert("agent".into(), json!(to));
    if let Some(session) = exported {
        body.insert("session".into(), session);
    } else {
        let mut pages: Vec<Vec<Value>> = Vec::new();
        let mut before: Option<i64> = None;
        loop {
            let page = api::get_history(
                &source_target,
                session_id,
                Some(RESUME_PAGE_SIZE),
                before,
                from_agent,
            )?;
            let messages = page
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let start = page
                .get("start")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let empty = messages.is_empty();
            pages.insert(0, messages);
            if start <= 0 || empty {
                break;
            }
            before = Some(start);
        }
        body.insert(
            "history".into(),
            Value::Array(pages.into_iter().flatten().collect()),
        );
    }
    if let Some(cwd) = cwd {
        body.insert("cwd".into(), json!(cwd));
    }
    if let Some(title) = title {
        body.insert("title".into(), json!(title));
    }
    if let Some(model) = model {
        body.insert("model".into(), json!(model));
    }
    let summary = api::import_session(&args.target(), &Value::Object(body))?;
    if args.json {
        print_json(&serde_json::to_value(&summary).unwrap_or_default());
        return Ok(());
    }
    print_session(&summary);
    Ok(())
}

/// `sepia sessions restore <id>` — revert workspace files via recorded
/// diffs or a checkpoint ref. `--confirm` mandatory.
#[allow(clippy::too_many_arguments)]
pub fn sessions_restore(
    session_id: &str,
    confirm: bool,
    path: Option<&str>,
    tool_call_id: Option<&str>,
    checkpoint: Option<&str>,
    paths: &[String],
    agent: Option<&str>,
    args: &NodeArgs,
) -> Result<(), CliError> {
    if !confirm {
        return Err(CliError(
            "Pass --confirm — restore writes real files under the session's cwd".into(),
        ));
    }
    if path.is_some() == checkpoint.is_some() {
        return Err(CliError(
            "Pass exactly one of --path or --checkpoint".into(),
        ));
    }
    let selector = if let Some(path) = path {
        let mut s = Map::new();
        s.insert("path".into(), json!(path));
        if let Some(tool_call_id) = tool_call_id {
            s.insert("toolCallId".into(), json!(tool_call_id));
        }
        Value::Object(s)
    } else {
        let mut s = Map::new();
        s.insert("checkpoint".into(), json!(checkpoint.unwrap_or_default()));
        if !paths.is_empty() {
            s.insert("paths".into(), json!(paths));
        }
        Value::Object(s)
    };
    let result = api::restore_session(&args.target(), session_id, &selector, agent)?;
    if args.json {
        print_json(&serde_json::to_value(&result).unwrap_or_default());
        return Ok(());
    }
    for file in &result.restored {
        println!("{}\t{}", file.action, file.path);
    }
    for file in &result.skipped {
        println!("skipped\t{}\t{}", file.path, file.reason);
    }
    Ok(())
}

/// `sepia sessions rewind <id>` — truncate the transcript at a node,
/// turn count or checkpoint. `--confirm` mandatory.
#[allow(clippy::too_many_arguments)]
pub fn sessions_rewind(
    session_id: &str,
    confirm: bool,
    node_id: Option<i64>,
    turns: Option<i64>,
    checkpoint: Option<&str>,
    agent: Option<&str>,
    args: &NodeArgs,
) -> Result<(), CliError> {
    if !confirm {
        return Err(CliError(
            "Pass --confirm — rewind truncates the session's stored transcript".into(),
        ));
    }
    let selectors: Vec<Value> = [
        node_id.map(|n| json!({ "nodeId": n })),
        turns.map(|t| json!({ "turns": t })),
        checkpoint.map(|c| json!({ "checkpoint": c })),
    ]
    .into_iter()
    .flatten()
    .collect();
    if selectors.len() != 1 {
        return Err(CliError(
            "Pass exactly one of --node-id, --turns, --checkpoint".into(),
        ));
    }
    let result = api::rewind_session(&args.target(), session_id, &selectors[0], agent)?;
    if args.json {
        print_json(&serde_json::to_value(&result).unwrap_or_default());
        return Ok(());
    }
    println!(
        "Rewound session {session_id}: kept {}, removed {}",
        result.kept, result.removed
    );
    Ok(())
}

// --- projects -----------------------------------------------------------------

pub fn projects_list(args: &NodeArgs) -> Result<(), CliError> {
    let projects = api::list_projects(&args.target())?;
    if args.json {
        print_json(&json!({ "projects": projects }));
        return Ok(());
    }
    for project in &projects {
        println!("{}\t{}", project.id, project.name);
    }
    Ok(())
}

pub fn projects_create(name: &str, args: &TargetArgs) -> Result<(), CliError> {
    let project = api::create_project(&args.target(), name)?;
    println!("{}\t{}", project.id, project.name);
    Ok(())
}

pub fn projects_rename(project_id: &str, name: &str, args: &TargetArgs) -> Result<(), CliError> {
    api::rename_project(&args.target(), project_id, name)?;
    println!("Renamed project {project_id}");
    Ok(())
}

pub fn projects_delete(project_id: &str, args: &TargetArgs) -> Result<(), CliError> {
    api::delete_project(&args.target(), project_id)?;
    println!("Deleted project {project_id}");
    Ok(())
}

// --- config (node) ------------------------------------------------------------

/// `sepia config get` — `GET /api/config`.
pub fn config_get(args: &NodeArgs) -> Result<(), CliError> {
    let config = api::get_config(&args.target())?;
    if args.json {
        print_json(&config);
        return Ok(());
    }
    if let Value::Object(map) = &config {
        for (key, value) in map {
            println!(
                "{key}\t{}",
                serde_json::to_string(value).unwrap_or_default()
            );
        }
    }
    Ok(())
}

/// `sepia config set <key> <value>` — `PATCH /api/config/:key`.
pub fn config_set(key: &str, value: &str, args: &TargetArgs) -> Result<(), CliError> {
    let parsed: Value = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
    api::set_config(&args.target(), key, &parsed)?;
    println!(
        "{key} = {}",
        serde_json::to_string(&parsed).unwrap_or_default()
    );
    Ok(())
}

// --- push ---------------------------------------------------------------------

/// `sepia push vapid` — `GET /api/push/vapid`.
pub fn push_vapid(args: &TargetArgs) -> Result<(), CliError> {
    let public_key = api::get_vapid(&args.target())?;
    println!("{public_key}");
    Ok(())
}

/// `sepia push subscribe <json>` — `POST /api/push/subscribe`.
pub fn push_subscribe(subscription: &str, args: &TargetArgs) -> Result<(), CliError> {
    let parsed: Value = serde_json::from_str(subscription)
        .map_err(|_| CliError("subscription must be valid JSON".into()))?;
    api::push_subscribe(&args.target(), &parsed)?;
    println!("Subscribed");
    Ok(())
}

/// `sepia push unsubscribe <endpoint>` — `DELETE /api/push/subscribe`.
pub fn push_unsubscribe(endpoint: &str, args: &TargetArgs) -> Result<(), CliError> {
    api::push_unsubscribe(&args.target(), endpoint)?;
    println!("Unsubscribed");
    Ok(())
}
