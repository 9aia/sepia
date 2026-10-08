//! Top-level CLI surface — version, help, argument validation.

#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::process::{Command, Output};

fn sepia(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sepia"))
        .args(args)
        .output()
        .unwrap()
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
        "sessions", "projects", "config", "servers", "push", "store", "list", "export", "import",
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
        "run",
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
