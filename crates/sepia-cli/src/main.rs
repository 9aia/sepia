//! sepia — the CLI for the agent-session abstraction. Local store verbs
//! over the Devin/Cline/Claude/Cursor stores, agent config IR verbs,
//! driver discovery, and `serve` — the ndjson JSON-RPC session-plane
//! service orchestrators (peregrin) spawn and drive over stdio.
//!
//! Thin front-end: the clap surface, verb dispatch, and `run_from`
//! live in `sepia_cli` so drivers can embed or re-exec the same
//! engine.

fn main() {
    std::process::exit(sepia_cli::run());
}
