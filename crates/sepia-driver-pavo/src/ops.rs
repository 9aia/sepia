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
//!
//! The same session-plane methods are also exposed as *query* ops —
//! `sessions.<method>` (e.g. `{"op":"query","query":"sessions.
//! prompt_wait","params":{…}}`) — so orchestrators driving
//! `tamias serve`'s `driver.query` (peregrin `driver: ops` campaign
//! nodes) get typed request/response ops instead of shell+curl.
//! Every bounded serve method is a query op; `sessions.subscribe`
//! stays surface-only (an unbounded notification feed). Query
//! params are the serve wire's camelCase JSON — the same fields the
//! method table in `sepia-cli/src/serve.rs` documents — plus the
//! invoke lane's ergonomic aliases (`text`→`parts`, flat
//! `title`/`pinned`/…→`patch`, `request` objects pass through
//! verbatim for restore/rewind). See [`run_serve_query`].

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
    let mut ops: Vec<Value> = verbs()
        .iter()
        .filter(|(n, _)| !SURFACES.contains(&n.as_str()))
        .map(|(n, c)| {
            json!({
                "query": format!("cli.{n}"),
                "doc": c.get_about().map(ToString::to_string).unwrap_or_default(),
                "params": c.get_arguments()
                    .filter(|a| !skipped(a))
                    .map(param_json)
                    .collect::<Vec<_>>(),
                "mutates": MUTATES.contains(&n.as_str()),
            })
        })
        .collect();
    ops.extend(serve_manifest_ops());
    json!({
        "engine": "sepia",
        "invoke": "query `cli.<verb>` (engine verbs) or `sessions.<method>` (session plane) with the listed params, or argv `tamias-driver-sepia run <sepia argv>`",
        "ops": ops,
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
    view: Option<&str>,
    target: Option<&Value>,
    params: &Map<String, Value>,
) -> Result<ActionResult, String> {
    // Session-plane actions dispatch over the serve wire first —
    // no engine verb shares the `session(s).` prefix, so the lanes
    // can't collide.
    if let Some(method) = serve_method(action) {
        return Ok(serve_invoke(&method, view, target, params));
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

/// `cli.<verb>` params → sepia argv (no argv\[0\] — `run` prepends
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
    // `sessions.<method>`/`session.<method>` names route to the
    // session-plane query lane; everything else is a `cli.<verb>`
    // engine op (`OpSpec.name` is already `cli.`-stripped).
    if serve_method(op).is_some() {
        return run_serve_query(op, params);
    }
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
    "run",
    "commands",
    "command",
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

// ------------------------------------------------- sessions query ops
//
// `sessions.<method>` wire *queries* — the same bounded serve
// methods the invoke lane's `session.*` actions dispatch, exposed
// as request/response ops so `tamias serve`'s `driver.query` (and
// any ops-manifest consumer) can drive the session plane as typed
// ops. `sessions.subscribe` is deliberately absent — it is an
// unbounded notification feed, not a request/response op.
//
// Params are the serve wire's camelCase JSON (the method table in
// `sepia-cli/src/serve.rs`), translated by the same [`serve_params`]
// the invoke lane uses — so wire-native fields pass through and the
// ergonomic aliases (`text`, flat `patch`/`request` fields,
// `session_id`/`sessionId` for `id`) work identically on both
// lanes. `sepiaBin` (also `sepia`/`bin`, `$SEPIA_BIN`) steers which
// `sepia serve` child answers — it is a transport param, never sent
// to the plane.

/// One manifest row per [`SERVE_ACTIONS`] verb — `sessions.<verb>`
/// query ops appended to the `cli.*` ops in [`manifest`].
fn serve_manifest_ops() -> Vec<Value> {
    SERVE_ACTIONS
        .iter()
        .map(|verb| {
            json!({
                "query": format!("sessions.{verb}"),
                "doc": serve_op_doc(verb),
                "params": serve_op_params(verb)
                    .iter()
                    .map(serve_param_json)
                    .collect::<Vec<_>>(),
                "mutates": serve_op_mutates(verb),
            })
        })
        .collect()
}

/// The sessions ops' one-line docs — what the method does on the
/// plane, not how the wire spells it.
fn serve_op_doc(verb: &str) -> &'static str {
    match verb {
        "list" => "list sessions across agent stores (`withLocks` adds lock holders)",
        "get" => "full session IR for one session",
        "summary" => "session summary row for one session",
        "export" => "session IR wrapped as `{session}` — the transfer bundle",
        "history" => "paginated session history (`limit`/`before` cursor)",
        "attach" => "attach to a session's agent runtime (lock, model, permissions)",
        "detach" => "detach from a session's agent runtime",
        "create" => "create a session in `cwd` under an agent",
        "prompt" => "send prompt `parts`/`text` to a session (no wait)",
        "prompt_wait" => {
            "prompt and await turn end → PromptWaitResult (`timeoutMs` bounds the wait)"
        }
        "run" => {
            "one-call turn: attach `id` or create from `cwd`+`agent`, optional `mode`, prompt `text`/`parts`, await → PromptWaitResult"
        }
        "commands" => "the agent's advertised slash commands (`available_commands_update`)",
        "command" => {
            "invoke an advertised slash command as a prompt turn → PromptWaitResult (`name`, `args?`/`text?`, `timeoutMs?`)"
        }
        "cancel" => "cancel the session's in-flight turn",
        "permission" => "answer a pending `session/request_permission` (`request` id + `option`)",
        "set_mode" => "set the session's mode (`mode` id)",
        "set_permission_policy" => "set the auto-answer policy for permission requests",
        "delete" => "delete a session",
        "patch_meta" => {
            "patch the meta overlay (`patch` object or flat title/pinned/archived/projectIds/model)"
        }
        "restore" => "restore a checkpoint/file state (`request` object or flat fields)",
        "rewind" => "rewind to a node/turn/checkpoint (`request` object or flat fields)",
        "import" => "write a session IR object into an agent's store (`session` or `path`/`file`)",
        "convert" => "copy a session into another agent's store (`from`)",
        _ => "session plane method",
    }
}

/// Read-only serve methods — everything else mutates plane state
/// (locks, stores, checkpoints, the meta overlay).
fn serve_op_mutates(verb: &str) -> bool {
    !matches!(
        verb,
        "list" | "get" | "summary" | "export" | "history" | "commands"
    )
}

/// A sessions-op param → the manifest's `{name,kind,doc,schema}`
/// row. `kind` is always `"opt"` — these are named wire params, not
/// argv; the typed [`ParamSchema`] in `schema` carries the real
/// widget hints (hosts prefer it, per pavo-core's ops→actions
/// derivation).
fn serve_param_json(p: &ParamSchema) -> Value {
    json!({
        "name": p.name,
        "kind": "opt",
        "doc": p.doc.clone().unwrap_or_default(),
        "schema": serde_json::to_value(p).unwrap_or(Value::Null),
    })
}

/// The typed params each `sessions.*` op advertises — the serve
/// wire's own field names (camelCase), with `id` ref-picked over
/// the `sessions` view query and `agent` enumerated over the four
/// store ids. Requiredness mirrors [`serve_missing`]'s gate.
fn serve_op_params(verb: &str) -> Vec<ParamSchema> {
    // Shared builders.
    let id = || -> ParamSchema {
        kit::param("id")
            .kind(ParamKind::Ref)
            .ref_query("sessions")
            .required()
            .doc("session id")
            .build()
    };
    let agent = |required: bool| -> ParamSchema {
        let b = kit::param("agent")
            .enumeration(STORE_IDS.iter().map(|s| (*s).to_string()).collect())
            .doc("agent id — session ids collide across agent stores");
        (if required { b.required() } else { b }).build()
    };
    let bin = || -> ParamSchema {
        kit::param("sepiaBin")
            .doc("path to a `sepia`-shaped executable — pins the serve child for this call (else $SEPIA_BIN, a sibling `sepia`, or this driver's own `run serve`)")
            .build()
    };
    let flag = |name: &str, doc: &str| kit::param(name).kind(ParamKind::Bool).doc(doc).build();
    let int = |name: &str, doc: &str| kit::param(name).kind(ParamKind::Integer).doc(doc).build();
    let text = |name: &str, doc: &str| kit::param(name).kind(ParamKind::Text).doc(doc).build();
    let string_list = |name: &str, doc: &str| {
        kit::param(name)
            .kind(ParamKind::StringList)
            .doc(doc)
            .build()
    };
    let string = |name: &str, doc: &str| kit::param(name).doc(doc).build();

    let mut v = Vec::new();
    match verb {
        "list" => v.extend([flag("withLocks", "include per-session lock holders"), bin()]),
        "get" | "summary" | "export" | "cancel" | "delete" => {
            v.extend([id(), agent(false), bin()]);
        }
        "history" => v.extend([
            id(),
            agent(false),
            bin(),
            int("limit", "max entries to return"),
            int("before", "sequence cursor — entries before this seq"),
        ]),
        "attach" => v.extend([
            id(),
            agent(false),
            bin(),
            flag("takeover", "take the lock from another holder"),
            string("model", "model override for the attach"),
            string_list("fallbacks", "fallback model ids"),
            text(
                "permissions",
                "PermissionPolicy JSON — 'ask'/'allow_all'/'deny_all'/'{\"rules\":[…]}'; applied only on a live attach",
            ),
        ]),
        "detach" => v.extend([id(), bin()]),
        "create" => v.extend([
            kit::param("cwd")
                .required()
                .doc("working directory for the new session")
                .build(),
            agent(false),
            bin(),
            string("title", "session title"),
            string("model", "model id"),
            string_list("fallbacks", "fallback model ids"),
            text("permissions", "create-time PermissionPolicy JSON"),
        ]),
        "prompt" | "prompt_wait" => {
            v.extend([id(), agent(false), bin()]);
            v.push(text(
                "text",
                "prompt text — shorthand for one text PromptPart (`parts` wins when both are given)",
            ));
            v.push(text(
                "parts",
                "JSON array of ACP PromptPart objects — the verbatim wire form",
            ));
            if verb == "prompt_wait" {
                v.push(int(
                    "timeoutMs",
                    "wait bound in ms — 0/absent waits until the turn settles; on expiry the turn is cancelled",
                ));
            }
        }
        "run" => v.extend([
            // `id` is optional on this op alone — absent means create
            // from `cwd`+`agent` (the schema can't express the
            // conditional; `serve_missing` enforces it).
            kit::param("id")
                .kind(ParamKind::Ref)
                .ref_query("sessions")
                .doc("existing session id — absent creates one from `cwd`+`agent`")
                .build(),
            agent(false),
            bin(),
            string(
                "cwd",
                "working directory — required with `agent` when `id` is absent",
            ),
            string("title", "session title (create path)"),
            string("model", "model id — attach override or create preference"),
            string_list("fallbacks", "fallback model ids"),
            string(
                "mode",
                "session mode id — applied after ensure, before the prompt",
            ),
            text(
                "text",
                "prompt text — shorthand for one text PromptPart (`parts` wins when both are given)",
            ),
            text(
                "parts",
                "JSON array of ACP PromptPart objects — the verbatim wire form",
            ),
            int(
                "timeoutMs",
                "wait bound in ms — 0/absent waits until the turn settles; on expiry the turn is cancelled",
            ),
        ]),
        "commands" => v.extend([id(), agent(false), bin()]),
        "command" => v.extend([
            id(),
            agent(false),
            bin(),
            kit::param("name")
                .required()
                .doc("advertised slash-command name (a leading '/' is tolerated)")
                .build(),
            string("args", "arguments appended to the `/{name}` command line"),
            text(
                "text",
                "verbatim command line (e.g. '/compact now') — overrides `args`; `name` still gates on the advertisement",
            ),
            int(
                "timeoutMs",
                "wait bound in ms — 0/absent waits until the turn settles; on expiry the turn is cancelled",
            ),
        ]),
        "permission" => v.extend([
            id(),
            agent(false),
            bin(),
            kit::param("request")
                .required()
                .doc("pending permission request id")
                .build(),
            string("option", "selected option id — absent picks the wire default"),
        ]),
        "set_mode" => v.extend([
            id(),
            agent(false),
            bin(),
            kit::param("mode")
                .required()
                .doc("session mode id")
                .build(),
        ]),
        "set_permission_policy" => v.extend([
            id(),
            agent(false),
            bin(),
            kit::param("permissions")
                .kind(ParamKind::Text)
                .required()
                .doc("PermissionPolicy JSON — 'ask'/'allow_all'/'deny_all'/'{\"rules\":[…]}'")
                .build(),
        ]),
        "patch_meta" => v.extend([
            id(),
            bin(),
            text(
                "patch",
                "SessionMeta JSON object — `title`/`pinned`/`archived`/`projectIds`/`model`; flat params of the same names also work",
            ),
        ]),
        "restore" => v.extend([
            id(),
            agent(false),
            bin(),
            text(
                "request",
                "RestoreRequest JSON — `confirm`/`path`/`toolCallId`/`checkpoint`/`paths`; flat aliases also work",
            ),
        ]),
        "rewind" => v.extend([
            id(),
            agent(false),
            bin(),
            text(
                "request",
                "RewindRequest JSON — `confirm`/`nodeId`/`turns`/`checkpoint`; flat aliases also work",
            ),
        ]),
        "import" => v.extend([
            agent(true),
            bin(),
            text(
                "session",
                "session IR JSON object — or pass `path`/`file` to read one from disk",
            ),
            string("path", "path to a session-IR JSON file (alternative to `session`)"),
            string("id", "override: imported session id"),
            string("cwd", "override: working directory"),
            string("title", "override: title"),
            string("model", "override: model"),
        ]),
        "convert" => v.extend([
            id(),
            agent(true),
            bin(),
            kit::param("from")
                .enumeration(STORE_IDS.iter().map(|s| (*s).to_string()).collect())
                .doc("source store the session is read from before the copy")
                .build(),
        ]),
        _ => {}
    }
    v
}

/// `{"op":"query","query":"sessions.<method>"}` — the session
/// plane as typed request/response ops. Params go through the same
/// [`serve_missing`]/[`serve_params`] translation the invoke lane
/// uses, then [`data::mutate`] on the shared `sepia serve` child —
/// one transport, one set of lock/attach semantics (the plane
/// itself enforces them; nothing here bypasses it). The serve
/// result becomes the [`QueryResult`]'s `data` verbatim, so
/// `sessions.list` answers the session array and `sessions.create`
/// the `{id, agentId, capabilities}` object.
///
/// # Errors
/// `ok:false` for an unknown `sessions.*` name, missing/invalid
/// params (keyed detail in the message), or a serve-wire/transport
/// failure — the honest signal a campaign's failure branch keys on.
pub fn run_serve_query(query: &str, params: &Map<String, Value>) -> Response {
    let Some(method) = serve_method(query) else {
        return Response::err(format!("unknown op: {query}"));
    };
    let mut errors = serve_missing(&method, params);
    if errors.is_empty() {
        match serve_params(&method, params) {
            Ok(p) => {
                return match data::mutate(&method, p, params) {
                    Ok(result) => Response::ok_typed(&QueryResult {
                        query: query.to_string(),
                        data: result,
                    }),
                    Err(e) => Response::err(e),
                };
            }
            Err(e) => errors = e,
        }
    }
    let keys = errors.keys().cloned().collect::<Vec<_>>().join(", ");
    Response::err(format!("{query}: missing/invalid params: {keys}"))
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
        "sessions.command" => &["name"],
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
        "sessions.list" | "sessions.create" | "sessions.import" | "sessions.run"
    );
    if needs_id && !present(session_id(params)) {
        errors.insert("id".to_string(), json!("required"));
    }
    // `sessions.run`'s create path — no `id` means `cwd`+`agent`
    // must be there (the schema can't express the conditional).
    if method == "sessions.run" && session_id(params).is_none() {
        for key in ["cwd", "agent"] {
            if !present(params.get(key)) {
                errors.insert(key.to_string(), json!("required when `id` is absent"));
            }
        }
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
    if matches!(
        method,
        "sessions.prompt" | "sessions.prompt_wait" | "sessions.run"
    ) && get(params, &["parts", "text", "prompt", "message"]).is_none_or(|v| !present(Some(v)))
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
    // Wire-native `request` object passes through verbatim — the
    // same convention `patch_meta`'s `patch` already follows.
    if let Some(request) = get(params, &["request"])
        && request.is_object()
    {
        return request.clone();
    }
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
    if let Some(request) = get(params, &["request"])
        && request.is_object()
    {
        return request.clone();
    }
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
        "sessions.run" => {
            put(&mut out, &["cwd"], "cwd", params);
            put(&mut out, &["title"], "title", params);
            put(&mut out, &["model"], "model", params);
            put(&mut out, &["fallbacks"], "fallbacks", params);
            put(&mut out, &["mode"], "mode", params);
            if let Some(parts) = parts_param(params) {
                out.insert("parts".to_string(), parts);
            }
            put(
                &mut out,
                &["timeoutMs", "timeout_ms", "timeout"],
                "timeoutMs",
                params,
            );
        }
        "sessions.command" => {
            put(&mut out, &["name"], "name", params);
            put(&mut out, &["args"], "args", params);
            put(&mut out, &["text"], "text", params);
            put(
                &mut out,
                &["timeoutMs", "timeout_ms", "timeout"],
                "timeoutMs",
                params,
            );
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
fn serve_invoke(
    method: &str,
    view: Option<&str>,
    target: Option<&Value>,
    params: &Map<String, Value>,
) -> ActionResult {
    let mut merged = merge_serve_target(params, target);
    // The invoke wire op carries no view params — fill `id`/`agent`
    // gaps from the *invoking* view's last-resolved context
    // (`session`/`stream` views record theirs; a view-less invoke
    // guesses the session view's, the "current session").
    if session_id(&merged).is_none() || !present(merged.get("agent")) {
        let ctx = match view {
            Some(v) => crate::views::view_context(v),
            None => crate::views::session_context(),
        };
        if let Some(ctx) = ctx {
            for key in ["id", "agent"] {
                if !present(merged.get(key))
                    && let Some(v) = ctx.get(key)
                {
                    merged.insert(key.to_string(), v.clone());
                }
            }
        }
    }
    let mut errors = serve_missing(method, &merged);
    if errors.is_empty() {
        match serve_params(method, &merged) {
            Ok(p) => {
                return match data::mutate(method, p, &merged) {
                    Ok(result) => {
                        // A successful permission answer retires the
                        // ask in the feed buffer — the `permissions`
                        // query and notifications clear on the next
                        // poll (see feed.rs's resolution rule).
                        if method == "sessions.permission"
                            && let (Some(sid), Some(rid)) = (
                                merged.get("id").and_then(Value::as_str),
                                merged.get("request").and_then(Value::as_str),
                            )
                        {
                            crate::feed::resolve_permission(sid, rid);
                        }
                        ActionResult {
                            message: Some(serve_message(method, &result)),
                            data: Some(json!({
                                "method": method,
                                "result": result,
                            })),
                            ..ActionResult::default()
                        }
                    }
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
    match invoke(&candidate, Some(view), None, values) {
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
        // mutates honesty — the cli side-effect set is exactly
        // these (sessions.* mutators are covered by
        // manifest_lists_sessions_ops)
        let mutators: Vec<&str> = ops
            .iter()
            .filter(|o| o["mutates"] == json!(true))
            .filter_map(|o| o["query"].as_str())
            .filter(|q| q.starts_with("cli."))
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
    fn manifest_lists_sessions_ops() {
        let m = manifest();
        let ops = m["ops"].as_array().unwrap();
        let queries: Vec<&str> = ops.iter().filter_map(|o| o["query"].as_str()).collect();
        // Every bounded serve method is a `sessions.*` query op;
        // the subscribe feed stays surface-only.
        for verb in SERVE_ACTIONS {
            let q = format!("sessions.{verb}");
            assert!(queries.contains(&q.as_str()), "{q} missing from ops");
        }
        assert!(!queries.contains(&"sessions.subscribe"));
        assert!(!queries.contains(&"sessions.bogus"));
        // No name collisions with the cli lane or view queries.
        assert!(queries.contains(&"cli.version"));
        assert!(!queries.contains(&"sessions"));

        let find = |q: &str| {
            ops.iter()
                .find(|o| o["query"] == json!(q))
                .unwrap_or_else(|| panic!("{q} missing"))
        };
        // mutates honesty — reads vs plane writes.
        for q in [
            "sessions.list",
            "sessions.get",
            "sessions.summary",
            "sessions.export",
            "sessions.history",
            "sessions.commands",
        ] {
            assert_eq!(find(q)["mutates"], json!(false), "{q}");
        }
        for q in [
            "sessions.create",
            "sessions.command",
            "sessions.prompt",
            "sessions.prompt_wait",
            "sessions.delete",
            "sessions.patch_meta",
            "sessions.restore",
            "sessions.rewind",
            "sessions.import",
            "sessions.convert",
            "sessions.attach",
            "sessions.detach",
            "sessions.cancel",
            "sessions.permission",
            "sessions.set_mode",
            "sessions.set_permission_policy",
        ] {
            assert_eq!(find(q)["mutates"], json!(true), "{q}");
        }
        // Typed schemas — id ref-picks over the sessions view query.
        let del = find("sessions.delete");
        let idp = del["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "id")
            .unwrap();
        assert_eq!(idp["kind"], "opt");
        assert_eq!(idp["schema"]["kind"], "ref");
        assert_eq!(idp["schema"]["ref_query"], "sessions");
        assert_eq!(idp["schema"]["required"], json!(true));
        // agent enumerates the store ids; sepiaBin steers the child.
        let agent = del["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "agent")
            .unwrap();
        assert_eq!(agent["schema"]["kind"], "enum");
        assert!(
            del["params"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "sepiaBin")
        );
        // prompt advertises both the wire `parts` and the `text`
        // shorthand; prompt_wait adds timeoutMs.
        let prompt = find("sessions.prompt");
        let names: Vec<&str> = prompt["params"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert!(
            names.contains(&"parts") && names.contains(&"text"),
            "{names:?}"
        );
        assert!(!names.contains(&"timeoutMs"));
        let wait = find("sessions.prompt_wait");
        assert!(
            wait["params"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "timeoutMs")
        );
    }

    #[test]
    fn serve_params_passes_request_objects_through() {
        // Wire-native `request` objects ride verbatim — restore and
        // rewind accept the serve wire's own shape.
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        p.insert(
            "request".into(),
            json!({"confirm": false, "turns": 3, "nodeId": "n9"}),
        );
        let v = serve_params("sessions.rewind", &p).unwrap();
        assert_eq!(
            v["request"],
            json!({"confirm": false, "turns": 3, "nodeId": "n9"})
        );
        let v = serve_params("sessions.restore", &p).unwrap();
        assert_eq!(v["request"]["confirm"], json!(false));
        assert_eq!(v["request"]["turns"], 3);
        // Flat aliases still assemble the request (invoke lane).
        let mut p = Map::new();
        p.insert("turns".into(), json!(2));
        p.insert("tool_call_id".into(), json!("tc-1"));
        let v = serve_params("sessions.rewind", &p).unwrap();
        assert_eq!(v["request"]["turns"], 2);
        assert_eq!(v["request"]["confirm"], json!(true));
        let v = serve_params("sessions.restore", &p).unwrap();
        assert_eq!(v["request"]["toolCallId"], "tc-1");
    }

    #[test]
    fn serve_params_maps_command_fields() {
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        p.insert("name".into(), json!("compact"));
        p.insert("args".into(), json!("now"));
        p.insert("timeout_ms".into(), json!(5000));
        let v = serve_params("sessions.command", &p).unwrap();
        assert_eq!(v["id"], "s1");
        assert_eq!(v["name"], "compact");
        assert_eq!(v["args"], "now");
        assert_eq!(v["timeoutMs"], 5000);
        assert!(v.get("text").is_none());
        // `text` passes through verbatim; commands needs nothing but id.
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        p.insert("name".into(), json!("review"));
        p.insert("text".into(), json!("/review the diff"));
        let v = serve_params("sessions.command", &p).unwrap();
        assert_eq!(v["text"], "/review the diff");
        let v = serve_params("sessions.commands", &p).unwrap();
        assert_eq!(v["id"], "s1");
        assert!(v.get("name").is_none());
        // `name` is required on the invoke/query lanes.
        let mut p = Map::new();
        p.insert("id".into(), json!("s1"));
        assert!(serve_missing("sessions.command", &p).contains_key("name"));
        p.insert("name".into(), json!("compact"));
        assert!(serve_missing("sessions.command", &p).is_empty());
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
