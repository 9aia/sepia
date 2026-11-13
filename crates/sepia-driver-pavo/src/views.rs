//! views — the driver's manifest: every ViewSpec plus the query
//! resolvers that feed them.
//!
//! Read path: most queries are one or two calls against `sepia
//! serve` through [`crate::data::call`] — the session plane's own
//! wire, so the driver sees exactly what orchestrators see and can
//! never mutate anything (only read-side methods are issued). The
//! `stores`/`drivers` queries are the exception: they ride the
//! `cli.*` lane ([`crate::ops::verb_output`] — a `run` child per
//! verb) because store listings and driver discovery are engine
//! verbs, not serve methods. A failed call degrades to an
//! `{"error": …}` payload inside an `ok:true` response, matching
//! the sibling drivers' contract.
//!
//! Surface mapping (what `sepia serve` actually exposes — see
//! `crates/sepia-cli/src/serve.rs` for the method table):
//!
//! | pavo view      | data source                              |
//! |----------------|------------------------------------------|
//! | `sepia:status` | `manifest` + `sessions/agents.list` counts |
//! | `sepia:sessions` | `sessions.list` (`withLocks` off)      |
//! | `sepia:session`  | `sessions.summary`/`list` + `sessions.history`; meta form → `sessions.patch_meta` |
//! | `sepia:agents`   | `agents.list` + `config.list`          |
//! | `sepia:config`   | `config.list` + `config.get` (serve)   |
//! | `sepia:stores`   | `store list` per store (`cli` lane, TSV) |
//! | `sepia:drivers`  | `driver list --json` (`cli` lane)      |
//!
//! Every view also declares `refresh_ms` — the host's auto-refresh
//! hint (TUI timer, web polling; CLI ignores it). The cadence
//! constants by [`spec`] record the per-view rationale: feed-buffer
//! views tick fastest, `cli.*`-lane and discovery views slowly.
//!
//! Mutations live in two places, both dispatching through the wire
//! `invoke` op: the declared [`Action`]s below (`session.*` ids →
//! the `sepia serve` session plane, everything else → `cli.<verb>`
//! engine ops — see `ops.rs` for the routing), and the ops-derived
//! actions pavo-core builds from the `ops` manifest's param
//! schemas. This module stays read-only: every *query* issues
//! read-side calls only — serve methods or read-only engine verbs.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex};

use pavo_driver_kit as kit;
use pavo_protocol::{
    Action, DriverManifest, FormFieldKind, JobItem, MenuEntry, NotificationItem, ParamKind,
    ParamSchema, ViewNode, ViewSpec,
};
use serde_json::{Map, Value, json};

use crate::{data, feed, ops};

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

/// A table whose node carries actions — row context travels as the
/// invoke `target` where the host supplies it (object targets merge
/// per `ops::merge_serve_target`/`merge_target`).
fn table_actions(
    title: &str,
    query: &str,
    columns: &[(&str, &str)],
    actions: Vec<Action>,
) -> ViewNode {
    kit::sortable_table(
        kit::base()
            .title(title)
            .query(query)
            .actions(actions)
            .build(),
        columns
            .iter()
            .map(|(key, t)| kit::table_column(key, t))
            .collect(),
        Vec::new(),
    )
}

/// An expandable tree bound to a query — the bound result supplies
/// `items` (see `config.detail`).
fn tree(title: &str, query: &str) -> ViewNode {
    kit::expandable_tree(kit::base().title(title).query(query).build(), Vec::new())
}

/// A `kv` node carrying actions — the `session` view's detail pane.
fn kv_actions(title: &str, query: &str, actions: Vec<Action>) -> ViewNode {
    kit::kv(
        kit::base()
            .title(title)
            .query(query)
            .actions(actions)
            .build(),
        Vec::new(),
    )
}

// ------------------------------------------------------- session meta form
//
// The `session` view's edit form. `submit_op` names the
// session-plane action; `ops::submit` resolves it through
// [`form_submit_op`] and merges the view's `id`/`agent` (the wire
// `submit` op carries no view params — see [`session_context`]).
// Field `value`s are seeded per request by [`patch_form_values`]
// from the `session` query data — the spec itself stays static.

/// The session view's meta-edit form — fields mirror the
/// `session.patch_meta` action's params (title/pinned/archived/
/// model; `projectIds` stays action-only — a comma string can't
/// express the wire's array).
fn meta_form() -> ViewNode {
    let mut model = kit::field("model", "Model");
    model.hint = Some("Preferred spawn model — empty keeps the current one.".to_string());
    kit::form_submit(
        kit::base().id("meta").title("Edit meta").build(),
        vec![
            kit::field("title", "Title"),
            kit::field_kind("pinned", "Pinned", FormFieldKind::Bool),
            kit::field_kind("archived", "Archived", FormFieldKind::Bool),
            model,
        ],
        "session.patch_meta",
    )
}

/// Per-view resolved `id`/`agent` — recorded by [`view_result`] for
/// every view whose params carry `id` (`session`, `stream`, …).
/// `ops::invoke` merges the *invoking* view's context into
/// session-scoped serve actions because the wire `invoke` op's
/// params don't automatically carry view params (same gap the
/// `submit` op has — see [`session_context`]). Process-global like
/// the serve client: one driver process, last-viewed wins per view.
static VIEW_CTX: LazyLock<Mutex<HashMap<String, Value>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The `id`/`agent` view `view` last resolved, if it has run.
pub(crate) fn view_context(view: &str) -> Option<Map<String, Value>> {
    VIEW_CTX
        .lock()
        .ok()
        .and_then(|g| g.get(view).and_then(Value::as_object).cloned())
}

/// Record a view's resolved `id`/`agent` — called by
/// [`view_result`] on every resolution whose params carry `id`.
fn note_view_context(view: &str, params: &Map<String, Value>) {
    let mut ctx = Map::new();
    if let Some(id) = params.get("id").and_then(Value::as_str) {
        ctx.insert("id".to_string(), json!(id));
    }
    if let Some(agent) = params.get("agent").and_then(Value::as_str) {
        ctx.insert("agent".to_string(), json!(agent));
    }
    if ctx.is_empty() {
        return;
    }
    if let Ok(mut g) = VIEW_CTX.lock() {
        g.insert(view.to_string(), Value::Object(ctx));
    }
}

/// The session view's resolved `id`/`agent`, if it has run —
/// `view_context("session")` kept under its old name for the
/// `submit` path.
pub(crate) fn session_context() -> Option<Map<String, Value>> {
    view_context("session")
}

/// The `submit_op` of the form node `form` names on view `view`
/// (`None` → the view's first form, per the wire contract's
/// "default/first form" rule). `ops::submit` consults this before
/// the verb-named-form fallback.
pub(crate) fn form_submit_op(view: &str, form: Option<&str>) -> Option<String> {
    let spec = specs().into_iter().find(|v| v.id == view)?;
    let mut found = None;
    collect_form_op(&spec.root, form, &mut found);
    found
}

/// Depth-first search for the named (or first) form's `submit_op`.
fn collect_form_op(node: &ViewNode, form: Option<&str>, out: &mut Option<String>) {
    if out.is_some() {
        return;
    }
    match node {
        ViewNode::Form(f) => {
            let matches = match form {
                Some(id) => f.base.id.as_deref() == Some(id),
                None => true,
            };
            if matches {
                out.clone_from(&f.submit_op);
            }
        }
        ViewNode::Panel(n) => {
            for c in &n.children {
                collect_form_op(c, form, out);
            }
        }
        ViewNode::Row(n) => {
            for c in &n.children {
                collect_form_op(c, form, out);
            }
        }
        ViewNode::Column(n) => {
            for c in &n.children {
                collect_form_op(c, form, out);
            }
        }
        _ => {}
    }
}

/// Seed a form's field `value`s from the bound session detail — the
/// spec is static, so [`view_result`] rewrites the returned copy per
/// request; fields the data lacks keep their declared default.
fn patch_form_values(node: &mut ViewNode, session: &Value) {
    match node {
        ViewNode::Form(f) => {
            for field in &mut f.fields {
                if let Some(v) = session.get(field.key.as_str()) {
                    field.value = Some(v.clone());
                }
            }
        }
        ViewNode::Panel(n) => {
            for c in &mut n.children {
                patch_form_values(c, session);
            }
        }
        ViewNode::Row(n) => {
            for c in &mut n.children {
                patch_form_values(c, session);
            }
        }
        ViewNode::Column(n) => {
            for c in &mut n.children {
                patch_form_values(c, session);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------- refresh
//
// `refresh_ms` arms the host's auto-refresh (`ViewSpec::refresh_ms`
// — TUI timer, web polling; the CLI ignores it; hosts drop values
// under 1s or over 5min). Cadence tracks how fast each view's data
// actually moves and what a tick costs:
//
// - `permissions` is buffer-only (the feed already holds the asks)
//   so it ticks fastest; `live`/`stream` merge serve calls
//   (`sessions.list` titles, `sessions.history` tail) and sit at
//   the session cadence with `sessions`/`session`;
// - `status` is three cheap serve calls but its counts move
//   slowly — 5s;
// - `stores` fans `store list` out over the `cli.*` lane — one
//   spawned `run` child per store — so it polls slowly;
// - `drivers`/`agents`/`config` are near-static discovery/config
//   views; `config.detail` alone fans out to `config.get`, so the
//   whole view stays on the slow cadence.

/// Buffer-only feed views — the asks live in `feed.rs`, no serve call per tick.
const REFRESH_FEED_MS: u64 = 2_000;
/// Session-plane views — `sessions.list`/`sessions.summary`/`sessions.history` per tick.
const REFRESH_SESSION_MS: u64 = 3_000;
/// Plane digest — counts move slowly.
const REFRESH_STATUS_MS: u64 = 5_000;
/// `cli.*` lane views — a spawned `run` child per query.
const REFRESH_CLI_MS: u64 = 10_000;
/// Discovery/config views — near-static, heaviest queries.
const REFRESH_SLOW_MS: u64 = 30_000;

fn spec(
    id: &str,
    title: &str,
    route: &str,
    description: &str,
    queries: &[&str],
    refresh_ms: Option<u64>,
    root: ViewNode,
) -> ViewSpec {
    let mut view = kit::view(id, title, route, root).description(description);
    if let Some(ms) = refresh_ms {
        view = view.refresh_ms(ms);
    }
    for q in queries {
        view = view.query(kit::binding(q, "query"));
    }
    view.build()
}

// ---------------------------------------------------------------- actions
//
// Interaction actions — the pavo `invoke` layer. `session.*` ids
// dispatch to `sepia serve` methods (the session plane — see
// `ops::serve_method`); engine verbs (`store.*`, `config.*`, …)
// ride the `cli.<verb>` argv path and are already reachable as
// ops-derived actions, so nothing here duplicates them.
//
// Danger mapping (myco-driver's convention): ordinary mutations
// get `confirm`; irreversible-ish ones (`delete`, `restore`,
// `rewind`) get `type-target`; reads and trivially-recoverable ops
// (`export`, `cancel`, `permission` answers, `create`) get `none`.

/// The shared `id` param — a ref picker over the `sessions` view's
/// query binding.
fn id_param() -> ParamSchema {
    kit::param("id")
        .reference("sessions")
        .required()
        .label("Session")
        .doc("Session id — the picker lists `sessions.list` rows.")
        .build()
}

/// `agent` — optional store/agent disambiguator, ref picker over
/// the `agents` view's query binding.
fn agent_param() -> ParamSchema {
    kit::param("agent")
        .reference("agents")
        .label("Agent")
        .doc("Agent/store id — pass when session ids collide across stores.")
        .build()
}

/// `permissions` — the auto-answer policy enum; the wire also
/// accepts a `{"rules":[…]}` object which the enum can't express,
/// so the doc says so.
fn permissions_param() -> ParamSchema {
    kit::param("permissions")
        .enumeration(vec![
            "ask".to_string(),
            "allow_all".to_string(),
            "deny_all".to_string(),
        ])
        .label("Permissions")
        .doc("Auto-answer policy for `session/request_permission` — a `{\"rules\":[…]}` object passes through too.")
        .build()
}

/// A plain labelled string param.
fn str_param(name: &str, label: &str, doc: &str) -> ParamSchema {
    kit::param(name).label(label).doc(doc).build()
}

/// A labelled multi-line text param.
fn text_param(name: &str, label: &str, doc: &str) -> ParamSchema {
    kit::param(name).text().label(label).doc(doc).build()
}

/// A labelled bool param.
fn bool_param(name: &str, label: &str, doc: &str) -> ParamSchema {
    kit::param(name)
        .kind(ParamKind::Bool)
        .label(label)
        .doc(doc)
        .build()
}

/// Mark a built [`ParamSchema`] required — the helpers return
/// finished schemas, so required-ness is a field flip.
fn req(mut s: ParamSchema) -> ParamSchema {
    s.required = true;
    s
}

/// The session-scoped action set — `target("session")` marks the
/// needed context; every action still declares `id` because node
/// actions carry no automatic row target (only tree items do).
fn session_actions() -> Vec<Action> {
    vec![
        kit::action("session.attach", "Attach")
            .icon("plug")
            .confirm()
            .key("a")
            .target("session")
            .param(id_param())
            .param(agent_param())
            .param(bool_param("takeover", "Takeover", "Detach the current owner and attach anyway."))
            .param(str_param("model", "Model", "Preferred spawn model for this attach."))
            .param(permissions_param())
            .description("Attach the session plane to a live agent session (read-write unless the store is read-only).")
            .build(),
        kit::action("session.detach", "Detach")
            .confirm()
            .key("d")
            .target("session")
            .param(id_param())
            .description("Detach the live attach — the agent keeps running, events stop streaming to this plane.")
            .build(),
        kit::action("session.prompt", "Prompt")
            .icon("send")
            .confirm()
            .key("p")
            .target("session")
            .param(id_param())
            .param(agent_param())
            .param(req(text_param(
                "text",
                "Prompt",
                "Message text — sent as one `text` PromptPart.",
            )))
            .description("Send a prompt to the session's agent — starts or continues a turn.")
            .build(),
        kit::action("session.cancel", "Cancel turn")
            .key("x")
            .target("session")
            .param(id_param())
            .description("Cancel the in-flight turn, if any.")
            .build(),
        kit::action("session.permission", "Answer permission")
            .target("session")
            .param(id_param())
            .param(req(str_param(
                "request",
                "Request",
                "Pending `session/request_permission` request id.",
            )))
            .param(str_param(
                "option",
                "Option",
                "Option id to select — absent picks the agent's preferred option.",
            ))
            .description("Answer a pending permission request on the session.")
            .build(),
        kit::action("session.set_mode", "Set mode")
            .confirm()
            .target("session")
            .param(id_param())
            .param(req(str_param(
                "mode",
                "Mode",
                "Agent session mode id (e.g. `ask`, `code` — agent-defined).",
            )))
            .description("Switch the session's agent mode.")
            .build(),
        kit::action("session.set_permission_policy", "Set permission policy")
            .confirm()
            .target("session")
            .param(id_param())
            .param(req(permissions_param()))
            .description("Set the session's auto-answer policy for permission requests.")
            .build(),
        kit::action("session.patch_meta", "Edit session meta")
            .confirm()
            .target("session")
            .param(id_param())
            .param(str_param("title", "Title", "New display title."))
            .param(bool_param("pinned", "Pinned", "Pin/unpin the session."))
            .param(bool_param("archived", "Archived", "Archive/unarchive the session."))
            .param(str_param("model", "Model", "Preferred spawn model — empty string clears."))
            .description("Patch the session overlay — title, pin, archive, project tags, model.")
            .build(),
        kit::action("session.export", "Export session")
            .target("session")
            .param(id_param())
            .description("Export the session IR — the transfer bundle `sessions.import` consumes.")
            .build(),
        kit::action("session.restore", "Restore files")
            .type_target()
            .target("session")
            .param(id_param())
            .param(str_param("path", "Path", "File to restore (with recorded diffs)."))
            .param(str_param("checkpoint", "Checkpoint", "Snapshot ref to materialize instead."))
            .param(kit::param("paths").kind(ParamKind::StringList).label("Paths").doc("Multiple paths to restore.").build())
            .description("Revert files through recorded diffs or materialize a checkpoint — the plane requires `confirm`, sent automatically.")
            .build(),
        kit::action("session.rewind", "Rewind")
            .type_target()
            .target("session")
            .param(id_param())
            .param(kit::param("turns").kind(ParamKind::Integer).label("Turns").doc("Drop this many trailing turns.").build())
            .param(kit::param("nodeId").kind(ParamKind::Integer).label("Node").doc("Cut at this node id instead.").build())
            .param(str_param("checkpoint", "Checkpoint", "Or cut at this checkpoint ref."))
            .description("Rewind the conversation — drops nodes after the selector; `confirm` is sent automatically.")
            .build(),
        kit::action("session.delete", "Delete session")
            .type_target()
            .key("D")
            .target("session")
            .param(id_param())
            .description("Delete the session — store-level delete, irreversible.")
            .build(),
    ]
}

/// Manifest-global actions — the session-lifecycle verbs offered on
/// every view (ids are `session.<verb>`; the serve lane resolves
/// them). `session.import`/`session.convert`/`session.create` take
/// no `id`, so they live here rather than in the scoped set.
fn global_actions() -> Vec<Action> {
    let mut out = session_actions();
    out.extend([
        kit::action("session.create", "New session")
            .icon("plus")
            .key("n")
            .param(req(str_param(
                "cwd",
                "Working dir",
                "Directory the session runs in.",
            )))
            .param(agent_param())
            .param(str_param("title", "Title", "Display title."))
            .param(str_param("model", "Model", "Preferred spawn model."))
            .param(permissions_param())
            .description("Create a session on an agent's store.")
            .build(),
        kit::action("session.import", "Import session")
            .confirm()
            .param(req(agent_param()))
            .param(req(str_param(
                "path",
                "Session JSON",
                "Path to a session-IR JSON file (e.g. from `session.export`).",
            )))
            .param(str_param(
                "id",
                "New id",
                "Override the imported session id.",
            ))
            .param(str_param(
                "cwd",
                "Working dir",
                "Override the working directory.",
            ))
            .param(str_param("title", "Title", "Override the title."))
            .description("Write a session IR into an agent's store.")
            .build(),
        kit::action("session.convert", "Convert session")
            .confirm()
            .target("session")
            .param(id_param())
            .param(
                kit::param("agent")
                    .enumeration(vec![
                        "devin".to_string(),
                        "cline".to_string(),
                        "claude".to_string(),
                        "cursor".to_string(),
                    ])
                    .required()
                    .label("Target agent")
                    .doc("Store/agent to convert into.")
                    .build(),
            )
            .param(
                kit::param("from")
                    .enumeration(vec![
                        "devin".to_string(),
                        "cline".to_string(),
                        "claude".to_string(),
                        "cursor".to_string(),
                    ])
                    .label("Source store")
                    .doc("Source store when it differs from the merged view's.")
                    .build(),
            )
            .description("Copy the session into another agent's store, node-local.")
            .build(),
    ]);
    out
}

/// Actions scoped to the `sessions` table node — the frequent
/// session ops. `target("row")` hints hosts may pass the selected
/// row; `id` stays declared because only tree items get automatic
/// targets.
fn table_row_actions() -> Vec<Action> {
    vec![
        kit::action("session.prompt", "Prompt")
            .confirm()
            .key("p")
            .target("row")
            .param(id_param())
            .param(req(text_param("text", "Prompt", "Message text.")))
            .build(),
        kit::action("session.attach", "Attach")
            .confirm()
            .target("row")
            .param(id_param())
            .param(bool_param(
                "takeover",
                "Takeover",
                "Detach the current owner.",
            ))
            .build(),
        kit::action("session.detach", "Detach")
            .confirm()
            .target("row")
            .param(id_param())
            .build(),
        kit::action("session.cancel", "Cancel turn")
            .target("row")
            .param(id_param())
            .build(),
        kit::action("session.delete", "Delete")
            .type_target()
            .target("row")
            .param(id_param())
            .build(),
    ]
}

/// The four store/agent ids — enum options for `from`/`to`/`agent`
/// params (same set as `ops::STORE_IDS`).
fn store_enum() -> Vec<String> {
    ops::STORE_IDS.iter().map(|s| (*s).to_string()).collect()
}

/// `from` — the source store/agent selector (`--from` on the store
/// and config verbs). Rows also carry a `from` key so an object
/// target fills it without the form asking.
fn from_param(required: bool) -> ParamSchema {
    let mut p = kit::param("from")
        .enumeration(store_enum())
        .label("Store")
        .doc("Source store/agent — the row's `from` fills this when a target is supplied.");
    if required {
        p = p.required();
    }
    p.build()
}

/// `to` — the target store/agent selector (`--to` on install verbs).
fn to_param(required: bool) -> ParamSchema {
    let mut p = kit::param("to")
        .enumeration(store_enum())
        .label("Target store")
        .doc("Store/agent to write into.");
    if required {
        p = p.required();
    }
    p.build()
}

/// `session_id` — the store verbs' positional; a ref picker over the
/// `stores` view's rows.
fn session_id_param() -> ParamSchema {
    kit::param("session_id")
        .reference("stores")
        .required()
        .label("Session")
        .doc("Session id on the store — the row's `session_id` fills this when a target is supplied.")
        .build()
}

/// Actions scoped to the `stores` table — the store verbs bound to
/// a session row. Rows carry `session_id`/`from` so an object target
/// merges them into the verb's params; the declared params keep the
/// auto-form usable when no row target arrives (the pinned pavo rev's
/// TUI sends none for table rows — only tree items get targets).
fn store_row_actions() -> Vec<Action> {
    vec![
        kit::action("store.export", "Export")
            .target("row")
            .param(session_id_param())
            .param(from_param(true))
            .param(str_param(
                "out",
                "Out",
                "Output file (or directory); stdout when omitted.",
            ))
            .param(
                kit::param("format")
                    .enumeration(vec!["json".to_string(), "cline".to_string()])
                    .label("Format")
                    .doc("`json` = session IR; `cline` = Cline session files (needs `out` dir).")
                    .build(),
            )
            .description("Export the session — IR JSON by default, Cline files with format=cline.")
            .build(),
        kit::action("store.install", "Install into…")
            .confirm()
            .target("row")
            .param(session_id_param())
            .param(from_param(true))
            .param(to_param(false))
            .param(str_param("id", "New id", "Install under this session id."))
            .param(bool_param(
                "force",
                "Force",
                "Replace a session that still belongs to a live owner.",
            ))
            .description("Copy the session into another agent's store, ready to resume.")
            .build(),
        kit::action("store.delete", "Delete")
            .type_target()
            .target("row")
            .param(session_id_param())
            .param(from_param(true))
            .description("Delete the session from its store — irreversible.")
            .build(),
    ]
}

/// The "open the stream" row action — a route action (P-033): the
/// host interpolates `{target.id}` over the invoke context and
/// navigates locally, no wire round-trip. Rows on `live`/
/// `permissions` carry `id` = session id.
fn open_stream_action() -> Action {
    kit::action("live.open", "Open stream")
        .route("/sepia/stream?id={target.id}")
        .description("Open the session's live stream view.")
        .build()
}

/// Actions scoped to the `live` table — open the stream plus the
/// frequent session ops bound to the row (`id` merges from an
/// object target).
fn live_row_actions() -> Vec<Action> {
    vec![
        open_stream_action(),
        kit::action("session.prompt", "Prompt")
            .confirm()
            .key("p")
            .target("row")
            .param(id_param())
            .param(req(text_param("text", "Prompt", "Message text.")))
            .build(),
        kit::action("session.cancel", "Cancel turn")
            .target("row")
            .param(id_param())
            .build(),
        kit::action("session.attach", "Attach")
            .confirm()
            .target("row")
            .param(id_param())
            .build(),
        kit::action("session.detach", "Detach")
            .confirm()
            .target("row")
            .param(id_param())
            .build(),
    ]
}

/// Actions scoped to the `permissions` table — `session.permission`
/// with row context: the row's `id`/`request` merge into the serve
/// params; `option` stays a form param (option ids are
/// agent-defined — the row's `options` column lists them).
fn perm_row_actions() -> Vec<Action> {
    vec![
        kit::action("session.permission", "Answer")
            .target("row")
            .param(id_param())
            .param(req(str_param(
                "request",
                "Request",
                "Pending request id — the row's `request` fills this.",
            )))
            .param(str_param(
                "option",
                "Option",
                "Option id to select — see the row's `options` column; absent picks the agent's preferred option.",
            ))
            .description("Answer the pending permission request on the session.")
            .build(),
        open_stream_action(),
    ]
}

/// The `stream` view's kv actions — the session-plane subset that
/// makes sense mid-transcript (prompt/cancel/policy/mode/lifecycle).
fn stream_actions() -> Vec<Action> {
    session_actions()
        .into_iter()
        .filter(|a| {
            matches!(
                a.id.as_str(),
                "session.attach"
                    | "session.detach"
                    | "session.prompt"
                    | "session.cancel"
                    | "session.set_mode"
                    | "session.set_permission_policy"
            )
        })
        .collect()
}

/// Actions scoped to the `config` agents table — the `config.*`
/// engine verbs (cli lane) bound to an agent row. Rows carry `from`
/// so an object target fills `--from`; `to`/`out`/`path` stay
/// form params.
fn config_row_actions() -> Vec<Action> {
    vec![
        kit::action("config.export", "Export config")
            .target("row")
            .param(from_param(true))
            .param(str_param(
                "out",
                "Out",
                "Output file for the config JSON; stdout when omitted.",
            ))
            .description("Export the agent's config IR JSON.")
            .build(),
        kit::action("config.diff", "Diff config")
            .target("row")
            .param(from_param(true))
            .param(to_param(true))
            .description("Compare this agent's config against another's.")
            .build(),
        kit::action("config.install", "Install config…")
            .confirm()
            .target("row")
            .param(from_param(true))
            .param(to_param(true))
            .description("Copy this agent's config into another agent's store.")
            .build(),
    ]
}

/// `contributes.menus[]` — the command palette / quick-launch
/// list: session actions plus routes to the views.
fn menu_entries() -> Vec<MenuEntry> {
    let mut out = Vec::new();
    let mut add = |id: &str,
                   title: &str,
                   action: Option<&str>,
                   route: Option<&str>,
                   section: &str,
                   priority: i32,
                   keywords: &[&str]| {
        let mut e = kit::menu(id, title).section(section).priority(priority);
        if let Some(a) = action {
            e = e.action(a);
        }
        if let Some(r) = route {
            e = e.route(r);
        }
        out.push(
            e.keywords(keywords.iter().map(|s| (*s).to_string()).collect())
                .build(),
        );
    };
    add(
        "new-session",
        "New session",
        Some("session.create"),
        None,
        "Sessions",
        10,
        &["create", "spawn", "session"],
    );
    add(
        "import-session",
        "Import session",
        Some("session.import"),
        None,
        "Sessions",
        20,
        &["import", "restore", "load"],
    );
    add(
        "prompt-session",
        "Prompt session",
        Some("session.prompt"),
        None,
        "Sessions",
        30,
        &["prompt", "send", "message"],
    );
    add(
        "delete-session",
        "Delete session",
        Some("session.delete"),
        None,
        "Sessions",
        40,
        &["delete", "remove", "destroy"],
    );
    add(
        "sessions-view",
        "Sessions",
        None,
        Some("/sepia/sessions"),
        "Views",
        10,
        &["list", "sessions"],
    );
    add(
        "agents-view",
        "Agents",
        None,
        Some("/sepia/agents"),
        "Views",
        20,
        &["agents", "config"],
    );
    add(
        "config-view",
        "Agent config",
        None,
        Some("/sepia/config"),
        "Views",
        25,
        &["config", "skills", "rules", "mcp"],
    );
    add(
        "stores-view",
        "Stores",
        None,
        Some("/sepia/stores"),
        "Views",
        26,
        &["stores", "sessions", "list"],
    );
    add(
        "drivers-view",
        "Drivers",
        None,
        Some("/sepia/drivers"),
        "Views",
        27,
        &["drivers", "discovery"],
    );
    add(
        "status-view",
        "Status",
        None,
        Some("/sepia/status"),
        "Views",
        30,
        &["digest", "summary"],
    );
    add(
        "live-view",
        "Live sessions",
        None,
        Some("/sepia/live"),
        "Views",
        28,
        &["live", "events", "feed", "busy"],
    );
    add(
        "permissions-view",
        "Permissions",
        None,
        Some("/sepia/permissions"),
        "Views",
        29,
        &["permissions", "asks", "approve"],
    );
    out
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

const CONFIG_AGENT_COLS: &[(&str, &str)] = &[
    ("id", "Agent"),
    ("label", "Label"),
    ("runtime", "Runtime"),
    ("skills", "Skills"),
    ("rules", "Rules"),
    ("commands", "Commands"),
    ("hooks", "Hooks"),
    ("subagents", "Subagents"),
    ("mcpServers", "MCP"),
    ("error", "Error"),
];

const STORE_COLS: &[(&str, &str)] = &[
    ("store", "Store"),
    ("id", "ID"),
    ("title", "Title"),
    ("cwd", "Cwd"),
    ("error", "Error"),
];

const DRIVER_COLS: &[(&str, &str)] = &[
    ("id", "ID"),
    ("label", "Label"),
    ("version", "Version"),
    ("capabilities", "Capabilities"),
    ("agentCommand", "Agent command"),
    ("agentCommandResolves", "Resolves"),
    ("binary", "Binary"),
    ("error", "Error"),
];

const LIVE_COLS: &[(&str, &str)] = &[
    ("id", "ID"),
    ("title", "Title"),
    ("agent", "Agent"),
    ("status", "Status"),
    ("cwd", "Cwd"),
    ("lastEvent", "Last event"),
    ("pending", "Pending"),
    ("lagged", "Lagged"),
];

const PERM_COLS: &[(&str, &str)] = &[
    ("id", "Session"),
    ("agent", "Agent"),
    ("request", "Request"),
    ("title", "Title"),
    ("tool", "Tool"),
    ("options", "Options"),
    ("age", "Age (s)"),
    ("since", "Since"),
];

/// Every view the driver declares — static (no sepia state needed;
/// `manifest`/`views` must answer anywhere).
pub fn specs() -> Vec<ViewSpec> {
    vec![
        spec(
            "status",
            "Status",
            "/sepia/status",
            "Plane digest — node identity, protocol version, and live counts of sessions and agents.",
            &["summary"],
            Some(REFRESH_STATUS_MS),
            col("Sepia status", vec![kv("Plane", "summary")]),
        ),
        spec(
            "sessions",
            "Sessions",
            "/sepia/sessions",
            "Every session the plane knows — store rows plus live attaches — with busy/overlay flags. Lock-holder probing stays off (`sessions.list` without `withLocks`) so the view answers inside a host's request budget; the probe path can hit a store driver's call timeout. Filter with --param agent=.",
            &["sessions"],
            Some(REFRESH_SESSION_MS),
            col(
                "Sepia sessions",
                vec![table_actions(
                    "Sessions",
                    "sessions",
                    SESSION_COLS,
                    table_row_actions(),
                )],
            ),
        ),
        spec(
            "session",
            "Session",
            "/sepia/session",
            "One session — summary fields, a meta-edit form (`sessions.patch_meta` on submit), and the paged message history as an events timeline. Requires --param id=<session-id> (--param agent= when ids collide across stores).",
            &["session", "history"],
            Some(REFRESH_SESSION_MS),
            col(
                "Session",
                vec![
                    kv_actions("Session", "session", session_actions()),
                    meta_form(),
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
            Some(REFRESH_SLOW_MS),
            table("Agents", "agents", AGENT_COLS),
        ),
        spec(
            "config",
            "Agent config",
            "/sepia/config",
            "Per-agent config inventory (`config.list` counts) with the selected agent's config IR as a tree (`config.get`) — skills, rules, commands, hooks, subagents, MCP servers. Pass --param agent=<id> to pick the detail (--param dir= overrides the config dir); without it the first config-capable agent shows.",
            &["config.agents", "config.detail"],
            Some(REFRESH_SLOW_MS),
            col(
                "Agent config",
                vec![
                    table_actions(
                        "Agents",
                        "config.agents",
                        CONFIG_AGENT_COLS,
                        config_row_actions(),
                    ),
                    tree("Config detail", "config.detail"),
                ],
            ),
        ),
        spec(
            "stores",
            "Stores",
            "/sepia/stores",
            "Every agent session store (`store list` per store — the cli lane, not the serve plane) merged into one table. A failing store degrades to an error row. --param db=/data-dir=/claude-dir=/cursor-dir= forward as store flags.",
            &["stores"],
            Some(REFRESH_CLI_MS),
            col(
                "Store sessions",
                vec![table_actions(
                    "Sessions",
                    "stores",
                    STORE_COLS,
                    store_row_actions(),
                )],
            ),
        ),
        spec(
            "drivers",
            "Drivers",
            "/sepia/drivers",
            "Discovered `sepia-driver-*` binaries (`driver list --json` — the cli lane): manifest id/version/capabilities, agent-command resolution, and probe problems as error rows.",
            &["drivers"],
            Some(REFRESH_SLOW_MS),
            col("Drivers", vec![table("Drivers", "drivers", DRIVER_COLS)]),
        ),
        spec(
            "live",
            "Live sessions",
            "/sepia/live",
            "Sessions the event feed sees — attach/create/detach lifecycle plus run state (busy/attached/detached), last activity, and pending permission counts, merged with `sessions.list` for titles. Data comes from the feed child's `events.subscribe` stream; feed health is the `feed.status` kv. Row action opens the stream.",
            &["feed.status", "live"],
            Some(REFRESH_SESSION_MS),
            col(
                "Live sessions",
                vec![
                    kv("Event feed", "feed.status"),
                    table_actions("Sessions", "live", LIVE_COLS, live_row_actions()),
                ],
            ),
        ),
        spec(
            "stream",
            "Stream",
            "/sepia/stream",
            "One session's merged transcript — the paged `sessions.history` head plus the live event-feed tail flattened to rows (text/reasoning deltas coalesced per message, tool calls, permission asks). Requires --param id=<session-id> (--param agent= when ids collide across stores); session actions dispatch with the view's resolved id.",
            &["session", "stream"],
            Some(REFRESH_SESSION_MS),
            col(
                "Stream",
                vec![
                    kv_actions("Session", "session", stream_actions()),
                    table("Transcript", "stream", HISTORY_COLS),
                ],
            ),
        ),
        spec(
            "permissions",
            "Permissions",
            "/sepia/permissions",
            "Pending `session/request_permission` asks the feed has seen and not yet resolved — `sessions.permission` answers them (the row supplies session id + request id; pick an option id from the row's options column). Feed health is the `feed.status` kv.",
            &["feed.status", "permissions"],
            Some(REFRESH_FEED_MS),
            col(
                "Permissions",
                vec![
                    kv("Event feed", "feed.status"),
                    table_actions(
                        "Pending permissions",
                        "permissions",
                        PERM_COLS,
                        perm_row_actions(),
                    ),
                ],
            ),
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
    // Platform feeds (P3): busy sessions ride `jobs`, pending
    // permission asks + feed health ride `notifications`.
    let mut jobs_feed = kit::contribution("live", "Sepia live sessions", "/sepia/live");
    jobs_feed.priority = 30;
    let mut notif_feed = kit::contribution("alerts", "Sepia alerts", "/sepia/permissions");
    notif_feed.priority = 30;

    let mut m = kit::Manifest::new("sepia", "Sepia")
        .version(env!("CARGO_PKG_VERSION"))
        .icon("🐙")
        .navbar(navbar)
        .dashboard(dashboard)
        .statusbar(statusbar)
        .jobs(jobs_feed)
        .notifications(notif_feed);
    for a in global_actions() {
        m = m.action(a);
    }
    for e in menu_entries() {
        m = m.menu(e);
    }
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
        "summary"
            | "sessions"
            | "session"
            | "history"
            | "agents"
            | "config.agents"
            | "config.detail"
            | "stores"
            | "drivers"
            | "live"
            | "stream"
            | "permissions"
            | "feed.status"
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
        "config.agents" => config_agent_rows(params),
        "config.detail" => config_detail(params),
        "stores" => stores_rows(params),
        "drivers" => drivers_rows(params),
        "live" => live_rows(params),
        "stream" => stream_rows(params),
        "permissions" => permissions_rows(params),
        "feed.status" => feed::status(),
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
/// long transcripts truncate. `pub(crate)` — the feed module's
/// stream-row flattener shares it.
const CELL_MAX: usize = 200;

pub(crate) fn cell_text(content: &Value) -> String {
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

// ------------------------------------------------------------- config view

/// `config.agents` query — `config.list` inventory rows merged with
/// `agents.list` labels/runtime (the `agents` query's pattern,
/// config-first: `config.list` is authoritative, `agents.list` only
/// decorates). `agent`/`dir` forward like the `agents` query; a
/// `config.list` failure fails the table — there's nothing to merge
/// into.
fn config_agent_rows(ctx: &Map<String, Value>) -> Value {
    let mut params = agent_params(ctx);
    if let Some(dir) = data::dir_param(ctx) {
        params.insert("dir".to_string(), json!(dir));
    }
    let configs = match serve("config.list", Value::Object(params), ctx) {
        Ok(Value::Array(rows)) => rows,
        Ok(other) => return other,
        Err(e) => return e,
    };
    let agents = serve("agents.list", json!({}), ctx)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    Value::Array(
        configs
            .iter()
            .map(|c| config_agent_row(c, &agents))
            .collect(),
    )
}

/// One `config.list` row + its `agents.list` twin → table row. Pure.
/// `from` doubles as the target-merge key for `config.*` cli-lane
/// actions — an object row target contributes it as `--from`.
fn config_agent_row(cfg: &Value, agents: &[Value]) -> Value {
    let id = cfg["agent"].as_str().unwrap_or_default();
    let agent = agents.iter().find(|a| a["id"].as_str() == Some(id));
    let counts = cfg.get("counts");
    let count = |key: &str| counts.map_or(Value::Null, |c| c[key].clone());
    json!({
        "id": id,
        "from": id,
        "label": agent.map_or_else(|| json!(id), |a| a["label"].clone()),
        "runtime": agent.map_or(Value::Null, |a| {
            if a["capabilities"].is_null() { json!("—") } else { json!("acp") }
        }),
        "skills": count("skills"),
        "rules": count("rules"),
        "commands": count("commands"),
        "hooks": count("hooks"),
        "subagents": count("agents"),
        "mcpServers": count("mcpServers"),
        "error": cfg.get("error").cloned().unwrap_or(Value::Null),
    })
}

/// `config.detail` query — `config.get` for one agent, flattened
/// into tree `items` (kind sections → entries). `agent` selects the
/// config; absent → the first `config.list` row's agent (the view's
/// table shows all agents but the tree needs one). `dir` forwards
/// to both calls.
fn config_detail(ctx: &Map<String, Value>) -> Value {
    let mut params = Map::new();
    if let Some(dir) = data::dir_param(ctx) {
        params.insert("dir".to_string(), json!(dir));
    }
    let agent = if let Some(a) = data::agent_param(ctx) {
        Some(a)
    } else {
        let mut list_params = Map::new();
        if let Some(dir) = params.get("dir") {
            list_params.insert("dir".to_string(), dir.clone());
        }
        match serve("config.list", Value::Object(list_params), ctx) {
            Ok(Value::Array(rows)) => rows
                .first()
                .and_then(|r| r["agent"].as_str().map(str::to_string)),
            _ => None,
        }
    };
    let Some(agent) = agent else {
        return json!({"error": "no config-capable agents — pass --param agent=<id>"});
    };
    params.insert("agent".to_string(), json!(agent));
    match serve("config.get", Value::Object(params), ctx) {
        Ok(config) if config.is_object() => json!({
            "agent": agent,
            "items": config_items(&config),
        }),
        Ok(other) => other,
        Err(e) => e,
    }
}

/// Flatten a `config.get` AgentConfig IR into tree items — one
/// section node per non-empty kind (skills/rules/commands/subagents/
/// hooks/MCP servers), entries as children. Pure — unit-tested with
/// synthetic JSON.
fn config_items(config: &Value) -> Vec<Value> {
    let mut items = Vec::new();
    for (key, label) in [
        ("skills", "Skills"),
        ("rules", "Rules"),
        ("commands", "Commands"),
        ("agents", "Subagents"),
    ] {
        if let Some(section) = named_section(config, key, label) {
            items.push(section);
        }
    }
    if let Some(section) = hook_section(config) {
        items.push(section);
    }
    if let Some(section) = mcp_section(config) {
        items.push(section);
    }
    items
}

/// One named-item section (`skills`/`rules`/`commands`/`agents`) —
/// `{name, description?, body?}` entries → `{label, detail}`
/// children. `detail` is the description, else a flattened body
/// snippet.
fn named_section(config: &Value, key: &str, label: &str) -> Option<Value> {
    let entries = config.get(key)?.as_array()?;
    if entries.is_empty() {
        return None;
    }
    let children: Vec<Value> = entries
        .iter()
        .map(|e| {
            let name = e["name"].as_str().unwrap_or_default();
            let detail = e["description"]
                .as_str()
                .filter(|d| !d.is_empty())
                .map_or_else(|| cell_text(&e["body"]), str::to_string);
            let mut item = json!({"label": name});
            if !detail.is_empty() {
                item["detail"] = json!(detail);
            }
            item
        })
        .collect();
    Some(json!({
        "label": format!("{label} ({})", entries.len()),
        "children": children,
    }))
}

/// The `hooks` section — `{event, matcher?, command?|prompt?}` →
/// `event ~matcher → command` rows (the `config list` summary's
/// spelling).
fn hook_section(config: &Value) -> Option<Value> {
    let hooks = config.get("hooks")?.as_array()?;
    if hooks.is_empty() {
        return None;
    }
    let children: Vec<Value> = hooks
        .iter()
        .map(|h| {
            let event = h["event"].as_str().unwrap_or_default();
            let matcher = h["matcher"]
                .as_str()
                .map(|m| format!(" ~{m}"))
                .unwrap_or_default();
            let label = format!("{event}{matcher}");
            let detail = match (h["command"].as_str(), h["prompt"].as_str()) {
                (Some(c), _) => c.to_string(),
                (None, Some(p)) => format!("[prompt] {p}"),
                (None, None) => String::new(),
            };
            let mut item = json!({"label": label});
            if !detail.is_empty() {
                item["detail"] = json!(detail);
            }
            item
        })
        .collect();
    Some(json!({
        "label": format!("Hooks ({})", hooks.len()),
        "children": children,
    }))
}

/// The `mcpServers` section — a name → server-object map →
/// `command`/`url` details.
fn mcp_section(config: &Value) -> Option<Value> {
    let servers = config.get("mcpServers")?.as_object()?;
    if servers.is_empty() {
        return None;
    }
    let children: Vec<Value> = servers
        .iter()
        .map(|(name, s)| {
            let detail = s["command"]
                .as_str()
                .or_else(|| s["url"].as_str())
                .unwrap_or_default();
            let mut item = json!({"label": name});
            if !detail.is_empty() {
                item["detail"] = json!(detail);
            }
            item
        })
        .collect();
    Some(json!({
        "label": format!("MCP servers ({})", servers.len()),
        "children": children,
    }))
}

// --------------------------------------------------------------- stores view

/// `stores` query — `store list` per agent store through the cli
/// lane ([`ops::verb_output`] → `run store list --from <id>`),
/// merged into one table with a `store` column. A failing store
/// degrades to an error row, not a failed query. Store flags
/// (`db`, `data-dir`/`dataDir`, `claude-dir`/`claudeDir`,
/// `cursor-dir`/`cursorDir`) forward from view params.
fn stores_rows(ctx: &Map<String, Value>) -> Value {
    let mut rows = Vec::new();
    for store in ops::STORE_IDS {
        let mut params = Map::new();
        params.insert("from".to_string(), json!(store));
        for (view_key, flag) in [
            ("db", "db"),
            ("data-dir", "data-dir"),
            ("dataDir", "data-dir"),
            ("claude-dir", "claude-dir"),
            ("claudeDir", "claude-dir"),
            ("cursor-dir", "cursor-dir"),
            ("cursorDir", "cursor-dir"),
        ] {
            if let Some(v) = ctx.get(view_key).and_then(Value::as_str) {
                params.insert(flag.to_string(), json!(v));
            }
        }
        match ops::verb_output("store.list", &params) {
            Ok(out) if out.code == 0 => rows.extend(store_listing(store, &out.stdout)),
            Ok(out) => rows.push(store_error_row(store, &out.stderr, &out.stdout)),
            Err(e) => rows.push(store_error_row(store, &e, "")),
        }
    }
    Value::Array(rows)
}

/// `store list` TSV (`id\ttitle\tworkingDirectory` rows, or the
/// `No sessions found` line) → table rows. `session_id`/`from` ride
/// along as target-merge keys: an object row target contributes
/// them as the store verbs' positional/`--from` params. Pure.
fn store_listing(store: &str, stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty() && *l != "No sessions found")
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let id = cols.next()?.trim();
            if id.is_empty() {
                return None;
            }
            Some(json!({
                "store": store,
                "id": id,
                "session_id": id,
                "title": cols.next().unwrap_or_default(),
                "cwd": cols.next().unwrap_or_default(),
                "from": store,
            }))
        })
        .collect()
}

/// A failed store's placeholder row — `session_id` stays empty so
/// required-param gating rejects row actions on it. Pure.
fn store_error_row(store: &str, stderr: &str, stdout: &str) -> Value {
    let msg = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("store list failed");
    json!({
        "store": store,
        "id": "(error)",
        "session_id": "",
        "title": msg,
        "cwd": "",
        "from": store,
        "error": msg,
    })
}

// -------------------------------------------------------------- drivers view

/// `drivers` query — `driver list --json` through the cli lane →
/// one row per discovered `sepia-driver-*` plus probe problems as
/// error rows. Non-zero exit or unparsable output degrades to
/// `{"error": …}`.
fn drivers_rows(_ctx: &Map<String, Value>) -> Value {
    let params = Map::from_iter([("json".to_string(), json!(true))]);
    match ops::verb_output("driver.list", &params) {
        Ok(out) if out.code == 0 => match serde_json::from_str::<Value>(&out.stdout) {
            Ok(v) => drivers_payload(&v),
            Err(e) => json!({"error": format!("driver list --json: unparsable output: {e}")}),
        },
        Ok(out) => {
            let msg = out
                .stderr
                .lines()
                .chain(out.stdout.lines())
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("driver list failed");
            json!({"error": msg})
        }
        Err(e) => json!({"error": e}),
    }
}

/// The `driver list --json` payload → row array: `drivers` entries
/// shaped by [`driver_row`], `problems` appended as error rows.
/// Pure.
fn drivers_payload(v: &Value) -> Value {
    let mut rows: Vec<Value> = v["drivers"]
        .as_array()
        .map(|ds| ds.iter().map(driver_row).collect())
        .unwrap_or_default();
    for p in v["problems"].as_array().into_iter().flatten() {
        rows.push(json!({
            "id": "(problem)",
            "label": p.as_str().unwrap_or_default(),
            "error": "probe failed",
        }));
    }
    Value::Array(rows)
}

/// One `driver list --json` entry → table row. Pure.
fn driver_row(d: &Value) -> Value {
    let caps = d["capabilities"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(capability_label)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    json!({
        "id": d["id"],
        "label": d["label"],
        "version": d["version"],
        "capabilities": caps,
        "agentCommand": d["agentCommand"],
        "agentCommandResolves": d["agentCommandResolves"],
        "binary": d["binary"],
        "error": Value::Null,
    })
}

/// A `Capability` wire value → the compact label `driver list`'s
/// table prints: the camelCase name for unit variants,
/// `convert:{from}->{to}` for a conversion pair. Pure.
fn capability_label(c: &Value) -> String {
    if let Some(s) = c.as_str() {
        return s.to_string();
    }
    let pair = &c["convert"];
    if pair.is_object() {
        format!(
            "convert:{}->{}",
            pair["from"].as_str().unwrap_or_default(),
            pair["to"].as_str().unwrap_or_default()
        )
    } else {
        c.to_string()
    }
}

// ------------------------------------------------------------- event feed
//
// The P3 live-event surface: `live`/`stream`/`permissions`/
// `feed.status` queries read the feed child's buffer
// ([`crate::feed`]); `job_items`/`notification_items` back the
// `jobs`/`notifications` wire ops declared via `contributes`.

/// `live` query — feed sessions merged with `sessions.list` rows
/// (title/cwd/plane flags). Feed state wins for the status word
/// (`busy`/`attached`/`detached`); `sessions.list` is best-effort —
/// feed rows stand alone on its failure.
fn live_rows(ctx: &Map<String, Value>) -> Value {
    let feed_rows = feed::live();
    if feed_rows.is_empty() {
        return Value::Array(Vec::new());
    }
    let list = serve("sessions.list", json!({}), ctx)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    Value::Array(
        feed_rows
            .iter()
            .map(|f| {
                let id = f["id"].as_str().unwrap_or_default();
                let agent = f["agent"].as_str();
                let row = list.iter().find(|s| {
                    s["id"].as_str() == Some(id)
                        && agent.is_none_or(|a| {
                            s["agent"].as_str() == Some(a) || s["agentId"].as_str() == Some(a)
                        })
                });
                json!({
                    "id": id,
                    "title": row.map_or(Value::Null, |r| r["title"].clone()),
                    "agent": row.map_or_else(|| f["agent"].clone(), |r| r["agent"].clone()),
                    "status": f["status"],
                    "cwd": row.map_or(Value::Null, |r| r["cwd"].clone()),
                    "lastEvent": f["lastEvent"],
                    "pending": f["pending"],
                    "lagged": f["lagged"],
                    "attached": f["attached"],
                    "running": f["running"],
                })
            })
            .collect(),
    )
}

/// Trailing history rows kept ahead of the live tail — the stream
/// view shows recent context, not the whole backlog (paging stays
/// on the `history` query).
const STREAM_HEAD_ROWS: usize = 200;

/// `stream` query — `sessions.history` head (same shaping as the
/// `history` query) + the feed's buffered `SessionEvent` tail
/// flattened to rows. A history failure degrades to a marker row +
/// the live tail rather than failing the whole transcript.
fn stream_rows(ctx: &Map<String, Value>) -> Value {
    let Some(id) = ctx.get("id").and_then(Value::as_str) else {
        return json!({"error": "missing required param 'id' (e.g. --param id=<session-id>)"});
    };
    let mut rows: Vec<Value> = Vec::new();
    match history_rows(ctx) {
        Value::Array(h) => {
            let n = h.len();
            rows.extend(h.into_iter().skip(n.saturating_sub(STREAM_HEAD_ROWS)));
        }
        other => {
            let msg = other["error"].as_str().unwrap_or("history unavailable");
            rows.push(json!({
                "role": "system",
                "time": "",
                "node": Value::Null,
                "tool": "",
                "content": format!("history: {msg}"),
            }));
        }
    }
    rows.extend(feed::stream_tail(id));
    Value::Array(rows)
}

/// `permissions` query — pending asks from the feed buffer,
/// `agent`-filtered when the param is supplied (asks whose agent
/// the feed doesn't know stay visible).
fn permissions_rows(ctx: &Map<String, Value>) -> Value {
    let agent = data::agent_param(ctx);
    Value::Array(
        feed::permissions()
            .into_iter()
            .filter(|r| {
                agent
                    .as_deref()
                    .is_none_or(|a| r["agent"].as_str() == Some(a) || r["agent"].is_null())
            })
            .collect(),
    )
}

/// RFC 3339 now — the platform feeds' timestamp shape.
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// `{"op":"jobs"}` — mid-turn sessions as [`JobItem`]s: the feed's
/// busy set enriched with `sessions.list` titles. Empty feed →
/// empty list (still `ok:true`).
pub fn job_items() -> Vec<JobItem> {
    let busy = feed::busy();
    if busy.is_empty() {
        return Vec::new();
    }
    let titles = serve("sessions.list", json!({}), &Map::new())
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let now = now_rfc3339();
    busy.into_iter()
        .map(|(id, agent)| {
            let title = titles
                .iter()
                .find(|s| s["id"].as_str() == Some(id.as_str()))
                .and_then(|s| s["title"].as_str())
                .unwrap_or(id.as_str());
            let mut j = kit::job(id.clone(), title, "running");
            j.detail = (!agent.is_empty()).then(|| format!("agent {agent}"));
            j.route = Some(format!("/sepia/stream?id={id}"));
            j.updated_at = Some(now.clone());
            j
        })
        .collect()
}

/// `{"op":"notifications"}` — pending permission asks (warning,
/// route to `sepia:permissions`) plus feed-health alerts: `down`
/// (error) and cumulative `lagged` drops (warning). Empty buffer →
/// empty list (still `ok:true`).
pub fn notification_items() -> Vec<NotificationItem> {
    let mut out = Vec::new();
    for p in feed::permissions() {
        let sid = p["id"].as_str().unwrap_or_default();
        let rid = p["request"].as_str().unwrap_or_default();
        let title = p["title"].as_str().unwrap_or("permission requested");
        let mut body = format!("session {sid}");
        if let Some(a) = p["agent"].as_str() {
            let _ = write!(body, " · agent {a}");
        }
        if let Some(t) = p["tool"].as_str().filter(|t| !t.is_empty()) {
            let _ = write!(body, " · {t}");
        }
        if let Some(o) = p["options"].as_str().filter(|o| !o.is_empty()) {
            let _ = write!(body, " — options: {o}");
        }
        let mut n = kit::notification(
            format!("perm:{sid}:{rid}"),
            format!("Permission requested — {title}"),
        );
        n.body = Some(body);
        n.severity = Some("warning".to_string());
        n.route = Some("/sepia/permissions".to_string());
        n.created_at = p["since"].as_str().map(str::to_string);
        out.push(n);
    }
    let status = feed::status();
    if status["status"].as_str() == Some("down") {
        let mut n = kit::notification("feed-down", "Sepia event feed disconnected");
        n.body = status["detail"]
            .as_str()
            .map(|d| format!("events.subscribe failing: {d}"));
        n.severity = Some("error".to_string());
        n.route = Some("/sepia/live".to_string());
        out.push(n);
    }
    let lagged = status["lagged"].as_u64().unwrap_or(0);
    if lagged > 0 {
        let mut n = kit::notification(
            "feed-lagged",
            format!("Sepia event feed lagged — {lagged} events dropped"),
        );
        n.severity = Some("warning".to_string());
        n.route = Some("/sepia/live".to_string());
        out.push(n);
    }
    out
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

    // Every id-scoped view records its resolved `id`/`agent` — the
    // invoke lane merges the *invoking* view's context into
    // session-scoped serve actions (the wire carries no view params).
    note_view_context(view_id, params);
    // The session view is additionally stateful: the returned spec's
    // form fields get seeded from the `session` detail so the meta
    // form edits current values, not blanks.
    let mut spec = spec;
    if view_id == "session" {
        if let Some(session) = data_map
            .get("session")
            .filter(|v| v.is_object() && v.get("error").is_none())
        {
            patch_form_values(&mut spec.root, session);
        }
    }

    Ok(pavo_protocol::wire::ViewResult {
        view: spec,
        data: data_map,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::many_single_char_names)]
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
        for want in [
            "status", "sessions", "session", "agents", "config", "stores", "drivers",
        ] {
            assert!(ids.contains(want), "missing view {want}");
        }
    }

    #[test]
    fn views_arm_auto_refresh_at_their_cadence() {
        let refresh = |id: &str| {
            specs()
                .into_iter()
                .find(|v| v.id == id)
                .unwrap_or_else(|| panic!("missing view {id}"))
                .refresh_ms
        };
        // buffer-only feed view — fastest tick
        assert_eq!(refresh("permissions"), Some(REFRESH_FEED_MS));
        // session-plane views — serve call per tick
        for id in ["sessions", "session", "live", "stream"] {
            assert_eq!(refresh(id), Some(REFRESH_SESSION_MS), "{id}");
        }
        assert_eq!(refresh("status"), Some(REFRESH_STATUS_MS));
        assert_eq!(refresh("stores"), Some(REFRESH_CLI_MS));
        for id in ["agents", "config", "drivers"] {
            assert_eq!(refresh(id), Some(REFRESH_SLOW_MS), "{id}");
        }
        // every declared view is armed — no silent manual-only views
        for v in specs() {
            assert!(v.refresh_ms.is_some(), "{} has no refresh_ms", v.id);
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

    #[test]
    fn manifest_declares_actions_and_menus() {
        let m = manifest();
        assert_ne!(m.actions.len(), 0);
        assert_ne!(m.contributes.menus.len(), 0);
        let ids: std::collections::BTreeSet<&str> =
            m.actions.iter().map(|a| a.id.as_str()).collect();
        // the curated session-plane set — every id must resolve on
        // the invoke lanes (serve method or engine verb)
        for want in [
            "session.create",
            "session.attach",
            "session.detach",
            "session.prompt",
            "session.cancel",
            "session.permission",
            "session.set_mode",
            "session.set_permission_policy",
            "session.delete",
            "session.patch_meta",
            "session.restore",
            "session.rewind",
            "session.export",
            "session.import",
            "session.convert",
        ] {
            assert!(ids.contains(want), "missing action {want}");
            let a = m.actions.iter().find(|a| a.id == want).unwrap();
            assert!(
                crate::ops::serve_method(&a.id).is_some(),
                "{want} doesn't route to a serve method"
            );
        }
        // danger sanity — irreversible ops never `none`
        for a in &m.actions {
            if matches!(
                a.id.as_str(),
                "session.delete" | "session.restore" | "session.rewind"
            ) {
                assert_eq!(a.danger, pavo_protocol::DangerLevel::TypeTarget, "{}", a.id);
            }
        }
        // node-scoped actions ride the view specs
        let sessions = specs().into_iter().find(|v| v.id == "sessions").unwrap();
        let ViewNode::Column(c) = &sessions.root else {
            panic!("sessions root is not a column");
        };
        let ViewNode::Table(t) = &c.children[0] else {
            panic!("sessions child is not a table");
        };
        assert_ne!(t.base.actions.len(), 0, "sessions table has no actions");
        let session = specs().into_iter().find(|v| v.id == "session").unwrap();
        let ViewNode::Column(c) = &session.root else {
            panic!("session root is not a column");
        };
        let ViewNode::Kv(kv) = &c.children[0] else {
            panic!("session child is not a kv");
        };
        assert_ne!(kv.base.actions.len(), 0, "session kv has no actions");
        // the meta form sits between the kv and the history table
        let ViewNode::Form(f) = &c.children[1] else {
            panic!("session child[1] is not a form");
        };
        assert_eq!(f.base.id.as_deref(), Some("meta"));
        assert_eq!(f.submit_op.as_deref(), Some("session.patch_meta"));
        // the new views carry their row actions
        for (view, idx) in [("stores", 0usize), ("config", 0usize)] {
            let v = specs().into_iter().find(|v| v.id == view).unwrap();
            let ViewNode::Column(c) = &v.root else {
                panic!("{view} root is not a column");
            };
            let ViewNode::Table(t) = &c.children[idx] else {
                panic!("{view} child is not a table");
            };
            assert_ne!(t.base.actions.len(), 0, "{view} table has no actions");
        }
    }

    #[test]
    fn config_agent_row_merges_counts_and_label() {
        let cfg = json!({
            "agent": "fake",
            "counts": {"skills": 2, "rules": 1, "commands": 0, "hooks": 3, "agents": 4, "mcpServers": 5},
        });
        let agents = vec![json!({"id": "fake", "label": "Fake", "capabilities": {"x": 1}})];
        let row = config_agent_row(&cfg, &agents);
        assert_eq!(row["id"], "fake");
        assert_eq!(row["from"], "fake"); // target-merge key for config.* actions
        assert_eq!(row["label"], "Fake");
        assert_eq!(row["runtime"], "acp");
        assert_eq!(row["skills"], 2);
        assert_eq!(row["subagents"], 4);
        assert_eq!(row["mcpServers"], 5);
        // no agents.list twin → label falls back to the id, runtime null
        let row = config_agent_row(&cfg, &[]);
        assert_eq!(row["label"], "fake");
        assert!(row["runtime"].is_null());
    }

    #[test]
    fn config_items_flattens_every_kind() {
        let config = json!({
            "skills": [{"name": "tidy", "description": "clean code"}],
            "rules": [{"name": "house", "body": "be nice"}],
            "commands": [{"name": "ship", "body": "deploy it"}],
            "agents": [{"name": "scout", "description": "look around"}],
            "hooks": [
                {"event": "PreToolUse", "matcher": "Write", "command": "lint.sh"},
                {"event": "Stop", "prompt": "review?"}
            ],
            "mcpServers": {"docs": {"command": "mcp-docs"}},
        });
        let items = config_items(&config);
        let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert_eq!(
            labels,
            [
                "Skills (1)",
                "Rules (1)",
                "Commands (1)",
                "Subagents (1)",
                "Hooks (2)",
                "MCP servers (1)"
            ]
        );
        // description wins over body; body flattens when no description
        assert_eq!(items[0]["children"][0]["detail"], "clean code");
        assert_eq!(items[1]["children"][0]["detail"], "be nice");
        // hook label carries event+matcher, detail the command/prompt
        assert_eq!(items[4]["children"][0]["label"], "PreToolUse ~Write");
        assert_eq!(items[4]["children"][0]["detail"], "lint.sh");
        assert_eq!(items[4]["children"][1]["detail"], "[prompt] review?");
        assert_eq!(items[5]["children"][0]["detail"], "mcp-docs");
        // empty kinds emit no section
        assert_eq!(config_items(&json!({})), Vec::<Value>::new());
        assert_eq!(config_items(&json!({"skills": []})), Vec::<Value>::new());
    }

    #[test]
    fn store_listing_parses_tsv_and_skips_empty() {
        let rows = store_listing(
            "devin",
            "s1\tFirst\t/work/one\ns2\tSecond\t/work/two\nNo sessions found\n",
        );
        // the `No sessions found` sentinel is filtered, not a row
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0]["store"], "devin");
        assert_eq!(rows[0]["id"], "s1");
        assert_eq!(rows[0]["session_id"], "s1"); // target-merge key
        assert_eq!(rows[0]["title"], "First");
        assert_eq!(rows[0]["cwd"], "/work/one");
        assert_eq!(rows[0]["from"], "devin"); // target-merge key
        let clean = store_listing("cline", "No sessions found\n\n");
        assert!(clean.is_empty(), "{clean:?}");
    }

    #[test]
    fn store_error_row_disables_row_actions() {
        let row = store_error_row("claude", "boom: no such dir\n", "");
        assert_eq!(row["store"], "claude");
        assert_eq!(row["error"], "boom: no such dir");
        assert_eq!(row["session_id"], "", "required-param gate must reject");
        // falls back to stdout when stderr is empty
        let row = store_error_row("cursor", "", "weird output\n");
        assert_eq!(row["title"], "weird output");
    }

    #[test]
    fn driver_row_and_caps_shaping() {
        let row = driver_row(&json!({
            "id": "fake", "label": "Fake", "version": "0.0.1",
            "capabilities": ["sessionStore", {"convert": {"from": "cline", "to": "devin"}}],
            "agentCommand": "fake-acp", "agentCommandResolves": false,
            "binary": "/bin/fake",
        }));
        assert_eq!(row["id"], "fake");
        assert_eq!(row["capabilities"], "sessionStore, convert:cline->devin");
        assert_eq!(row["agentCommandResolves"], false);
        // problems append as error rows
        let rows = drivers_payload(&json!({
            "drivers": [{"id": "fake"}],
            "problems": ["sepia-driver-bad: probe failed"],
        }));
        let arr = rows.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["id"], "(problem)");
        assert_eq!(arr[1]["error"], "probe failed");
    }

    #[test]
    fn form_submit_op_resolves_declared_forms() {
        assert_eq!(
            form_submit_op("session", Some("meta")).as_deref(),
            Some("session.patch_meta")
        );
        // absent form → the view's first (only) form
        assert_eq!(
            form_submit_op("session", None).as_deref(),
            Some("session.patch_meta")
        );
        // unknown form/view → None (the verb-named fallback applies)
        assert!(form_submit_op("session", Some("bogus")).is_none());
        assert!(form_submit_op("sessions", Some("meta")).is_none());
        assert!(form_submit_op("bogus", None).is_none());
    }
}
