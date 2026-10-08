//! `sepia service` — the rendered unit/plist templates and the env file
//! contents, asserted without touching `systemctl`/`launchctl`.

#![allow(clippy::unwrap_used, clippy::pedantic)]
#![cfg(unix)]

use std::path::PathBuf;

use sepia_cli::service::{ServiceSpec, default_env_file, env_template, launchd, systemd};
use tempfile::TempDir;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn spec(system: bool, env_file: PathBuf) -> ServiceSpec {
    ServiceSpec {
        name: "sepia".into(),
        exec: vec!["/usr/local/bin/sepia".into(), "serve".into()],
        env_file,
        system,
    }
}

// --- systemd ------------------------------------------------------------------

#[test]
fn systemd_user_unit_uses_h_specifier() {
    let spec = spec(false, home().join(".config/sepia/env"));
    let unit = systemd::render(&spec);
    assert!(unit.contains("Description=Sepia node"), "{unit}");
    assert!(
        unit.contains("ExecStart=/usr/local/bin/sepia serve"),
        "{unit}"
    );
    // A user unit under $HOME renders the env file as %h/…
    assert!(
        unit.contains("EnvironmentFile=-%h/.config/sepia/env"),
        "{unit}"
    );
    assert!(unit.contains("Restart=on-failure"), "{unit}");
    assert!(unit.contains("WantedBy=default.target"), "{unit}");
}

#[test]
fn systemd_system_unit_keeps_a_literal_env_path() {
    let spec = spec(true, PathBuf::from("/etc/sepia/env"));
    let unit = systemd::render(&spec);
    assert!(unit.contains("EnvironmentFile=-/etc/sepia/env"), "{unit}");
    assert!(unit.contains("WantedBy=multi-user.target"), "{unit}");
}

#[test]
fn systemd_env_file_outside_home_stays_literal_in_user_units() {
    let spec = spec(false, PathBuf::from("/opt/sepia/env"));
    let unit = systemd::render(&spec);
    assert!(unit.contains("EnvironmentFile=-/opt/sepia/env"), "{unit}");
}

#[test]
fn systemd_quotes_exec_args_with_spaces() {
    let mut spec = spec(false, PathBuf::from("/tmp/env"));
    spec.exec = vec!["/path with space/sepia".into(), "serve".into()];
    let unit = systemd::render(&spec);
    assert!(
        unit.contains("ExecStart=\"/path with space/sepia\" serve"),
        "{unit}"
    );
}

#[test]
fn systemd_unit_paths() {
    let user_spec = spec(false, PathBuf::from("/tmp/env"));
    assert_eq!(
        systemd::unit_path(&user_spec),
        home().join(".config/systemd/user/sepia.service")
    );
    let system_spec = spec(true, PathBuf::from("/tmp/env"));
    assert_eq!(
        systemd::unit_path(&system_spec),
        PathBuf::from("/etc/systemd/system/sepia.service")
    );
}

// --- launchd --------------------------------------------------------------------

#[test]
fn launchd_plist_renders_label_args_and_log_paths() {
    let tmp = TempDir::new().unwrap();
    let spec = spec(false, tmp.path().join("env"));
    let plist = launchd::render(&spec);
    assert!(plist.contains("<key>Label</key>"), "{plist}");
    assert!(plist.contains("<string>ai.sepia</string>"), "{plist}");
    assert!(plist.contains("<key>ProgramArguments</key>"), "{plist}");
    assert!(
        plist.contains("<string>/usr/local/bin/sepia</string>"),
        "{plist}"
    );
    assert!(plist.contains("<string>serve</string>"), "{plist}");
    assert!(plist.contains("<key>RunAtLoad</key>"), "{plist}");
    assert!(plist.contains("<key>StandardOutPath</key>"), "{plist}");
    // No env file → no EnvironmentVariables dict.
    assert!(!plist.contains("EnvironmentVariables"), "{plist}");
}

#[test]
fn launchd_renders_env_file_into_the_plist() {
    let tmp = TempDir::new().unwrap();
    let env_file = tmp.path().join("env");
    std::fs::write(
        &env_file,
        "# comment\nSEPIA_TOKEN=abc123\nSEPIA_HOST=0.0.0.0\nBROKEN LINE\n=novalue\n",
    )
    .unwrap();
    let spec = spec(false, env_file);
    let plist = launchd::render(&spec);
    assert!(plist.contains("<key>EnvironmentVariables</key>"), "{plist}");
    assert!(plist.contains("<key>SEPIA_TOKEN</key>"), "{plist}");
    assert!(plist.contains("<string>abc123</string>"), "{plist}");
    assert!(plist.contains("<key>SEPIA_HOST</key>"), "{plist}");
    // Invalid lines are skipped, not emitted.
    assert!(!plist.contains("BROKEN"), "{plist}");
}

#[test]
fn launchd_escapes_xml_in_values() {
    let tmp = TempDir::new().unwrap();
    let env_file = tmp.path().join("env");
    std::fs::write(&env_file, "SEPIA_TOKEN=a&b<c>\n").unwrap();
    let mut spec = spec(false, env_file);
    spec.exec = vec!["/weird&path/sepia".into()];
    let plist = launchd::render(&spec);
    assert!(plist.contains("a&amp;b&lt;c&gt;"), "{plist}");
    assert!(plist.contains("/weird&amp;path/sepia"), "{plist}");
}

#[test]
fn launchd_unit_paths() {
    let user_spec = spec(false, PathBuf::from("/tmp/env"));
    assert_eq!(
        launchd::unit_path(&user_spec),
        home().join("Library/LaunchAgents/ai.sepia.plist")
    );
    let system_spec = spec(true, PathBuf::from("/tmp/env"));
    assert_eq!(
        launchd::unit_path(&system_spec),
        PathBuf::from("/Library/LaunchDaemons/ai.sepia.plist")
    );
}

// --- env template -----------------------------------------------------------------

#[test]
fn env_template_documents_the_knobs() {
    let template = env_template();
    for knob in [
        "SEPIA_TOKEN",
        "SEPIA_HOST",
        "SEPIA_DB",
        "SEPIA_CLINE_DIR",
        "SEPIA_CLAUDE_DIR",
        "SEPIA_CURSOR_DIR",
    ] {
        assert!(template.contains(knob), "missing {knob} in template");
    }
    // Every real line is a comment or KEY=value — nothing uncommented ships.
    for line in template.lines() {
        assert!(
            line.is_empty() || line.starts_with('#'),
            "uncommented line shipped in template: {line}"
        );
    }
}

#[test]
fn env_file_default_is_under_dot_config() {
    assert_eq!(
        default_env_file(&home(), "sepia"),
        home().join(".config/sepia/env")
    );
}
