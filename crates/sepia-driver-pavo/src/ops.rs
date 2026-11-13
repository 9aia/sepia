//! Engine ops — `cli.<verb>` wire queries dispatch into the `sepia`
//! engine lib through this binary's own `run` verb: each call
//! re-execs `tamias-driver-sepia run <argv>` with the request's
//! params mapped back onto sepia's own flags, so
//! `sepia_cli::run_from` writes to the child's captured stdout and
//! never touches the serve loop's ndjson stream — and every mutation
//! keeps the engine's own semantics (store resolution, lock probes,
//! config-dir defaults).
//!
//! `ops` answers the ops manifest as JSON — *derived from the
//! engine's own clap tree* ([`sepia_cli::command`]), so a verb can't
//! exist on the CLI and be missing here: declared args become typed
//! params (`arg`/`opt`/`flag`), doc strings ride along unchanged.
//!
//! sepia has one verb that can never be a request/response op — it
//! rides `run` passthrough / generated-surface subcommands
//! (inherited stdio) instead, and is listed in `surfaces`:
//!
//! - `serve` — the unbounded ndjson session-plane loop; it never
//!   returns. (The *ops-carrying* `tamias-driver-sepia serve` is a
//!   different process — this surface is `sepia serve` itself.)
//!
//! No verb takes a secret, token, or prompt text via argv — session
//! prompts travel the `sepia serve` wire (`sessions.prompt`), never
//! the CLI — so `SKIP_PARAMS` stays empty and no param exclusion is
//! needed. Streaming *flags* would join `SKIP_PARAMS` — none exist
//! today. A `params-stdin` channel is org task T-008; nothing here
//! needs it yet.
//!
//! `mutates` is honest: `store`/`config` `import`/`install`/`delete`
//! (and their top-level aliases) rewrite agent session stores and
//! config dirs. `export`/`list`/`diff`/`driver list`/`version` only
//! read — an export's `--out`-style positional writes an artifact
//! file, not engine state.
//!
//! `invoke` has two lanes. `cli.<verb>`-style ids (bare verb names,
//! `store.*`, `config.*`, `driver.*`, `cli.` prefix tolerated) ride
//! the argv/exec path above — engine/store/config mutations keep
//! the CLI's own semantics. `session.<verb>`/`sessions.<verb>` ids
//! are the curated session-plane actions: they dispatch to `sepia
//! serve` methods (`sessions.attach`, `sessions.prompt`, …) through
//! the same persistent child the view queries use
//! ([`crate::data::call`]) — the plane operations that have no CLI
//! verb at all. See the "serve lane" section by [`invoke`].

use clap::{Arg, ArgAction, Command};
use pavo_driver_kit as kit;
use pavo_protocol::wire::{ActionResult, QueryResult, Response, SubmitResult};
use pavo_protocol::{ParamKind, ParamSchema};
use serde_json::{Map, Value, json};

use crate::data;

/// Unbounded surfaces — verbs that never return (`serve`). They can
/// never be request/response ops.
const SURFACES: &[&str] = &["serve"];

/// Params excluded from ops — flags that turn a bounded verb into
/// a stream, or params that must never ride argv. None today: no
/// sepia verb takes a secret or prompt via argv (prompts are a
/// `sepia serve` wire concern), and no verb has a watch/follow flag.
const SKIP_PARAMS: &[&str] = &[];

/// Verbs with side effects beyond reads — session-store and
/// agent-config writes. `store.*`/`config.*` are the namespaced
/// spellings; the bare top-level `import`/`install`/`delete` are the
/// same store verbs exposed without the `store` prefix.
const MUTATES: &[&str] = &[
    "store.import",
    "store.install",
    "store.delete",
    "config.import",
    "config.install",
    "import",
    "install",
    "delete",
];

/// `(name, clap spec)` for every leaf verb. A subcommand node whose
/// own subcommand is *optional* (`sepia driver` alone lists drivers)
/// is itself invocable, so it emits as a leaf *and* recurses —
/// `driver` and `driver.list` are both ops. Hidden plumbing and
/// clap's `help` pseudo-verb are skipped.
fn verbs() -> Vec<(String, Command)> {
    fn walk(prefix: &str, cmd: &Command, out: &mut Vec<(String, Command)>) {
        for sub in cmd.get_subcommands() {
            let name = sub.get_name();
            if name == "help" || sub.is_hide_set() {
                continue;
            }
            let dotted = match prefix {
                "" => name.to_string(),
                _ => format!("{prefix}.{name}"),
            };
            if sub.has_subcommands() {
                // `sepia driver` (no sub-subcommand) is a real verb —
                // emit the node itself when its subcommand isn't
                // required, then keep walking for `driver.list`.
                if !sub.is_subcommand_required_set() {
                    out.push((dotted.clone(), sub.clone()));
                }
                walk(&dotted, sub, out);
            } else {
                out.push((dotted, sub.clone()));
            }
        }
    }
    let mut out = Vec::new();
    walk("", &sepia_cli::command(), &mut out);
    out
}

fn op_cmd(name: &str) -> Option<Command> {
    verbs()
        .into_iter()
        .find(|(n, _)| n == name && !SURFACES.contains(&n.as_str()))
        .map(|(_, c)| c)
}

/// Args that can't ride the wire: streaming flags (above) and clap's
/// own help/version actions.
fn skipped(a: &Arg) -> bool {
    SKIP_PARAMS.contains(&a.get_id().as_str())
        || matches!(
            a.get_action(),
            ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version
        )
}

/// A clap arg → the manifest's typed param kind — `arg` = required
/// positional, `opt` = `--name <v>` *or* an optional positional
/// (the generated surfaces spell it `--name`; `argv` still lands it
/// positionally), `flag` = `--name`.
fn kind(a: &Arg) -> &'static str {
    if a.is_positional() {
        if a.is_required_set() { "arg" } else { "opt" }
    } else if matches!(
        a.get_action(),
        ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Count
    ) {
        "flag"
    } else {
        "opt"
    }
}

/// The four store/agent ids every `--from`/`--to` flag accepts —
/// `StoreId` and `AgentId` spell the same set, so one enum covers
/// both the store and config verbs. `pub(crate)` — the `stores`
/// view query iterates it.
pub(crate) const STORE_IDS: &[&str] = &["devin", "cline", "claude", "cursor"];

/// `ExportFormat` tokens (`store.export --format`).
const EXPORT_FORMATS: &[&str] = &["json", "cline"];

/// A clap arg → the typed [`ParamSchema`] the ops manifest carries
/// on each param's `schema` field — hosts auto-generate action
/// forms from it (enum pickers, ref pickers, bool toggles), and
/// pavo-core's ops→actions derivation prefers it over the
/// `arg`/`opt`/`flag` kind. Name-driven upgrades on top of the
/// argv kind: store/agent selectors → enum, session ids → ref
/// picker over the `sessions` view query, formats → enum,
/// long-text fields → text, counts → integer. Anything not
/// statically knowable stays `string` — honest degradation.
fn param_schema(a: &Arg) -> ParamSchema {
    // Same wire-name rule as `param_json`: positionals keep their
    // id, flags spell their `--long`.
    let id = a.get_id().as_str();
    let name = if a.is_positional() {
        id
    } else {
        a.get_long().unwrap_or(id)
    };
    let mut b = kit::param(name).doc(a.get_help().map(ToString::to_string).unwrap_or_default());
    if a.is_required_set() {
        b = b.required();
    }
    let b = if matches!(kind(a), "flag") {
        b.kind(ParamKind::Bool)
    } else {
        match name {
            "from" | "to" => b.enumeration(STORE_IDS.iter().map(|s| (*s).to_string()).collect()),
            "format" => b.enumeration(EXPORT_FORMATS.iter().map(|s| (*s).to_string()).collect()),
            "session_id" | "id" => b.kind(ParamKind::Ref).ref_query("sessions"),
            "title" | "note" | "text" | "body" => b.text(),
            "limit" | "delta" | "count" | "before" | "turns" | "node_id" => {
                b.kind(ParamKind::Integer)
            }
            _ => b,
        }
    };
    b.build()
}

fn param_json(a: &Arg) -> Value {
    // Wire name = the flag name (`--claude-dir` → `claude-dir`), not
    // the field id (`claude_dir`) — generated surfaces spell params
    // `--<name>`. Positionals keep their id (`session_id`).
    let id = a.get_id().as_str();
    let name = if a.is_positional() {
        id
    } else {
        a.get_long().unwrap_or(id)
    };
    json!({
        "name": name,
        "kind": kind(a),
        "doc": a.get_help().map(ToString::to_string).unwrap_or_default(),
        "schema": serde_json::to_value(param_schema(a)).unwrap_or(Value::Null),
    })
}

/// The `ops` query/`ops` argv verb payload — self-describing so
/// hosts and agents discover the engine surface without argv help.
/// sepia's clap tree carries no globals — every flag is per-verb,
/// so there is no `globals` section.
pub fn manifest() -> Value {
    json!({
        "engine": "sepia",
        "invoke": "query `cli.<verb>` with the listed params, or argv `tamias-driver-sepia run <sepia argv>`",
        "ops": verbs().iter()
            .filter(|(n, _)| !SURFACES.contains(&n.as_str()))
            .map(|(n, c)| json!({
                "query": format!("cli.{n}"),
                "doc": c.get_about().map(ToString::to_string).unwrap_or_default(),
                "params": c.get_arguments()
                    .filter(|a| !skipped(a))
                    .map(param_json)
                    .collect::<Vec<_>>(),
                "mutates": MUTATES.contains(&n.as_str()),
            }))
            .collect::<Vec<_>>(),
        "surfaces": SURFACES,
    })
}

/// Is `query` an engine op (`cli.<verb>`)? Returns the verb name.
pub fn op_of(query: &str) -> Option<String> {
    query
        .strip_prefix("cli.")
        .filter(|n| op_cmd(n).is_some())
        .map(str::to_string)
}

// ------------------------------------------------------------- invoke
//
// The interaction protocol's `invoke` op (pavo-protocol
// `Request::Invoke`). Action ids ARE verb names — the ops-derived
// actions `pavo-core` builds from this manifest spell ids as the
// verb (`store.import`, `driver.list`, …); a `cli.` prefix is
// tolerated for symmetry with the query spelling. Dispatch rides
// the exact `cli.<verb>` path: params → argv → `run` child →
// engine semantics unchanged. `view` is context only (node/item
// resolution is a host concern).

/// Resolve an action id to an engine verb name — bare names and
/// the `cli.`-prefixed ops spelling both work; surfaces are not
/// actions.
fn action_verb(action: &str) -> Option<String> {
    op_of(&format!(
        "cli.{}",
        action.strip_prefix("cli.").unwrap_or(action)
    ))
}

/// A param value counts as present when it's a non-empty scalar or
/// a non-empty array — `{"session":""}` is as missing as absent.
fn present(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(_) => true,
    }
}

/// The `invoke` target→params merge rule (same shape as
/// myco-driver's): an object target contributes keys the params
/// lack (explicit params win — the form's answers override row
/// context); a scalar target fills the verb's first positional
/// param when params don't (tree-item actions send the row id
/// this way). Anything else is ignored.
fn merge_target(
    cmd: &Command,
    params: &Map<String, Value>,
    target: Option<&Value>,
) -> Map<String, Value> {
    let mut out = params.clone();
    match target {
        Some(Value::Object(t)) => {
            for (k, val) in t {
                out.entry(k.clone()).or_insert_with(|| val.clone());
            }
        }
        Some(t) => {
            if let Some(pos) = cmd.get_positionals().find(|a| !skipped(a)) {
                let id = pos.get_id().as_str();
                let ok = present(out.get(id)) || present(out.get(id.replace('_', "-").as_str()));
                if !ok {
                    if let Some(s) = scalar(t) {
                        out.insert(id.to_string(), json!(s));
                    }
                }
            }
        }
        None => {}
    }
    out
}

/// Required positionals the verb can't run without — `arg`-kind
/// params (required clap positionals). Flag/opt params are all
/// optional by construction, so only positionals gate.
fn missing_required(cmd: &Command, params: &Map<String, Value>) -> Map<String, Value> {
    let mut errors = Map::new();
    for a in cmd.get_positionals() {
        if skipped(a) || !a.is_required_set() {
            continue;
        }
        let id = a.get_id().as_str();
        let ok = present(params.get(id)) || present(params.get(id.replace('_', "-").as_str()));
        if !ok {
            errors.insert(id.to_string(), json!("required"));
        }
    }
    errors
}

/// First non-empty line — the human-facing message for an
/// [`ActionResult`].
fn first_line(s: &str) -> Option<String> {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// `{"op":"invoke"}` — dispatch `action` (a verb id) through the
/// same argv/exec path `cli.<verb>` queries use. Unknown actions
/// and spawn failures are `Err` (wire `ok:false`); verb outcomes —
/// including nonzero exits — come back `Ok` with `{code, stdout,
/// stderr}` in `data`, mirroring how `cli.*` queries report the
/// engine's own exit status as honest data.
pub fn invoke(
    action: &str,
    target: Option<&Value>,
    params: &Map<String, Value>,
) -> Result<ActionResult, String> {
    // Session-plane actions dispatch over the serve wire first —
    // no engine verb shares the `session(s).` prefix, so the lanes
    // can't collide.
    if let Some(method) = serve_method(action) {
        return Ok(serve_invoke(&method, target, params));
    }
    let Some(name) = action_verb(action) else {
        return Err(format!("unknown action: {action}"));
    };
    let Some(cmd) = op_cmd(&name) else {
        return Err(format!("unknown action: {action}"));
    };
    let merged = merge_target(&cmd, params, target);
    let errors = missing_required(&cmd, &merged);
    if !errors.is_empty() {
        return Ok(ActionResult {
            errors,
            ..ActionResult::default()
        });
    }
    let (args, o) = capture(&cmd, &name, &merged)?;
    let code = o.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
    let message = if code == 0 {
        first_line(&stdout).unwrap_or_else(|| format!("{name} ok"))
    } else {
        first_line(&stderr).unwrap_or_else(|| format!("{name} failed (exit {code})"))
    };
    Ok(ActionResult {
        message: Some(message),
        data: Some(json!({
            "op": name,
            "argv": args,
            "code": code,
            "stdout": stdout,
            "stderr": stderr,
        })),
        ..ActionResult::default()
    })
}

/// A wire scalar → its argv token (numbers/bools stringify).
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// All values of a param — arrays flatten, scalars wrap.
fn values(v: &Value) -> Vec<String> {
    match v {
        Value::Array(xs) => xs.iter().filter_map(scalar).collect(),
        other => scalar(other).into_iter().collect(),
    }
}

/// `cli.<verb>` params → sepia argv (no argv[0] — `run` prepends
/// it). Declared params map by kind — positionals land last, in
/// schema order, whether the manifest spelled them `arg` (required)
/// or `opt` (optional, e.g. `export`'s `out`).
fn argv(cmd: &Command, name: &str, params: &Map<String, Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.extend(name.split('.').map(String::from));
    for a in cmd.get_arguments() {
        if skipped(a) || a.is_positional() {
            continue;
        }
        let id = a.get_id().as_str();
        let long = a.get_long().unwrap_or(id);
        let raw = params
            .get(long)
            .or_else(|| params.get(id))
            .or_else(|| params.get(id.replace('_', "-").as_str()));
        let Some(raw) = raw else { continue };
        match kind(a) {
            "flag" => {
                if raw.as_bool().unwrap_or(false) {
                    out.push(format!("--{long}"));
                }
            }
            _ => {
                for s in values(raw) {
                    out.push(format!("--{long}"));
                    out.push(s);
                }
            }
        }
    }
    // `last = true` positionals only parse after a `--` separator —
    // sepia has none today, but keep the rule so a future trailing
    // arg verb can't silently misparses.
    if cmd.get_positionals().any(Arg::is_last_set) {
        out.push("--".into());
    }
    let mut positionals: Vec<String> = Vec::new();
    for a in cmd.get_positionals() {
        if skipped(a) {
            continue;
        }
        let id = a.get_id().as_str();
        if let Some(raw) = params
            .get(id)
            .or_else(|| params.get(id.replace('_', "-").as_str()))
        {
            positionals.extend(values(raw));
        }
    }
    out.extend(positionals);
    out
}

/// The bare-name entry the embedded surfaces use (`OpSpec.name` is
/// `cli.`-stripped); the wire path goes `op_of` → `run_verb`.
#[cfg(feature = "mcp")]
pub fn run_op(op: &str, params: &Map<String, Value>) -> Response {
    match op_of(&format!("cli.{}", op.strip_prefix("cli.").unwrap_or(op))) {
        Some(name) => run_verb(&name, params),
        None => Response::err(format!("unknown op: {op}")),
    }
}

/// Run one engine op by name: re-exec `run` in a child with
/// captured output, and hand back `{op, argv, code, stdout,
/// stderr}` as the query payload. `ok:false` is reserved for spawn
/// failures — a nonzero `code` is the engine's own exit status
/// (e.g. a missing session id), honest data.
pub fn run_verb(name: &str, params: &Map<String, Value>) -> Response {
    let Some(cmd) = op_cmd(name) else {
        return Response::err(format!("unknown op: {name}"));
    };
    let data = match capture(&cmd, name, params) {
        Ok((args, o)) => json!({
            "op": name,
            "argv": args,
            "code": o.status.code().unwrap_or(-1),
            "stdout": String::from_utf8_lossy(&o.stdout),
            "stderr": String::from_utf8_lossy(&o.stderr),
        }),
        Err(e) => return Response::err(format!("run {name}: {e}")),
    };
    Response::ok_typed(&QueryResult {
        query: format!("cli.{name}"),
        data,
    })
}

/// Captured engine-verb output for *view queries* — the same
/// argv/exec path `cli.<verb>` uses, minus the wire envelope:
/// callers parse `stdout` themselves (`store list` is TSV,
/// `driver list --json` is JSON). A nonzero `code` is the engine's
/// own exit status — honest data the query shaper degrades on.
pub(crate) struct VerbOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run one engine verb by name for a view query. `Err` is reserved
/// for unknown verbs and spawn failures, same as [`run_verb`].
pub(crate) fn verb_output(name: &str, params: &Map<String, Value>) -> Result<VerbOutput, String> {
    let Some(cmd) = op_cmd(name) else {
        return Err(format!("unknown op: {name}"));
    };
    let (_args, o) = capture(&cmd, name, params)?;
    Ok(VerbOutput {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    })
}

/// Params → argv → `run` child with captured output — the shared
/// tail of `cli.<verb>` queries and `invoke`. The child's stdin is
/// null: no sepia op needs it — `serve` is a surface, and prompts
/// never ride argv.
fn capture(
    cmd: &Command,
    name: &str,
    params: &Map<String, Value>,
) -> Result<(Vec<String>, std::process::Output), String> {
    let args = argv(cmd, name, params);
    let out = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("run")
            .args(&args)
            .output()
    });
    out.map(|o| (args, o)).map_err(|e| e.to_string())
}

// ---------------------------------------------------------- serve lane
//
// The session-plane half of `invoke`. Action ids `session.<verb>`
// (the `sessions.<verb>` spelling works too) name `sepia serve`
// methods — the plane operations that have no CLI verb:
// `sessions.attach`, `sessions.prompt`, `sessions.patch_meta`, …
// They ride [`data::call`], the same persistent `sepia serve`
// child the view queries use, so a mutation sees exactly the plane
// an orchestrator would. `sessions.subscribe` stays off the action
// surface (an unbounded notification feed, not a request op), as do
// `manifest`/`agents.*`/`config.*`/`shutdown` — `config.*` in
// particular has a different meaning on each wire (serve's
// agent-config IR vs the engine's file verbs), so only the
// `cli.config.*` spelling reaches the engine lane.
//
// Params are the serve wire's camelCase JSON, mostly verbatim —
// the translations are the ergonomic ones: `text` becomes
// `parts: [{"type":"text",…}]` for prompt/prompt_wait, flat
// selectors become the `request`/`patch` objects restore/rewind/
// patch_meta expect, and `path` reads a session-IR JSON file for
// `sessions.import`. `id` aliases (`session_id`, `sessionId`)
// normalize to the wire's `id`; `agent`/`sepiaBin` in params also
// steer the serve child itself (data::call's ctx is the merged
// param map).

/// The `sessions.*` verbs exposed as actions — everything bounded
/// on the method table except the subscribe feeds.
const SERVE_ACTIONS: &[&str] = &[
    "list",
    "get",
    "summary",
    "export",
    "history",
    "attach",
    "detach",
    "create",
    "prompt",
    "prompt_wait",
    "cancel",
    "permission",
    "set_mode",
    "set_permission_policy",
    "delete",
    "patch_meta",
    "restore",
    "rewind",
    "import",
    "convert",
];

/// `session.<verb>` / `sessions.<verb>` → the serve method name.
/// Anything else returns `None` and falls through to the `cli.*`
/// lane.
pub(crate) fn serve_method(action: &str) -> Option<String> {
    let verb = action
        .strip_prefix("session.")
        .or_else(|| action.strip_prefix("sessions."))?;
    SERVE_ACTIONS
        .contains(&verb)
        .then(|| format!("sessions.{verb}"))
}

/// First present value among `keys` — param aliases (`session_id`,
/// `sessionId`, `project_ids`, `tool_call_id`, …) resolve to the
/// wire's canonical camelCase name at translation time.
fn get<'a>(params: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| params.get(*k))
}

/// Copy the first present alias into `out` under the wire name.
fn put(out: &mut Map<String, Value>, keys: &[&str], to: &str, params: &Map<String, Value>) {
    if let Some(v) = get(params, keys) {
        out.insert(to.to_string(), v.clone());
    }
}

/// The session id — `id` canonical, `session_id`/`sessionId`
/// tolerated (scalar targets and hand-written params land either).
fn session_id(params: &Map<String, Value>) -> Option<&Value> {
    get(params, &["id", "session_id", "sessionId"])
}

/// Params a serve method can't run without, beyond the shared `id`
/// rule (every session-scoped method needs it; `list`/`create`/
/// `import` don't). `import`'s `session` is satisfied by the
/// `session` object *or* a `path` to a session-IR JSON file —
/// checked in [`serve_missing`] rather than here.
fn serve_required(method: &str) -> &'static [&'static str] {
    match method {
        "sessions.create" => &["cwd"],
        "sessions.import" | "sessions.convert" => &["agent"],
        "sessions.permission" => &["request"],
        "sessions.set_mode" => &["mode"],
        "sessions.set_permission_policy" => &["permissions"],
        _ => &[],
    }
}

/// Required-param gating for the serve lane — same honesty as the
/// cli lane's `missing_required`: absent/empty required params come
/// back keyed by name in `errors`, never as a wire call.
fn serve_missing(method: &str, params: &Map<String, Value>) -> Map<String, Value> {
    let mut errors = Map::new();
    let needs_id = !matches!(
        method,
        "sessions.list" | "sessions.create" | "sessions.import"
    );
    if needs_id && !present(session_id(params)) {
        errors.insert("id".to_string(), json!("required"));
    }
    for key in serve_required(method) {
        if !present(params.get(*key)) {
            errors.insert((*key).to_string(), json!("required"));
        }
    }
    if method == "sessions.import"
        && !present(params.get("session"))
        && !present(params.get("path"))
        && !present(params.get("file"))
    {
        errors.insert("session".to_string(), json!("required (object or path)"));
    }
    if matches!(method, "sessions.prompt" | "sessions.prompt_wait")
        && get(params, &["parts", "text", "prompt", "message"]).is_none_or(|v| !present(Some(v)))
    {
        errors.insert("text".to_string(), json!("required"));
    }
    errors
}

/// The serve lane's target→params merge — same rule as the cli
/// lane's `merge_target`: an object target contributes keys the
/// params lack (a selected table row lands `{"id": …}` this way);
/// a scalar target fills `id` when params don't.
fn merge_serve_target(params: &Map<String, Value>, target: Option<&Value>) -> Map<String, Value> {
    let mut out = params.clone();
    match target {
        Some(Value::Object(t)) => {
            for (k, val) in t {
                out.entry(k.clone()).or_insert_with(|| val.clone());
            }
        }
        Some(t) if session_id(&out).is_none() => {
            if let Some(s) = scalar(t) {
                out.insert("id".to_string(), json!(s));
            }
        }
        _ => {}
    }
    out
}

/// `text`/`prompt`/`message`/`parts` → the wire's `parts` array.
/// A `parts` array passes through verbatim (multi-part prompts);
/// a string becomes one `{"type":"text","text":…}` PromptPart —
/// the serde shape `sepia_acp::types::PromptPart` expects
/// (`#[serde(tag = "type", rename_all = "camelCase")]`).
fn parts_param(params: &Map<String, Value>) -> Option<Value> {
    if let Some(parts) = get(params, &["parts"]) {
        if parts.is_array() {
            return Some(parts.clone());
        }
    }
    get(params, &["text", "prompt", "message"])
        .and_then(Value::as_str)
        .map(|t| json!([{"type": "text", "text": t}]))
}

/// `patch_meta`'s `patch` — a `patch` object passes through; flat
/// params (`title`, `pinned`, `archived`, `projectIds`, `model`)
/// assemble the `SessionMeta` wire shape instead.
fn meta_patch(params: &Map<String, Value>) -> Value {
    if let Some(patch) = get(params, &["patch"]) {
        if patch.is_object() {
            return patch.clone();
        }
    }
    let mut patch = Map::new();
    // `title`/`model` only take non-empty strings from flat params —
    // a form's untouched optional field arrives as null/"" and must
    // not clear the stored value (the wire maps `model: null` to an
    // explicit clear; that stays reachable through `patch`).
    put_text(&mut patch, &["title"], "title", params);
    put(&mut patch, &["pinned"], "pinned", params);
    put(&mut patch, &["archived"], "archived", params);
    put(
        &mut patch,
        &["projectIds", "project_ids"],
        "projectIds",
        params,
    );
    put_text(&mut patch, &["model"], "model", params);
    Value::Object(patch)
}

/// [`put`] for string-valued patch fields — copies only a non-empty
/// string, so null/"" (an untouched form field) means "keep".
fn put_text(out: &mut Map<String, Value>, keys: &[&str], to: &str, params: &Map<String, Value>) {
    if let Some(v) = get(params, keys)
        && v.as_str().is_some_and(|s| !s.is_empty())
    {
        out.insert(to.to_string(), v.clone());
    }
}

/// `restore`'s `request` — `RestoreRequest` wire shape
/// (`confirm` mandatory on the wire; the action's danger level is
/// the host-side gate, so an unset `confirm` param defaults true).
fn restore_request(params: &Map<String, Value>) -> Value {
    let mut request = Map::new();
    request.insert(
        "confirm".to_string(),
        get(params, &["confirm"]).cloned().unwrap_or(json!(true)),
    );
    put(&mut request, &["path"], "path", params);
    put(
        &mut request,
        &["toolCallId", "tool_call_id"],
        "toolCallId",
        params,
    );
    put(&mut request, &["checkpoint"], "checkpoint", params);
    put(&mut request, &["paths"], "paths", params);
    Value::Object(request)
}

/// `rewind`'s `request` — `RewindRequest` wire shape; exactly one
/// of `nodeId`/`turns`/`checkpoint` is the caller's job (the plane
/// validates), `confirm` defaults true like [`restore_request`].
fn rewind_request(params: &Map<String, Value>) -> Value {
    let mut request = Map::new();
    request.insert(
        "confirm".to_string(),
        get(params, &["confirm"]).cloned().unwrap_or(json!(true)),
    );
    put(&mut request, &["nodeId", "node_id"], "nodeId", params);
    put(&mut request, &["turns"], "turns", params);
    put(&mut request, &["checkpoint"], "checkpoint", params);
    Value::Object(request)
}

/// `sessions.import`'s `session` — an inline session-IR object, or
/// `path`/`file` naming a JSON file to read and parse. Read/parse
/// failures come back as param-keyed errors, not wire calls.
fn import_session(params: &Map<String, Value>) -> Result<Value, Map<String, Value>> {
    if let Some(session) = get(params, &["session"]) {
        return Ok(session.clone());
    }
    let Some(path) = get(params, &["path", "file"]).and_then(Value::as_str) else {
        return Err(Map::from_iter([(
            "session".to_string(),
            json!("required (object or path)"),
        )]));
    };
    let text = std::fs::read_to_string(path).map_err(|e| {
        Map::from_iter([(
            "path".to_string(),
            json!(format!("cannot read '{path}': {e}")),
        )])
    })?;
    serde_json::from_str(&text).map_err(|e| {
        Map::from_iter([(
            "path".to_string(),
            json!(format!("'{path}' is not session IR JSON: {e}")),
        )])
    })
}

/// Invoke params → the serve method's wire params. Explicit per
/// method — the wire is camelCase and mostly 1:1, so each arm is a
/// short `put` list plus the documented translations.
fn serve_params(method: &str, params: &Map<String, Value>) -> Result<Value, Map<String, Value>> {
    let mut out = Map::new();
    // `id` + `agent` are shared by nearly every method.
    let scoped = !matches!(
        method,
        "sessions.list" | "sessions.create" | "sessions.import"
    );
    if scoped {
        if let Some(id) = session_id(params) {
            out.insert("id".to_string(), id.clone());
        }
    }
    put(&mut out, &["agent"], "agent", params);
    match method {
        "sessions.list" => {
            put(&mut out, &["withLocks", "with_locks"], "withLocks", params);
        }
        "sessions.history" => {
            put(&mut out, &["limit"], "limit", params);
            put(&mut out, &["before"], "before", params);
        }
        "sessions.attach" => {
            put(&mut out, &["takeover"], "takeover", params);
            put(&mut out, &["model"], "model", params);
            put(&mut out, &["fallbacks"], "fallbacks", params);
            put(&mut out, &["permissions"], "permissions", params);
        }
        "sessions.create" => {
            put(&mut out, &["cwd"], "cwd", params);
            put(&mut out, &["title"], "title", params);
            put(&mut out, &["model"], "model", params);
            put(&mut out, &["fallbacks"], "fallbacks", params);
            put(&mut out, &["permissions"], "permissions", params);
        }
        "sessions.prompt" | "sessions.prompt_wait" => {
            if let Some(parts) = parts_param(params) {
                out.insert("parts".to_string(), parts);
            }
            if method == "sessions.prompt_wait" {
                put(
                    &mut out,
                    &["timeoutMs", "timeout_ms", "timeout"],
                    "timeoutMs",
                    params,
                );
            }
        }
        "sessions.permission" => {
            put(&mut out, &["request"], "request", params);
            put(&mut out, &["option"], "option", params);
        }
        "sessions.set_mode" => {
            put(&mut out, &["mode"], "mode", params);
        }
        "sessions.set_permission_policy" => {
            put(&mut out, &["permissions"], "permissions", params);
        }
        "sessions.patch_meta" => {
            out.insert("patch".to_string(), meta_patch(params));
        }
        "sessions.restore" => {
            out.insert("request".to_string(), restore_request(params));
        }
        "sessions.rewind" => {
            out.insert("request".to_string(), rewind_request(params));
        }
        "sessions.import" => {
            out.insert("session".to_string(), import_session(params)?);
            for key in ["id", "cwd", "title", "model"] {
                put(&mut out, &[key], key, params);
            }
        }
        "sessions.convert" => {
            put(&mut out, &["from"], "from", params);
        }
        // get/summary/export/history/detach/cancel/delete need
        // nothing beyond id+agent (+history's own keys above).
        _ => {}
    }
    Ok(Value::Object(out))
}

/// The human-facing [`ActionResult::message`] for a serve reply —
/// methods that mint an id name it, the rest report ok.
fn serve_message(method: &str, result: &Value) -> String {
    let short = method.trim_start_matches("sessions.");
    if let Some(id) = result["id"]
        .as_str()
        .or_else(|| result["sessionId"].as_str())
    {
        format!("{short}: {id}")
    } else {
        format!("{short} ok")
    }
}

/// The serve lane's [`invoke`] tail — merged params → wire params →
/// [`data::mutate`] on the shared `sepia serve` child. Wire errors
/// (control failures, invalid params) come back as `message` +
/// `data.error`, the same honesty convention the cli lane uses for
/// nonzero exits; validation failures stay in `errors`.
fn serve_invoke(method: &str, target: Option<&Value>, params: &Map<String, Value>) -> ActionResult {
    let merged = merge_serve_target(params, target);
    let mut errors = serve_missing(method, &merged);
    if errors.is_empty() {
        match serve_params(method, &merged) {
            Ok(p) => {
                return match data::mutate(method, p, &merged) {
                    Ok(result) => ActionResult {
                        message: Some(serve_message(method, &result)),
                        data: Some(json!({
                            "method": method,
                            "result": result,
                        })),
                        ..ActionResult::default()
                    },
                    Err(e) => ActionResult {
                        message: Some(first_line(&e).unwrap_or_else(|| e.clone())),
                        data: Some(json!({
                            "method": method,
                            "error": e,
                        })),
                        ..ActionResult::default()
                    },
                };
            }
            Err(e) => errors = e,
        }
    }
    ActionResult {
        errors,
        ..ActionResult::default()
    }
}

/// `{"op":"submit"}` — the form-interaction adapter. Resolution
/// order: a declared `form` node's `submit_op` names the action
/// first (the session view's meta form submits
/// `session.patch_meta` — see `views::form_submit_op`); failing
/// that, the `form`/view id itself names a known action — a serve
/// action (`session.*`) or an engine verb — with `values` as its
/// params. Anything else is `ok:false`.
///
/// The wire `submit` op carries no view params, so a session-scoped
/// serve action picks up `id`/`agent` from the session view's
/// last-resolved context ([`crate::views::session_context`]) when
/// `values` doesn't supply them — the meta form's editable fields
/// are title/pinned/archived/model only.
pub fn submit(view: &str, form: Option<&str>, values: &Map<String, Value>) -> Response {
    let candidate = crate::views::form_submit_op(view, form)
        .unwrap_or_else(|| form.unwrap_or(view).to_string());
    if serve_method(&candidate).is_none() && action_verb(&candidate).is_none() {
        return Response::err(format!(
            "unknown submit target '{candidate}' on view '{view}'"
        ));
    }
    // Session-scoped serve actions merge the view's resolved
    // id/agent when values don't carry them.
    let owned;
    let values = if serve_method(&candidate).is_some() {
        owned = merge_session_ctx(values);
        &owned
    } else {
        values
    };
    match invoke(&candidate, None, values) {
        Ok(r) => Response::ok_typed(&SubmitResult {
            message: r.message,
            errors: r.errors,
            data: r.data,
            view: r.view,
            extra: r.extra,
        }),
        Err(e) => Response::err(e),
    }
}

/// `values` + the session view's last-resolved `id`/`agent` —
/// explicit values win; the context only fills what they lack
/// (`id`/`session_id`/`sessionId` count as present for `id`).
fn merge_session_ctx(values: &Map<String, Value>) -> Map<String, Value> {
    let mut out = values.clone();
    let Some(ctx) = crate::views::session_context() else {
        return out;
    };
    if session_id(&out).is_none()
        && let Some(id) = ctx.get("id").and_then(Value::as_str)
    {
        out.insert("id".to_string(), json!(id));
    }
    if !present(out.get("agent"))
        && let Some(agent) = ctx.get("agent").and_then(Value::as_str)
    {
        out.insert("agent".to_string(), json!(agent));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn op_of_accepts_cli_prefix_only() {
        assert_eq!(op_of("cli.version").unwrap(), "version");
        assert_eq!(op_of("cli.store.list").unwrap(), "store.list");
        assert_eq!(op_of("cli.driver").unwrap(), "driver");
        assert_eq!(op_of("cli.driver.list").unwrap(), "driver.list");
        assert!(op_of("cli.serve").is_none()); // unbounded surface
        assert!(op_of("list").is_none()); // view query, not an op
        assert!(op_of("cli.bogus").is_none());
    }

    #[test]
    fn manifest_lists_every_non_surface_verb_as_cli_query() {
        let m = manifest();
        let ops = m["ops"].as_array().unwrap();
        for (n, _) in verbs()
            .iter()
            .filter(|(n, _)| !SURFACES.contains(&n.as_str()))
        {
            assert!(
                ops.iter().any(|o| o["query"] == json!(format!("cli.{n}"))),
                "{n} missing from ops manifest"
            );
        }
        assert_eq!(m["surfaces"], json!(SURFACES));
        for o in ops {
            for p in o["params"].as_array().unwrap() {
                assert!(["arg", "opt", "flag"].contains(&p["kind"].as_str().unwrap()));
                // streaming/stdin params never ride the wire
                assert!(!SKIP_PARAMS.contains(&p["name"].as_str().unwrap()));
            }
        }
        // mutates honesty — the side-effect set is exactly these
        let mutators: Vec<&str> = ops
            .iter()
            .filter(|o| o["mutates"] == json!(true))
            .filter_map(|o| o["query"].as_str())
            .collect();
        assert_eq!(
            mutators,
            vec![
                "cli.config.import",
                "cli.config.install",
                "cli.store.import",
                "cli.store.install",
                "cli.store.delete",
                "cli.import",
                "cli.install",
                "cli.delete",
            ]
        );
    }

    #[test]
    fn argv_maps_params_to_sepia_flags() {
        let cmd = op_cmd("store.export").unwrap();
        let mut p = Map::new();
        p.insert("session_id".into(), json!("sess-1"));
        p.insert("out".into(), json!("/tmp/s.json"));
        p.insert("format".into(), json!("cline"));
        p.insert("from".into(), json!("claude"));
        p.insert("force".into(), json!(true)); // not a store.export param — ignored
        assert_eq!(
            argv(&cmd, "store.export", &p),
            vec![
                "store",
                "export",
                "--format",
                "cline",
                "--from",
                "claude",
                "sess-1",
                "/tmp/s.json",
            ]
        );
    }

    #[test]
    fn argv_omits_absent_optionals() {
        let cmd = op_cmd("version").unwrap();
        assert_eq!(argv(&cmd, "version", &Map::new()), vec!["version"]);
        let cmd = op_cmd("store.delete").unwrap();
        let mut p = Map::new();
        p.insert("session_id".into(), json!("abc"));
        p.insert("from".into(), json!("devin"));
        assert_eq!(
            argv(&cmd, "store.delete", &p),
            vec!["store", "delete", "--from", "devin", "abc"]
        );
    }

    #[test]
    fn ops_params_carry_typed_schemas() {
        let m = manifest();
        let ops = m["ops"].as_array().unwrap();
        let find = |query: &str| {
            ops.iter()
                .find(|o| o["query"] == json!(query))
                .unwrap_or_else(|| panic!("{query} missing"))
        };
        // store/agent selectors → enum over the four store ids
        let export = find("cli.store.export");
        let from = export["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "from")
            .unwrap();
        assert_eq!(from["schema"]["kind"], "enum");
        assert_eq!(
            from["schema"]["options"],
            json!(["devin", "cline", "claude", "cursor"])
        );
        // session ids → ref picker over the sessions view query
        let del = find("cli.store.delete");
        let sid = del["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "session_id")
            .unwrap();
        assert_eq!(sid["schema"]["kind"], "ref");
        assert_eq!(sid["schema"]["ref_query"], "sessions");
        assert_eq!(sid["schema"]["required"], json!(true));
        // export format → enum; flags → bool
        let fmt = export["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "format")
            .unwrap();
        assert_eq!(fmt["schema"]["kind"], "enum");
        assert_eq!(fmt["schema"]["options"], json!(["json", "cline"]));
        let drv = find("cli.driver.list");
        let json_flag = drv["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "json")
            .unwrap();
        assert_eq!(json_flag["schema"]["kind"], "bool");
    }

    #[test]
    fn serve_method_routes_session_ids_only() {
        assert_eq!(serve_method("session.prompt").unwrap(), "sessions.prompt");
        assert_eq!(
            serve_method("sessions.patch_meta").unwrap(),
            "sessions.patch_meta"
        );
        assert!(serve_method("session.bogus").is_none());
        assert!(serve_method("sessions.subscribe").is_none()); // unbounded feed
        assert!(serve_method("store.delete").is_none()); // cli lane
        assert!(serve_method("session").is_none());
        assert!(serve_method("prompt").is_none());
    }

    #[test]
    fn serve_params_translates_prompt_text_to_parts() {
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        p.insert("text".into(), json!("fix the bug"));
        p.insert("agent".into(), json!("devin"));
        let v = serve_params("sessions.prompt", &p).unwrap();
        assert_eq!(v["id"], "s1");
        assert_eq!(v["agent"], "devin");
        assert_eq!(v["parts"], json!([{"type": "text", "text": "fix the bug"}]));
        // parts passthrough wins over text
        p.insert("parts".into(), json!([{"type": "text", "text": "a"}]));
        let v = serve_params("sessions.prompt", &p).unwrap();
        assert_eq!(v["parts"][0]["text"], "a");
        // id aliases normalize
        let mut p2 = Map::new();
        p2.insert("session_id".into(), json!("s2"));
        p2.insert("text".into(), json!("hi"));
        let v = serve_params("sessions.prompt_wait", &p2).unwrap();
        assert_eq!(v["id"], "s2");
        assert!(v.get("timeoutMs").is_none());
    }

    #[test]
    fn serve_params_builds_request_and_patch_objects() {
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        p.insert("turns".into(), json!(2));
        p.insert("tool_call_id".into(), json!("tc-9"));
        let v = serve_params("sessions.rewind", &p).unwrap();
        assert_eq!(v["request"]["confirm"], json!(true));
        assert_eq!(v["request"]["turns"], 2);
        // restore aliases land camelCase
        let v = serve_params("sessions.restore", &p).unwrap();
        assert_eq!(v["request"]["toolCallId"], "tc-9");
        // flat patch_meta params assemble the SessionMeta shape
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        p.insert("title".into(), json!("renamed"));
        p.insert("pinned".into(), json!(true));
        p.insert("project_ids".into(), json!(["p1"]));
        let v = serve_params("sessions.patch_meta", &p).unwrap();
        assert_eq!(v["patch"]["title"], "renamed");
        assert_eq!(v["patch"]["pinned"], true);
        assert_eq!(v["patch"]["projectIds"], json!(["p1"]));
    }

    #[test]
    fn serve_missing_gates_required_params() {
        // every session-scoped method needs id
        let mut p = Map::new();
        assert!(serve_missing("sessions.delete", &p).contains_key("id"));
        p.insert("id".into(), json!("s1"));
        assert!(serve_missing("sessions.delete", &p).is_empty());
        // create needs cwd, not id
        let mut p = Map::new();
        p.insert("cwd".into(), json!("/tmp"));
        assert!(serve_missing("sessions.create", &p).is_empty());
        // import needs agent + session|path
        let mut p = Map::new();
        p.insert("agent".into(), json!("fake"));
        let e = serve_missing("sessions.import", &p);
        assert!(e.contains_key("session"), "{e:?}");
        p.insert("path".into(), json!("/tmp/s.json"));
        assert!(serve_missing("sessions.import", &p).is_empty());
        // prompt needs text|parts
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        assert!(serve_missing("sessions.prompt", &p).contains_key("text"));
        p.insert("text".into(), json!("go"));
        assert!(serve_missing("sessions.prompt", &p).is_empty());
    }

    #[test]
    fn merge_serve_target_fills_id() {
        let mut p = Map::new();
        p.insert("agent".into(), json!("devin"));
        // scalar target → id
        let m = merge_serve_target(&p, Some(&json!("sess-1")));
        assert_eq!(m["id"], "sess-1");
        // explicit param wins
        let m = merge_serve_target(&m, Some(&json!("other")));
        assert_eq!(m["id"], "sess-1");
        // object target contributes missing keys only
        let m = merge_serve_target(&Map::new(), Some(&json!({"id": "row-1", "title": "T"})));
        assert_eq!(m["id"], "row-1");
        assert_eq!(m["title"], "T");
    }
}
