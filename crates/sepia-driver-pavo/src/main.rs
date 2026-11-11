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
//! - `serve` — ndjson JSON-RPC on stdio: `{"op":"manifest"}`,
//!   `{"op":"views"}`, `{"op":"view","view":…,"params":{…}}`,
//!   `{"op":"query","query":…,"params":{…}}` →
//!   `{"ok":true,"result":…}` / `{"ok":false,"error":…}`.
//!   `query:"ops"` answers the ops manifest; `query:"cli.<verb>"`
//!   runs the engine op.
//!
//! Optional surfaces: features `cli`/`mcp` embed
//! pavo-driver-kit's surface layer — `cli <op> [args]` is a
//! generated CLI over the ops manifest, `mcp` serves the ops as
//! MCP tools on stdio. Both are equivalent to the standalone
//! `tamias-driver-cli`/`tamias-driver-mcp` surface drivers, minus
//! the PATH/discovery hop.
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
#[cfg(any(feature = "cli", feature = "mcp"))]
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
        // Embedded surface verbs (feature-gated, optional): the
        // pavo-driver-kit surface layer renders this driver's own
        // ops manifest — `cli` = generated clap CLI (op →
        // subcommand), `mcp` = ops → MCP tools over stdio. The
        // standalone tamias-driver-cli/-mcp binaries do the same
        // over process dispatch; these verbs need nothing on PATH.
        #[cfg(feature = "cli")]
        Some("cli") => {
            let m = kit::surface::OpsManifest::from_value(&ops::manifest()).unwrap_or_default();
            let argv: Vec<String> = std::iter::once("sepia".to_string())
                .chain(args.iter().skip(1).cloned())
                .collect();
            kit::surface::cli::run("sepia", &m, &argv, &mut surface::Engine)
        }
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
                "usage: tamias-driver-sepia manifest|capabilities|ops|run <argv>|serve\n       (feature `cli` adds `cli <op> [args]`; feature `mcp` adds `mcp`)"
            );
            1
        }
    };
    std::process::exit(code);
}
