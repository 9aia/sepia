//! Test process helpers — build-and-locate workspace binaries any
//! crate's tests can spawn (`CARGO_BIN_EXE` only covers the owning
//! package).

use std::path::PathBuf;
use std::sync::OnceLock;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn target_debug() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace_root().join("target"), PathBuf::from)
        .join("debug")
}

/// `TESTKIT_BIN_DIRS` — `:`-separated dirs searched before the default
/// target dir. Cross-repo consumers (peregrin) point it at both their
/// own `target/debug` and sepia's, so testkit binaries resolve no
/// matter which workspace built them.
fn find_bin(name: &str) -> Option<PathBuf> {
    if let Some(dirs) = std::env::var_os("TESTKIT_BIN_DIRS") {
        for dir in std::env::split_paths(&dirs) {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    let candidate = target_debug().join(name);
    candidate.exists().then_some(candidate)
}

fn build_bin(name: &str) -> PathBuf {
    if let Some(found) = std::env::var_os("TESTKIT_BIN_DIRS").and_then(|_| find_bin(name)) {
        return found;
    }
    let root = workspace_root();
    let binary = target_debug().join(name);
    // Prebuilt mode: `cargo xtask test`/`e2e` build the binaries once
    // up front — spawning `cargo` inside a test process serializes on
    // the workspace+global package locks and can wedge nextest behind
    // other projects' builds (observed: 120s test timeouts on
    // "Blocking waiting for file lock on package cache").
    let prebuilt = std::env::var_os("SEPIA_TESTKIT_PREBUILT").is_some()
        || std::env::var_os("SEPIA_E2E_PREBUILT").is_some();
    if !prebuilt {
        let status = std::process::Command::new("cargo")
            .args(["build", "--bin", name])
            .current_dir(&root)
            .status();
        assert!(
            status.is_ok_and(|s| s.success()),
            "failed to build test binary {name}"
        );
    }
    assert!(
        binary.exists(),
        "missing test binary {name} at {} — build it or unset SEPIA_TESTKIT_PREBUILT",
        binary.display()
    );
    binary
}

/// The mock ACP agent binary (sepia-acp's `sepia-mock-acp-agent`) —
/// builds it on first call, then returns the cached path.
pub fn ensure_mock_acp_agent() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| build_bin("sepia-mock-acp-agent"))
}

/// Any workspace driver binary — builds it on first call, cached per
/// name.
pub fn ensure_driver_bin(name: &str) -> PathBuf {
    build_bin(name)
}
