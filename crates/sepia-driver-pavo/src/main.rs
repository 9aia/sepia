//! tamias-driver-sepia — the pavo view driver for sepia's session
//! plane.
//!
//! Verbs (the tamias driver contract):
//!
//! - `manifest` — print the [`DriverManifest`] JSON (discovery
//!   probe; never touches sepia state).
//! - `capabilities` — `{"capabilities": ["pavo"], "verbs": […]}`.
//! - `serve` — ndjson JSON-RPC on stdio: `{"op":"manifest"}`,
//!   `{"op":"views"}`, `{"op":"view","view":…,"params":{…}}`,
//!   `{"op":"query","query":…,"params":{…}}` →
//!   `{"ok":true,"result":…}` / `{"ok":false,"error":…}`.
//!
//! Read path: every `view`/`query` proxies read-side methods of a
//! spawned `sepia serve` (`sepia serve` is the session plane's
//! embedding surface — see `data.rs` for binary resolution). The
//! driver is strictly read-only: attach/prompt/delete and friends
//! stay on the `sepia` CLI and orchestrators. `--param sepiaBin=`
//! (or `$SEPIA_BIN`) pins which `sepia` executable serves the views.

mod data;
mod views;

use pavo_driver_kit as kit;
use pavo_protocol::wire::{self, Request, Response, ViewSummary};

fn capabilities_json() -> serde_json::Value {
    let m = views::manifest();
    let mut caps = kit::capabilities_value(&m);
    caps["views"] = serde_json::json!(m.views.len());
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
        Request::Query { query, params } => {
            // Unknown names fail the op; everything else answers
            // `ok:true` — degraded serve calls travel as
            // `{"error": …}` data (the marker renderers understand),
            // matching the reference driver's contract.
            if !views::known_query(query) {
                return Response::err(format!("unknown query: {query}"));
            }
            let payload = views::query_data(query, params);
            Response::ok_typed(&wire::QueryResult {
                query: query.clone(),
                data: payload,
            })
        }
        // `render`/`submit` — this driver ships no renderer plugins
        // and no forms; the manifest claims neither, so answering an
        // explicit error keeps the wire honest.
        Request::Render { kind, .. } => Response::err(format!(
            "tamias-driver-sepia claims no renderers (kind '{kind}')"
        )),
        Request::Submit { .. } => {
            Response::err("tamias-driver-sepia is read-only and implements no forms")
        }
        // `jobs`/`notifications` — the manifest declares neither
        // `contributes` feed slot, so both answer the same
        // unsupported-op error hosts read as "no feed".
        Request::Jobs | Request::Notifications => {
            Response::err("tamias-driver-sepia provides no jobs or notifications feed")
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
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("manifest") => {
            println!("{}", kit::manifest_json(&views::manifest()));
        }
        Some("capabilities") => {
            println!(
                "{}",
                serde_json::to_string_pretty(&capabilities_json()).unwrap_or_default()
            );
        }
        Some("serve") => std::process::exit(serve()),
        _ => {
            eprintln!("usage: tamias-driver-sepia manifest|capabilities|serve");
            std::process::exit(1);
        }
    }
}
