//! Driver conformance gate — runs pavo's wire-contract suite
//! ([`pavo_testkit::conformance::check_driver_with`]) against this
//! crate's own `tamias-driver-sepia serve` binary.
//!
//! `manifest`/`views` never touch sepia state; `view`/`query` ops
//! spawn `sepia serve` (resolved via `$SEPIA_BIN`, a workspace-sibling
//! `target/debug/sepia`, or PATH) and degrade to `{"error": …}`
//! payloads when no binary resolves.
//!
//! The timeout is raised above the 10s default: the first data op
//! pays for the `sepia serve` plane build (driver discovery + store
//! probes), which can exceed 10s on a loaded machine — and a timed-out
//! request leaves a late response that desyncs the rest of the
//! suite. Where no `sepia` binary resolves (fresh CI clone) every op
//! degrades instantly, so the longer budget costs nothing.

use std::path::Path;
use std::time::Duration;

#[test]
fn driver_passes_pavo_conformance() {
    let violations = pavo_testkit::conformance::check_driver_with(
        Path::new(env!("CARGO_BIN_EXE_tamias-driver-sepia")),
        pavo_testkit::CheckOptions {
            timeout: Duration::from_secs(120),
        },
    );
    assert!(
        violations.is_empty(),
        "pavo conformance violations:\n{}",
        violations
            .iter()
            .map(|v| format!("  - {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
