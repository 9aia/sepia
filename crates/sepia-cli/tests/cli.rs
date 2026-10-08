//! Top-level CLI surface — version, help, argument validation.

#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::process::{Command, Output};

fn sepia(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sepia"))
        .args(args)
        .output()
        .unwrap()
}

fn sepia_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sepia"));
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

#[test]
fn version_prints_the_package_stamp() {
    let out = sepia(&["version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn help_lists_the_verb_surface() {
    let out = sepia(&["--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for verb in [
        "sessions", "projects", "config", "driver", "push", "store", "list", "export", "import",
        "install", "delete", "pair", "serve", "service", "version", "prompt", "health", "node",
        "agents", "user", "fs", "events", "redeem",
    ] {
        assert!(stdout.contains(verb), "missing {verb} in --help:\n{stdout}");
    }
}

#[test]
fn invalid_store_choice_is_a_usage_error() {
    let out = sepia(&["list", "--from", "bogus"]);
    assert_eq!(out.status.code(), Some(2)); // clap usage error
    assert!(String::from_utf8_lossy(&out.stderr).contains("bogus"));
}

#[test]
fn node_verb_without_a_node_fails_cleanly() {
    // Nothing listens on this port — the verb must error + exit 1, not hang.
    let out = sepia(&["health", "--node", "http://127.0.0.1:1"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Cannot reach"));
}

#[test]
fn sessions_subgroup_lists_verbs() {
    let out = sepia(&["sessions", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for verb in [
        "list",
        "create",
        "attach",
        "prompt",
        "cancel",
        "permission",
        "history",
        "checkpoints",
        "export",
        "stream",
        "meta",
        "rename",
        "delete",
        "convert",
        "import",
        "resume",
        "restore",
        "rewind",
    ] {
        assert!(stdout.contains(verb), "missing sessions {verb}:\n{stdout}");
    }
}

/// A discovered driver binary is probed via `--manifest` — a fake
/// executable printing manifest JSON must show up as a row with its
/// version, capabilities and unresolved agent command.
#[cfg(unix)]
#[test]
fn driver_list_shows_a_discovered_driver() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().unwrap();
    let bin = tmp.path().join("sepia-driver-fake");
    std::fs::write(
        &bin,
        "#!/bin/sh\necho '{\"id\":\"fake\",\"label\":\"Fake\",\"version\":\"0.0.1\",\"protocol\":1,\"capabilities\":[\"sessionStore\"],\"agentCommand\":\"sepia-test-nonexistent-agent\"}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dir = tmp.path().to_string_lossy().into_owned();

    for args in [["driver", "list"].as_slice(), ["driver"].as_slice()] {
        let out = sepia_env(args, &[("SEPIA_DRIVER_DIR", &dir)]);
        assert!(
            out.status.success(),
            "sepia {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let row = stdout
            .lines()
            .find(|l| l.starts_with("fake\t"))
            .unwrap_or_else(|| panic!("no fake driver row in:\n{stdout}"));
        assert!(row.contains(bin.to_string_lossy().as_ref()), "{row}");
        assert!(row.contains("0.0.1"), "{row}");
        assert!(row.contains("sessionStore"), "{row}");
        assert!(row.contains("missing"), "{row}");
    }
}
