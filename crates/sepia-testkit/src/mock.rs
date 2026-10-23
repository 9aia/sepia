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
/// matter which workspace built them. The global cargo `target-dir`
/// (`$HOME/.cargo/shared-target`) is searched too — config-file
/// `target-dir` never reaches test processes as `CARGO_TARGET_DIR`,
/// so binaries land there invisibly otherwise.
fn find_bin(name: &str) -> Option<PathBuf> {
    find_bin_in(
        name,
        std::env::var_os("TESTKIT_BIN_DIRS"),
        std::env::var_os("HOME"),
        &target_debug(),
    )
}

/// The search, split from env access — tests drive every arm directly.
fn find_bin_in(
    name: &str,
    dirs: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    target: &std::path::Path,
) -> Option<PathBuf> {
    if let Some(dirs) = dirs {
        for dir in std::env::split_paths(&dirs) {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    if let Some(home) = home {
        let candidate = PathBuf::from(home)
            .join(".cargo/shared-target/debug")
            .join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let candidate = target.join(name);
    candidate.exists().then_some(candidate)
}

fn build_bin(name: &str) -> PathBuf {
    if let Some(found) = find_bin(name) {
        return found;
    }
    // Prebuilt mode: `cargo xtask test`/`e2e` build the binaries once
    // up front — spawning `cargo` inside a test process serializes on
    // the workspace+global package locks and can wedge nextest behind
    // other projects' builds (observed: 120s test timeouts on
    // "Blocking waiting for file lock on package cache").
    let prebuilt = std::env::var_os("SEPIA_TESTKIT_PREBUILT").is_some()
        || std::env::var_os("SEPIA_E2E_PREBUILT").is_some();
    build_bin_at(
        name,
        &workspace_root(),
        &target_debug(),
        prebuilt,
        |root, name| {
            std::process::Command::new("cargo")
                .args(["build", "--bin", name])
                .current_dir(root)
                .status()
                .is_ok_and(|s| s.success())
        },
    )
}

/// Build-or-assert split from env/spawn — tests inject the build.
fn build_bin_at(
    name: &str,
    root: &std::path::Path,
    target: &std::path::Path,
    prebuilt: bool,
    build: impl Fn(&std::path::Path, &str) -> bool,
) -> PathBuf {
    let binary = target.join(name);
    if !prebuilt {
        assert!(build(root, name), "failed to build test binary {name}");
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn find_bin_in_checks_each_dir_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("x-bin"), "").unwrap();
        let dirs = std::env::join_paths([&a, &b]).unwrap();
        // Misses dir a, hits dir b.
        assert_eq!(
            find_bin_in("x-bin", Some(dirs.clone()), None, Path::new("/none")),
            Some(b.join("x-bin"))
        );
        // No hit anywhere → None.
        assert!(
            find_bin_in(
                "nope",
                Some(dirs),
                Some(tmp.path().as_os_str().into()),
                Path::new("/none")
            )
            .is_none()
        );
        // HOME fallback: <home>/.cargo/shared-target/debug/<name>.
        let home = tmp.path().join("home");
        let shared = home.join(".cargo/shared-target/debug");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("h-bin"), "").unwrap();
        assert_eq!(
            find_bin_in(
                "h-bin",
                None,
                Some(home.into_os_string()),
                Path::new("/none")
            ),
            Some(shared.join("h-bin"))
        );
        // Target-dir fallback.
        let t = tmp.path().join("target");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("t-bin"), "").unwrap();
        assert_eq!(find_bin_in("t-bin", None, None, &t), Some(t.join("t-bin")));
    }

    #[test]
    fn build_bin_at_builds_or_trusts_prebuilt() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("t");
        std::fs::create_dir_all(&target).unwrap();
        // Build closure "builds" the binary.
        let t = target.clone();
        let built = build_bin_at("x", tmp.path(), &target, false, move |_, name| {
            std::fs::write(t.join(name), "").unwrap();
            true
        });
        assert!(built.exists());
        // Prebuilt skips the build entirely.
        let existing = build_bin_at("x", tmp.path(), &target, true, |_, _| {
            panic!("must not build")
        });
        assert!(existing.exists());
    }

    #[test]
    #[should_panic(expected = "failed to build test binary")]
    fn build_bin_at_fails_when_build_fails() {
        let tmp = tempfile::tempdir().unwrap();
        build_bin_at("gone", tmp.path(), tmp.path(), false, |_, _| false);
    }

    #[test]
    #[should_panic(expected = "missing test binary")]
    fn build_bin_at_fails_when_binary_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        build_bin_at("gone", tmp.path(), tmp.path(), true, |_, _| true);
    }

    #[test]
    fn ensure_helpers_and_roots_run() {
        // Real env under coverage runs has TESTKIT_BIN_DIRS set, so the
        // locators hit there; the roots resolve regardless.
        let _ = workspace_root();
        let _ = target_debug();
        assert!(ensure_mock_acp_agent().exists());
        assert!(ensure_driver_bin("sepia-mock-acp-agent").exists());
    }
}
