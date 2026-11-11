//! Embedded surface dispatch — the driver serving its own engine's
//! ops through `pavo-driver-kit`'s surface layer, no second binary
//! needed. `cli`/`mcp` argv verbs (feature-gated) plug this
//! [`Surface`] impl into `surface::cli::run` /
//! `surface::mcp::serve_stdio`.
//!
//! `op` reuses the same `cli.*` path wire queries take
//! (`ops::run_verb` re-execs `run` in a child — output stays
//! captured off the serve stream). `surface` dispatches in-process
//! through `sepia_cli::run_from`, which owns the runtime exactly
//! like the `sepia` bin — that's where the unbounded `serve` verb
//! lives, since it needs inherited stdio a captured op child can't
//! provide.

use pavo_driver_kit::surface::{OpOutcome, OpSpec, Surface};
use serde_json::{Map, Value};

use crate::ops;

/// sepia's [`Surface`] — engine dispatch, already linked.
pub struct Engine;

impl Surface for Engine {
    /// Same path as the `cli.<verb>` wire query: re-exec `run` in
    /// a child, captured output → `OpOutcome`.
    fn op(&mut self, op: &OpSpec, params: &Map<String, Value>) -> OpOutcome {
        OpOutcome::from_response(&ops::run_op(&op.name, params))
    }

    /// `sepia <name> <args>` in-process — for the `serve` surface:
    /// inherited stdio, multi-thread runtime, telemetry init. Bounded
    /// ops never reach here.
    fn surface(&mut self, name: &str, args: &[String]) -> i32 {
        let argv = std::iter::once("sepia".to_string())
            .chain(std::iter::once(name.to_string()))
            .chain(args.iter().cloned());
        sepia_cli::run_from(argv)
    }
}
