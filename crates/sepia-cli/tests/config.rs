#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `sepia config *` verbs against tempfile agent-config dirs — drives the
//! compiled binary so clap parsing, dispatch and the four drivers' config
//! adapters are all what gets asserted.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

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

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "expected success, got {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn fails(out: &Output) -> String {
    assert!(
        !out.status.success(),
        "expected failure, got {:?}\nstdout: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A config IR file exercising every config kind.
fn write_config_file(tmp: &TempDir) -> String {
    let file = tmp.path().join("config.json");
    std::fs::write(
        &file,
        r#"{
            "rules": [{"name": "house", "kind": "rule", "body": "be nice"}],
            "commands": [{"name": "ship", "body": "deploy it"}],
            "skills": [{"name": "tidy", "description": "clean code", "body": "fmt"}],
            "hooks": [
                {"event": "PreToolUse", "type": "command", "command": "lint.sh", "matcher": "Write"},
                {"event": "Stop", "type": "prompt", "prompt": "review?"}
            ],
            "agents": [{"name": "scout", "body": "look around"}],
            "mcpServers": {"docs": {"command": "mcp-docs"}}
        }"#,
    )
    .unwrap();
    path(&file)
}

// --- list / export ------------------------------------------------------------

#[test]
fn config_list_reads_every_agent_dir() {
    let tmp = TempDir::new().unwrap();
    for (agent, flag) in [
        ("claude", "--claude-dir"),
        ("cursor", "--cursor-dir"),
        ("cline", "--cline-dir"),
        ("devin", "--devin-dir"),
    ] {
        let dir = tmp.path().join(agent);
        std::fs::create_dir_all(&dir).unwrap();
        let out = sepia(&["config", "list", "--from", agent, flag, &path(&dir)]);
        let stdout = ok(&out);
        assert!(stdout.contains(&format!("# {agent} config")), "{stdout}");
        assert!(stdout.contains("(empty config)"), "{stdout}");
    }
}

#[test]
fn config_list_dir_flag_alone_resolves_the_agent() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cursor");
    std::fs::create_dir_all(&dir).unwrap();
    // No --from: the lone --*-dir names its agent.
    let out = sepia(&["config", "list", "--cursor-dir", &path(&dir)]);
    assert!(ok(&out).contains("# cursor config"));
}

#[test]
fn config_list_defaults_to_claude_under_home() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // No flags at all → --from defaults to claude, dir to ~/.claude.
    let out = sepia_env(&["config", "list"], &[("HOME", &path(&home))]);
    assert!(ok(&out).contains("# claude config"));
}

#[test]
fn config_export_stdout_and_file() {
    let tmp = TempDir::new().unwrap();
    let devin = tmp.path().join("devin");
    std::fs::create_dir_all(&devin).unwrap();
    let src = write_config_file(&tmp);
    ok(&sepia(&[
        "config",
        "import",
        &src,
        "--to",
        "devin",
        "--devin-dir",
        &path(&devin),
    ]));

    // stdout (no out) and "-" both print the IR.
    for args in [
        vec![
            "config",
            "export",
            "--from",
            "devin",
            "--devin-dir",
            &path(&devin),
        ],
        vec![
            "config",
            "export",
            "-",
            "--from",
            "devin",
            "--devin-dir",
            &path(&devin),
        ],
    ] {
        let stdout = ok(&sepia(&args));
        let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(parsed["rules"][0]["name"], "house", "{stdout}");
    }

    let out_file = tmp.path().join("exported.json");
    let stdout = ok(&sepia(&[
        "config",
        "export",
        &path(&out_file),
        "--from",
        "devin",
        "--devin-dir",
        &path(&devin),
    ]));
    assert!(stdout.contains("Exported devin config"), "{stdout}");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out_file).unwrap()).unwrap();
    assert_eq!(parsed["commands"][0]["name"], "ship");
}

#[test]
fn config_list_prints_the_full_summary() {
    let tmp = TempDir::new().unwrap();
    let claude = tmp.path().join("claude");
    std::fs::create_dir_all(&claude).unwrap();
    let src = write_config_file(&tmp);
    ok(&sepia(&[
        "config",
        "import",
        &src,
        "--to",
        "claude",
        "--claude-dir",
        &path(&claude),
    ]));
    let stdout = ok(&sepia(&[
        "config",
        "list",
        "--from",
        "claude",
        "--claude-dir",
        &path(&claude),
    ]));
    for needle in [
        "Rules (1):",
        // Claude has no rules dir — the import collapsed the named rule
        // into the managed `sepia:rules` block of CLAUDE.md, which reads
        // back as the single instructions rule named "claude".
        "  claude",
        "Commands (1):",
        "Skills (1):",
        "Subagents (1):",
        "Hooks (2):",
        "PreToolUse ~ Write → lint.sh",
        "Stop → [prompt]",
        "MCP servers (1): docs",
    ] {
        assert!(stdout.contains(needle), "missing {needle}:\n{stdout}");
    }
}

// --- import / install ----------------------------------------------------------

#[test]
fn config_import_reports_per_file_actions() {
    let tmp = TempDir::new().unwrap();
    let cursor = tmp.path().join("cursor");
    std::fs::create_dir_all(&cursor).unwrap();
    let src = write_config_file(&tmp);
    let stdout = ok(&sepia(&[
        "config",
        "import",
        &src,
        "--to",
        "cursor",
        "--cursor-dir",
        &path(&cursor),
    ]));
    assert!(
        stdout.contains("file(s) written into the cursor config"),
        "{stdout}"
    );
    // Second import is idempotent — files come back unchanged.
    let stdout = ok(&sepia(&[
        "config",
        "import",
        &src,
        "--to",
        "cursor",
        "--cursor-dir",
        &path(&cursor),
    ]));
    assert!(stdout.contains("unchanged"), "{stdout}");
}

#[test]
fn config_import_errors() {
    let tmp = TempDir::new().unwrap();
    let cline = tmp.path().join("cline");
    std::fs::create_dir_all(&cline).unwrap();
    // Missing file.
    let out = sepia(&[
        "config",
        "import",
        &path(&tmp.path().join("nope.json")),
        "--to",
        "cline",
        "--cline-dir",
        &path(&cline),
    ]);
    assert!(fails(&out).contains("Config source not found"));
    // Not JSON.
    let junk = tmp.path().join("junk.txt");
    std::fs::write(&junk, "not json").unwrap();
    let out = sepia(&[
        "config",
        "import",
        &path(&junk),
        "--to",
        "cline",
        "--cline-dir",
        &path(&cline),
    ]);
    assert!(fails(&out).contains("not a config IR JSON"));
    // JSON, but not the config IR shape.
    let bad = tmp.path().join("bad.json");
    std::fs::write(&bad, "{\"rules\": \"oops\"}").unwrap();
    let out = sepia(&[
        "config",
        "import",
        &path(&bad),
        "--to",
        "cline",
        "--cline-dir",
        &path(&cline),
    ]);
    assert!(fails(&out).contains("not a config IR JSON"));
}

#[test]
fn config_install_copies_between_agents() {
    let tmp = TempDir::new().unwrap();
    let claude = tmp.path().join("claude");
    let devin = tmp.path().join("devin");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(&devin).unwrap();
    let src = write_config_file(&tmp);
    ok(&sepia(&[
        "config",
        "import",
        &src,
        "--to",
        "claude",
        "--claude-dir",
        &path(&claude),
    ]));
    let stdout = ok(&sepia(&[
        "config",
        "install",
        "--from",
        "claude",
        "--to",
        "devin",
        "--claude-dir",
        &path(&claude),
        "--devin-dir",
        &path(&devin),
    ]));
    assert!(
        stdout.contains("Installed claude config into devin"),
        "{stdout}"
    );
    let stdout = ok(&sepia(&[
        "config",
        "list",
        "--from",
        "devin",
        "--devin-dir",
        &path(&devin),
    ]));
    // Claude's collapsed rule installs into devin's AGENTS.md managed
    // block, which reads back as the instructions rule "agents".
    assert!(stdout.contains("  agents"), "{stdout}");
}

// --- diff -----------------------------------------------------------------------

#[test]
fn config_diff_reports_every_kind() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a-claude");
    let b = tmp.path().join("b-cursor");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    // Source holds the full config plus an mcp server the target lacks.
    let src = tmp.path().join("src.json");
    std::fs::write(
        &src,
        r#"{
            "rules": [{"name": "house", "kind": "rule", "body": "be nice"}],
            "commands": [{"name": "ship", "body": "deploy it"}],
            "skills": [{"name": "tidy", "description": "clean code", "body": "fmt"}],
            "hooks": [{"event": "PreToolUse", "type": "command", "command": "lint.sh"}],
            "agents": [{"name": "scout", "body": "look around"}],
            "mcpServers": {"docs": {"command": "mcp-docs"}, "solo": {"command": "mcp-solo"}}
        }"#,
    )
    .unwrap();
    let src = path(&src);
    ok(&sepia(&[
        "config",
        "import",
        &src,
        "--to",
        "claude",
        "--claude-dir",
        &path(&a),
    ]));
    // Target holds a same-named-but-different rule + its own extras.
    // Claude reads its CLAUDE.md back as the instructions rule "claude",
    // so the target names its rule "claude" to land on the changed arm.
    let other = tmp.path().join("other.json");
    std::fs::write(
        &other,
        r#"{
            "rules": [{"name": "claude", "kind": "rule", "body": "different"}],
            "hooks": [{"event": "PostToolUse", "type": "command", "command": "x.sh"}],
            "mcpServers": {"extra": {"command": "mcp-extra"}, "docs": {"command": "other-docs"}}
        }"#,
    )
    .unwrap();
    ok(&sepia(&[
        "config",
        "import",
        &path(&other),
        "--to",
        "cursor",
        "--cursor-dir",
        &path(&b),
    ]));

    let stdout = ok(&sepia(&[
        "config",
        "diff",
        "--from",
        "claude",
        "--to",
        "cursor",
        "--claude-dir",
        &path(&a),
        "--cursor-dir",
        &path(&b),
    ]));
    for needle in [
        "# claude → cursor",
        "rule changed: claude",
        "command only in source: ship",
        "skill only in source: tidy",
        "subagent only in source: scout",
        "hooks only in source:",
        "hooks only in target:",
        "mcp server only in source: solo",
        "mcp server changed: docs",
        "mcp server only in target: extra",
    ] {
        assert!(stdout.contains(needle), "missing {needle}:\n{stdout}");
    }

    // Identical pair → "configs equivalent".
    let stdout = ok(&sepia(&[
        "config",
        "diff",
        "--from",
        "claude",
        "--to",
        "claude",
        "--claude-dir",
        &path(&a),
    ]));
    assert!(stdout.contains("configs equivalent"), "{stdout}");
}

#[test]
fn config_import_default_target_is_cursor() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let src = write_config_file(&tmp);
    // No --to and no dir flags → the cursor default (~/.cursor under HOME).
    let stdout = ok(&sepia_env(
        &["config", "import", &src],
        &[("HOME", &path(&home))],
    ));
    assert!(stdout.contains("cursor config"), "{stdout}");
    assert!(home.join(".cursor").is_dir());
}
