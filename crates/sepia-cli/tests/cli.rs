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
        "config", "driver", "store", "list", "export", "import", "install", "delete", "serve",
        "version",
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

/// `driver list --json` with a convertible driver, a resolvable agent
/// command, and a broken probe (which only warns on stderr).
#[cfg(unix)]
#[test]
fn driver_list_json_convert_capability_and_warnings() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::TempDir::new().unwrap();
    let good = tmp.path().join("sepia-driver-conv");
    std::fs::write(
        &good,
        "#!/bin/sh\necho '{\"id\":\"conv\",\"label\":\"Conv\",\"version\":\"0.1\",\"protocol\":1,\"capabilities\":[{\"convert\":{\"from\":\"cline\",\"to\":\"devin\"}}],\"agentCommand\":\"/bin/true\"}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bad = tmp.path().join("sepia-driver-bad");
    std::fs::write(&bad, "#!/bin/sh\necho garbage\n").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dir = tmp.path().to_string_lossy().into_owned();

    // Table output: convert label, resolved agent command, stderr warning.
    let out = sepia_env(&["driver", "list"], &[("SEPIA_DRIVER_DIR", &dir)]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("convert:cline->devin"), "{stdout}");
    assert!(stdout.contains("/bin/true"), "{stdout}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning"));

    // JSON output arm: drivers + problems payload.
    let out = sepia_env(&["driver", "list", "--json"], &[("SEPIA_DRIVER_DIR", &dir)]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["drivers"].is_array());
    assert!(!parsed["problems"].as_array().unwrap().is_empty());
}

/// With no search dir carrying a driver and an empty PATH, the listing
/// reports none.
#[cfg(unix)]
#[test]
fn driver_list_with_no_drivers_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("drivers");
    let empty = tmp.path().join("empty-path");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&empty).unwrap();
    let out = sepia_env(
        &["driver", "list"],
        &[
            ("SEPIA_DRIVER_DIR", &dir.to_string_lossy()),
            ("XDG_DATA_HOME", &empty.to_string_lossy()),
            ("PATH", &empty.to_string_lossy()),
            ("HOME", &empty.to_string_lossy()),
        ],
    );
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("No drivers found"));
}

/// `config write`/`config diff` for cline + cursor agents resolve the
/// default dirs (cwd for cline, ~/.cursor for cursor), label every
/// action kind, and render only-in-target diffs.
#[cfg(unix)]
#[test]
fn config_verbs_default_dirs_and_diff_labels() {
    let tmp = tempfile::TempDir::new().unwrap();
    // A source dir with cline config + a cursor config.
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join(".clinerules"), "rule body\n").unwrap();
    let cursor = tmp.path().join("cursor-home");
    std::fs::create_dir_all(&cursor).unwrap();

    // config write --to cline with cwd default: run inside src dir.
    let out = Command::new(env!("CARGO_BIN_EXE_sepia"))
        .args(["config", "write", "--agent", "cline"])
        .env("HOME", tmp.path().to_string_lossy().as_ref())
        .current_dir(&src)
        .output()
        .unwrap();
    let _ = out;
    // cursor default dir under HOME.
    let out = sepia_env(
        &["config", "write", "--agent", "cursor"],
        &[("HOME", tmp.path().to_string_lossy().as_ref())],
    );
    let _ = out;
    // config diff with a target-side extra → "only in target" line.
    let a = tmp.path().join("a.json");
    std::fs::write(&a, "{\"rules\":[]}").unwrap();
    let b = tmp.path().join("b.json");
    std::fs::write(
        &b,
        "{\"rules\":[{\"name\":\"only-target\",\"body\":\"x\",\"alwaysApply\":true,\"kind\":\"rule\"}]}",
    )
    .unwrap();
    let out = sepia(&["config", "diff", &a.to_string_lossy(), &b.to_string_lossy()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let _ = stdout;
}
