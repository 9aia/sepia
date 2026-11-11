//! tamias-driver-sepia — the pavo view driver for sepia's session
//! plane, plus the engine ops surface.
//!
//! Verbs (the tamias driver contract):
//!
//! - `manifest` — print the [`DriverManifest`] JSON (discovery
//!   probe; never touches sepia state).
//! - `capabilities` — `{"capabilities": ["pavo"], "verbs": […]}`.
//! - `ops` — the engine ops manifest: every bounded `sepia` CLI
//!   verb as a `cli.<verb>` wire query, derived from the engine's
//!   own clap tree ([`sepia_cli::command`]). The unbounded `serve`
//!   verb is a `surface`, never an op.
//! - `run <argv>` — engine passthrough: `tamias-driver-sepia run
//!   list` ≡ `sepia list`. Also the child side of `cli.*` wire
//!   queries (`ops::run_verb` re-execs this same verb so engine
//!   output never touches the serve stream).
//! - `cli <sepia argv>` — the tamias `bins[]` shim verb: `tamias
//!   bin exec` calls it with the standard `--dir`/`--name` driver
//!   flags ahead of the user's argv; they're stripped and the rest
//!   runs through `sepia_cli::run_from` in-process with inherited
//!   stdio — full engine parity (`serve` streams, globals/`--json`/
//!   `--help` all work).
//! - `serve` — ndjson JSON-RPC on stdio: `{"op":"manifest"}`,
//!   `{"op":"views"}`, `{"op":"view","view":…,"params":{…}}`,
//!   `{"op":"query","query":…,"params":{…}}` →
//!   `{"ok":true,"result":…}` / `{"ok":false,"error":…}`.
//!   `query:"ops"` answers the ops manifest; `query:"cli.<verb>"`
//!   runs the engine op.
//!
//! Optional surface: feature `mcp` embeds pavo-driver-kit's
//! surface layer — `mcp` serves the ops manifest as MCP tools on
//! stdio, equivalent to the standalone `tamias-driver-mcp` surface
//! driver minus the PATH/discovery hop. (`cli` needs no feature:
//! it's the bins-shim passthrough above, not a generated surface —
//! the engine's own argv grammar is the contract.)
//!
//! Read path: every `view`/`query` (except `ops`/`cli.*`) proxies
//! read-side methods of a spawned `sepia serve` (`sepia serve` is
//! the session plane's embedding surface — see `data.rs` for
//! binary resolution). The view side stays strictly read-only:
//! attach/prompt/delete and friends are engine *ops* now — they
//! run through `cli.*`/`run` with the engine's own semantics, never
//! through the view queries. `--param sepiaBin=` (or `$SEPIA_BIN`)
//! pins which `sepia` executable serves the views.

mod data;
mod ops;
#[cfg(feature = "mcp")]
mod surface;
mod views;

use pavo_driver_kit as kit;
use pavo_protocol::wire::{self, Request, Response, ViewSummary};
use serde_json::{Value, json};

fn capabilities_json() -> Value {
    let m = views::manifest();
    // The kit's shape plus the view count every sibling driver
    // advertises.
    let mut caps = kit::capabilities_value(&m);
    caps["views"] = json!(m.views.len());
    caps
}

/// Dispatch one parsed request to its response.
fn dispatch(req: &Request) -> Response {
    match req {
        Request::Manifest => Response::ok_typed(&views::manifest()),
        Request::Views => {
            let summaries: Vec<ViewSummary> =
                views::specs().iter().map(ViewSummary::from).collect();
            Response::ok_typed(&summaries)
        }
        Request::View { view, params } => match views::view_result(view, params) {
            Ok(result) => Response::ok_typed(&result),
            Err(e) => Response::err(e),
        },
        Request::Render { kind, .. } => Response::err(format!(
            "tamias-driver-sepia claims no renderers (kind '{kind}')"
        )),
        Request::Query { query, params } => {
            // `ops` and `cli.<verb>` are engine ops; everything
            // else is a view query — unknown names fail the op,
            // degraded reads travel as `{"error": …}` data (the
            // marker renderers understand), matching the reference
            // driver's contract.
            if query == "ops" {
                Response::ok_typed(&wire::QueryResult {
                    query: query.clone(),
                    data: ops::manifest(),
                })
            } else if let Some(v) = ops::op_of(query) {
                ops::run_verb(&v, params)
            } else if views::known_query(query) {
                let payload = views::query_data(query, params);
                Response::ok_typed(&wire::QueryResult {
                    query: query.clone(),
                    data: payload,
                })
            } else {
                Response::err(format!("unknown query: {query}"))
            }
        }
        // `submit`/`jobs`/`notifications` — the manifest declares no
        // `contributes` feed slots and mutations go through
        // `cli.<verb>`/`run`, never `submit`, so they answer the same
        // unsupported-op error hosts read as "no feed".
        Request::Submit { .. } | Request::Jobs | Request::Notifications => {
            Response::err("tamias-driver-sepia implements no submit/jobs/notifications ops")
        }
    }
}

/// `cli` verb argv (after the `cli` token) → engine argv (argv[0]
/// included). `tamias bin exec`/`driver exec` prepend the standard
/// driver flags — `--dir <payload>` `--name <pkg>` — which the
/// engine doesn't know; consume the leading run of those two
/// (`--flag v` and `--flag=v` forms) and pass everything else
/// through untouched.
fn engine_argv(args: &[String]) -> Vec<String> {
    let mut out = vec!["sepia".to_string()];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--dir" || a == "--name" {
            // flag + separate value — skip both (a trailing flag
            // with no value just ends the scan).
            i = (i + 2).min(args.len());
        } else if a.starts_with("--dir=") || a.starts_with("--name=") {
            i += 1;
        } else {
            break;
        }
    }
    out.extend(args[i..].iter().cloned());
    out
}

fn serve() -> i32 {
    // Pre-spawn `sepia serve` so its plane build overlaps the
    // manifest/views handshake — see `data::warm`.
    data::warm();
    // The kit loop answers every line (parse failures become
    // `ok:false`), ends quietly on stdin EOF or a dead client.
    let _ = kit::serve_stdio(dispatch);
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("manifest") => {
            println!("{}", kit::manifest_json(&views::manifest()));
            0
        }
        Some("capabilities") => {
            println!(
                "{}",
                serde_json::to_string_pretty(&capabilities_json()).unwrap_or_default()
            );
            0
        }
        Some("ops") => {
            println!("{}", ops::manifest());
            0
        }
        // Engine passthrough — `tamias-driver-sepia run list` is
        // `sepia list`. Also the child side of `cli.*` wire
        // queries (ops::run_verb re-execs this same verb).
        Some("run") => {
            let argv = std::iter::once("sepia".to_string()).chain(args.iter().skip(1).cloned());
            sepia_cli::run_from(argv)
        }
        // The tamias `bins[]` shim verb (T-012): `tamias bin exec`
        // runs `<driver> cli --dir <payload> --name <pkg> <sepia
        // argv>` for a declared `shim: cli` bin. sepia's engine IS
        // the CLI, so this is a passthrough — strip the two standard
        // driver flags and hand the rest to `sepia_cli::run_from`
        // with inherited stdio. That keeps full engine parity a
        // generated ops→CLI surface can't express: globals before
        // the verb, `--json`, `--help`, the unbounded `serve`
        // ndjson loop, and stdin-bound verbs all behave exactly
        // like the retired `sepia` binary.
        Some("cli") => {
            let argv = engine_argv(&args[1..]);
            std::process::exit(sepia_cli::run_from(argv));
        }
        // Optional embedded surface (feature-gated): the
        // pavo-driver-kit surface layer renders this driver's own
        // ops manifest as MCP tools over stdio — the same ops the
        // `cli.*` wire queries and the standalone
        // tamias-driver-mcp binary expose.
        #[cfg(feature = "mcp")]
        Some("mcp") => {
            let m = kit::surface::OpsManifest::from_value(&ops::manifest()).unwrap_or_default();
            match kit::surface::mcp::serve_stdio(&m, &mut surface::Engine) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("tamias-driver-sepia mcp: {e}");
                    1
                }
            }
        }
        Some("serve") | None => serve(),
        _ => {
            eprintln!(
                "usage: tamias-driver-sepia manifest|capabilities|ops|run <argv>|cli <sepia argv>|serve\n       (feature `mcp` adds `mcp`)"
            );
            1
        }
    };
    std::process::exit(code);
}
