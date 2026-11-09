//! data — a read-only client for `sepia serve`.
//!
//! The driver's whole read path is the service wire: one spawned
//! `sepia serve` subprocess per `tamias-driver-sepia serve` process,
//! spoken to with the same ndjson JSON-RPC framing driver binaries
//! use ([`sepia_driver::rpc`]). The driver only ever issues the
//! read-side methods (`manifest`, `agents.list`, `sessions.list`,
//! `sessions.get`/`summary`/`history`, `projects.list`,
//! `config.list`) — every mutation stays on the `sepia` CLI and the
//! orchestrator-facing ops (`attach`, `prompt`, `delete`, …) are
//! never called, so the view driver cannot mutate session state.
//!
//! Which `sepia`: first hit wins —
//!
//! 1. Request params — `sepiaBin`, `sepia`, or `bin` (a path to the
//!    `sepia` executable), e.g.
//!    `pavo view sepia:sessions --param sepiaBin=/path/to/sepia`.
//! 2. `$SEPIA_BIN`.
//! 3. `sepia` on `PATH`.
//! 4. `sepia` next to this driver's own executable — covers
//!    `cargo build` workspace layouts where `target/debug/` holds
//!    both binaries.
//!
//! When nothing resolves or the spawn fails, [`call`] returns `Err`
//! and every query degrades to an `{"error": …}` payload — the wire
//! stays `ok:true` so hosts render the failure inline (same contract
//! as the sibling drivers' degraded snapshots). A dead serve process
//! is respawned once per call before the error surfaces.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

use sepia_driver::rpc::{self, Inbound};
use serde_json::{Map, Value};

/// One live `sepia serve` — requests are issued sequentially by the
/// driver's own serve loop, and we never subscribe, so every stdout
/// line is a response (notifications only flow after
/// `sessions.subscribe`/`events.subscribe`, which this driver never
/// calls).
struct Sepia {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    /// The executable this client spawned — a later call whose
    /// params/env resolve a *different* `sepia` drops and respawns
    /// rather than answering from the wrong plane.
    bin: PathBuf,
    next_id: u64,
}

impl Sepia {
    /// Spawn `<bin> serve` with piped stdin/stdout; stderr inherits
    /// (serve logs there — keeping it off the wire is the child's
    /// job, and inheriting keeps diagnostics visible).
    fn spawn(bin: &PathBuf) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn `{}` serve: {e}", bin.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "sepia serve: no stdin pipe".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "sepia serve: no stdout pipe".to_string())?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            bin: bin.clone(),
            next_id: 0,
        })
    }

    /// One JSON-RPC round-trip. Skips notifications and stray
    /// responses until the reply to `id` arrives.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let request = rpc::Request::new(id, method, params);
        let line = rpc::encode_line(&request).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("sepia serve stdin: {e}"))?;
        loop {
            let mut buf = String::new();
            let n = self
                .stdout
                .read_line(&mut buf)
                .map_err(|e| format!("sepia serve stdout: {e}"))?;
            if n == 0 {
                return Err("sepia serve closed stdout".to_string());
            }
            match rpc::decode_line(buf.trim()) {
                Ok(Inbound::Response(resp)) if resp.id == id => {
                    return match (resp.result, resp.error) {
                        (_, Some(err)) => Err(format!(
                            "sepia serve {method}: {} ({})",
                            err.message, err.code
                        )),
                        (Some(result), None) => Ok(result),
                        (None, None) => Err(format!("sepia serve {method}: empty response")),
                    };
                }
                // Notifications, responses to other ids, and
                // undecodable lines — none apply; keep reading.
                _ => {}
            }
        }
    }
}

impl Drop for Sepia {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Process-wide client slot — `Vacant` until the first call, `Ready`
/// while `sepia serve` lives, `Failed` caches a spawn/resolution
/// error so a missing binary isn't re-probed per query.
enum Slot {
    Vacant,
    Ready(Box<Sepia>),
    Failed(String),
}

static CLIENT: Mutex<Slot> = Mutex::new(Slot::Vacant);

impl Slot {
    /// Ensure a live client for `ctx`'s resolved binary, then issue
    /// `method`.
    fn call(
        &mut self,
        method: &str,
        params: Value,
        ctx: &Map<String, Value>,
    ) -> Result<Value, String> {
        // A warmed client may have spawned a different `sepia` than
        // this call's params/env resolve — retarget before use.
        if let Slot::Ready(sepia) = self {
            if sepia.bin != resolve_bin(ctx) {
                *self = Slot::Vacant;
            }
        }
        if matches!(self, Slot::Vacant) {
            *self = match spawn_client(ctx) {
                Ok(sepia) => Slot::Ready(Box::new(sepia)),
                Err(e) => Slot::Failed(e),
            };
        }
        match self {
            Slot::Ready(sepia) => sepia.call(method, params),
            Slot::Failed(e) => Err(e.clone()),
            Slot::Vacant => unreachable!("slot filled above"),
        }
    }
}

/// The `sepia` executable — see the module docs for resolution order.
fn resolve_bin(ctx: &Map<String, Value>) -> PathBuf {
    for key in ["sepiaBin", "sepia", "bin"] {
        if let Some(path) = ctx.get(key).and_then(Value::as_str) {
            return PathBuf::from(path);
        }
    }
    if let Ok(path) = std::env::var("SEPIA_BIN") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    // A workspace-sibling build (`target/debug/sepia` next to
    // `target/debug/tamias-driver-sepia`) is fresher than anything
    // installed; otherwise `sepia` resolves on PATH at spawn time.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("sepia");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("sepia")
}

fn spawn_client(ctx: &Map<String, Value>) -> Result<Sepia, String> {
    Sepia::spawn(&resolve_bin(ctx))
}

/// Call one `sepia serve` method. On a transport-level failure of a
/// *live* client the process is dropped and the call retried once on
/// a fresh spawn; resolution/spawn failures surface directly.
///
/// # Errors
/// The serve wire's error text, or a spawn/transport failure string.
pub fn call(method: &str, params: Value, ctx: &Map<String, Value>) -> Result<Value, String> {
    let mut slot = CLIENT
        .lock()
        .map_err(|e| format!("sepia client lock poisoned: {e}"))?;
    let was_ready = matches!(*slot, Slot::Ready(_));
    match slot.call(method, params.clone(), ctx) {
        Ok(v) => Ok(v),
        Err(first) if was_ready => {
            // The serve process died mid-call — drop it and retry
            // once on a fresh spawn.
            *slot = Slot::Vacant;
            slot.call(method, params, ctx)
                .map_err(|e| format!("{first}; retry after respawn: {e}"))
        }
        Err(e) => Err(e),
    }
}

/// Spawn `sepia serve` in the background at `serve` start — the
/// plane build (driver discovery, store probes) overlaps the host's
/// `manifest`/`views` handshake instead of landing inside the first
/// data op's timeout budget. Best-effort: failures surface on the
/// first real [`call`] anyway. Params that pin a different `sepia`
/// binary retarget the client on first use.
pub fn warm() {
    std::thread::spawn(|| {
        let ctx = Map::new();
        if let Ok(mut slot) = CLIENT.lock() {
            if matches!(*slot, Slot::Vacant) {
                *slot = match spawn_client(&ctx) {
                    Ok(sepia) => Slot::Ready(Box::new(sepia)),
                    Err(e) => Slot::Failed(e),
                };
            }
        }
    });
}

/// `agent`/`dir` pass-through params the serve methods understand —
/// session ids collide across agent stores, so detail views forward
/// `--param agent=` when the caller supplies it.
pub fn agent_param(ctx: &Map<String, Value>) -> Option<String> {
    ctx.get("agent").and_then(Value::as_str).map(str::to_string)
}

/// `dir` pass-through for `config.list`.
pub fn dir_param(ctx: &Map<String, Value>) -> Option<String> {
    ctx.get("dir").and_then(Value::as_str).map(str::to_string)
}
