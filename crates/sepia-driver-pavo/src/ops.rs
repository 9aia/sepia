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

use clap::{Arg, ArgAction, Command};
use pavo_protocol::wire::{ActionResult, QueryResult, Response};
use serde_json::{Map, Value, json};

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
}
