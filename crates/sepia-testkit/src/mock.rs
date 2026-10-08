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

fn build_bin(name: &str) -> PathBuf {
    let root = workspace_root();
    let status = std::process::Command::new("cargo")
        .args(["build", "--bin", name])
        .current_dir(&root)
        .status();
    let binary = target_debug().join(name);
    assert!(
        status.is_ok_and(|s| s.success()) && binary.exists(),
        "failed to build test binary {name}"
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
