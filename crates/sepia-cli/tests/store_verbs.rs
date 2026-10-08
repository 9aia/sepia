//! Store-verb coverage — drives the compiled `sepia` binary against
//! tempfile stores so the clap surface (including the `--*-dir`
//! flags-doubling-as-store-selectors rules) is what gets asserted, not a
//! re-implementation.

#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::path::Path;
use std::process::{Command, Output};

use sepia_core::Session;
use sepia_core::storage::SessionRepository;
use tempfile::TempDir;

fn sepia(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sepia"))
        .args(args)
        .output()
        .unwrap()
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

fn seed_devin(db: &Path, sessions: &[Session]) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let store = sepia_driver_devin::DevinStore::open(db, false).unwrap();
        for session in sessions {
            store.save(session).await.unwrap();
        }
    });
}

/// An empty-but-initialized devin store (schema exists, zero rows).
fn empty_devin(db: &Path) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let _ = sepia_driver_devin::DevinStore::open(db, false).unwrap();
    });
}

fn seed_cline(data_dir: &Path, session: &Session, id: &str) {
    sepia_driver_cline::ClineStore::new(data_dir.to_path_buf())
        .install(session, id, false)
        .unwrap();
}

fn session(id: &str, title: &str) -> Session {
    sepia_testkit::contract::session(id, title, 2.0)
}

// --- list --------------------------------------------------------------------

#[test]
fn list_devin_sessions() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    seed_devin(
        &db,
        &[session("s1", "first session"), session("s2", "second")],
    );

    let out = sepia(&["list", "--db", &path(&db)]);
    let stdout = ok(&out);
    assert!(stdout.contains("s1\tfirst session\t/contract"), "{stdout}");
    assert!(stdout.contains("s2\tsecond\t/contract"), "{stdout}");
}

#[test]
fn list_empty_store_reports_none() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    empty_devin(&db);

    let out = sepia(&["list", "--db", &path(&db)]);
    assert_eq!(ok(&out).trim(), "No sessions found");
}

#[test]
fn list_store_subgroup_matches_top_level() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    seed_devin(&db, &[session("s1", "first session")]);

    let top = ok(&sepia(&["list", "--db", &path(&db)]));
    let grouped = ok(&sepia(&["store", "list", "--db", &path(&db)]));
    assert_eq!(top, grouped);
}

#[test]
fn list_missing_db_fails() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("does-not-exist.db");
    let out = sepia(&["list", "--db", &path(&db)]);
    assert!(fails(&out).contains("Failed to open database"));
}

// --- the `--*-dir` store selectors ---------------------------------------------

#[test]
fn data_dir_alone_selects_the_cline_store() {
    let tmp = TempDir::new().unwrap();
    let cline_dir = tmp.path().join("cline-data");
    seed_cline(&cline_dir, &session("c1", "cline session"), "c1");

    // No --from — the explicit --data-dir names the cline store.
    let out = sepia(&["list", "--data-dir", &path(&cline_dir)]);
    assert!(ok(&out).contains("c1\tcline session"),);
}

#[test]
fn claude_dir_alone_selects_the_claude_store() {
    let tmp = TempDir::new().unwrap();
    let claude_dir = tmp.path().join("claude-data");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let store = sepia_driver_claude::ClaudeStore::new(claude_dir.join("projects"));
        store.save(&session("cl1", "claude session")).await.unwrap();
    });

    let out = sepia(&["list", "--claude-dir", &path(&claude_dir)]);
    assert!(ok(&out).contains("cl1\tclaude session"));
}

#[test]
fn from_flag_beats_dir_selector() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let cline_dir = tmp.path().join("cline-data");
    seed_devin(&db, &[session("d1", "devin session")]);
    seed_cline(&cline_dir, &session("c1", "cline session"), "c1");

    // --from devin + --data-dir: the flag wins, the dir is just a path.
    let out = sepia(&[
        "list",
        "--from",
        "devin",
        "--db",
        &path(&db),
        "--data-dir",
        &path(&cline_dir),
    ]);
    let stdout = ok(&out);
    assert!(stdout.contains("d1\tdevin session"), "{stdout}");
    assert!(!stdout.contains("c1"), "{stdout}");
}

// --- export --------------------------------------------------------------------

#[test]
fn export_json_to_stdout() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    seed_devin(&db, &[session("s1", "first session")]);

    let out = sepia(&["export", "s1", "--db", &path(&db)]);
    let stdout = ok(&out);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["id"], "s1");
    assert_eq!(parsed["title"], "first session");
}

#[test]
fn export_json_to_file_and_dir() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    seed_devin(&db, &[session("s1", "first session")]);

    // A file target gets the JSON verbatim.
    let file = tmp.path().join("out.json");
    let out = sepia(&["export", "s1", &path(&file), "--db", &path(&db)]);
    assert!(ok(&out).contains("Exported session s1"));
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(parsed["id"], "s1");

    // A directory target gets <id>.session.json inside it.
    let dir = tmp.path().join("exports");
    std::fs::create_dir(&dir).unwrap();
    let out = sepia(&["export", "s1", &path(&dir), "--db", &path(&db)]);
    ok(&out);
    assert!(dir.join("s1.session.json").exists());
}

#[test]
fn export_cline_format_writes_session_files() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let out_dir = tmp.path().join("cline-out");
    seed_devin(&db, &[session("s1", "first session")]);

    let out = sepia(&[
        "export",
        "s1",
        &path(&out_dir),
        "--format",
        "cline",
        "--db",
        &path(&db),
    ]);
    ok(&out);
    assert!(out_dir.join("s1.json").exists());
    assert!(out_dir.join("s1.messages.json").exists());
}

#[test]
fn export_cline_format_requires_out_dir() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    seed_devin(&db, &[session("s1", "first session")]);

    let out = sepia(&["export", "s1", "--format", "cline", "--db", &path(&db)]);
    assert!(fails(&out).contains("needs an out directory"));
}

#[test]
fn export_missing_session_fails() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    empty_devin(&db);

    let out = sepia(&["export", "nope", "--db", &path(&db)]);
    assert!(fails(&out).contains("Session not found: nope"));
}

// --- import --------------------------------------------------------------------

fn export_to_disk(db: &Path, id: &str, file: &Path) {
    let out = sepia(&["export", id, &path(file), "--db", &path(db)]);
    ok(&out);
}

#[test]
fn import_session_json_into_devin() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("src.db");
    let dst = tmp.path().join("dst.db");
    let export = tmp.path().join("s1.json");
    seed_devin(&db, &[session("s1", "first session")]);
    export_to_disk(&db, "s1", &export);

    let out = sepia(&["import", &path(&export), "--db", &path(&dst)]);
    assert!(ok(&out).contains("Imported JSON session s1 into storage"));

    let out = sepia(&["list", "--db", &path(&dst)]);
    assert!(ok(&out).contains("s1\tfirst session"));
}

#[test]
fn import_skips_an_existing_devin_session() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let export = tmp.path().join("s1.json");
    seed_devin(&db, &[session("s1", "first session")]);
    export_to_disk(&db, "s1", &export);

    // Re-importing the same id leaves it untouched (idempotent).
    let out = sepia(&["import", &path(&export), "--db", &path(&db)]);
    assert!(ok(&out).contains("already imported"));
}

#[test]
fn import_session_id_override_rekeys() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("src.db");
    let dst = tmp.path().join("dst.db");
    let export = tmp.path().join("s1.json");
    seed_devin(&db, &[session("s1", "first session")]);
    export_to_disk(&db, "s1", &export);

    let out = sepia(&[
        "import",
        &path(&export),
        "--session-id",
        "renamed",
        "--db",
        &path(&dst),
    ]);
    ok(&out);
    let out = sepia(&["list", "--db", &path(&dst)]);
    assert!(ok(&out).contains("renamed\tfirst session"));
}

#[test]
fn import_cline_dir_into_devin() {
    let tmp = TempDir::new().unwrap();
    let cline_data = tmp.path().join("cline-data");
    let db = tmp.path().join("sessions.db");
    seed_cline(&cline_data, &session("c1", "cline session"), "c1");
    // ClineStore lays sessions out under <data>/sessions/<id>/.
    let session_dir = cline_data.join("sessions").join("c1");
    assert!(session_dir.is_dir());

    let out = sepia(&["import", &path(&session_dir), "--db", &path(&db)]);
    assert!(ok(&out).contains("Imported Cline session c1 into storage"));
}

#[test]
fn import_into_claude_via_to_flag() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("src.db");
    let claude_dir = tmp.path().join("claude-data");
    let export = tmp.path().join("s1.json");
    seed_devin(&db, &[session("s1", "first session")]);
    export_to_disk(&db, "s1", &export);

    let out = sepia(&[
        "import",
        &path(&export),
        "--to",
        "claude",
        "--claude-dir",
        &path(&claude_dir),
        "--db",
        &path(&db),
    ]);
    ok(&out);
    let out = sepia(&["list", "--claude-dir", &path(&claude_dir)]);
    assert!(ok(&out).contains("s1\tfirst session"));
}

#[test]
fn import_refuses_existing_target_without_force() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("src.db");
    let claude_dir = tmp.path().join("claude-data");
    let export = tmp.path().join("s1.json");
    seed_devin(&db, &[session("s1", "first session")]);
    export_to_disk(&db, "s1", &export);

    let args = [
        "import",
        &path(&export),
        "--to",
        "claude",
        "--claude-dir",
        &path(&claude_dir),
    ];
    ok(&sepia(&args));
    // Second import into claude refuses — save rewrites in place.
    assert!(fails(&sepia(&args)).contains("already exists"));
    // --force overwrites.
    let mut forced = args.to_vec();
    forced.push("--force");
    ok(&sepia(&forced));
}

#[test]
fn import_rejects_a_garbage_source() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let junk = tmp.path().join("junk.txt");
    std::fs::write(&junk, "not a session").unwrap();

    let out = sepia(&["import", &path(&junk), "--db", &path(&db)]);
    assert!(fails(&out).contains("neither a Cline dir"));
}

// --- install --------------------------------------------------------------------

#[test]
fn install_devin_into_cline_default() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let cline_dir = tmp.path().join("cline-data");
    seed_devin(&db, &[session("s1", "first session")]);

    // --data-dir both selects cline as the target and points at it.
    let out = sepia(&[
        "install",
        "s1",
        "--db",
        &path(&db),
        "--data-dir",
        &path(&cline_dir),
    ]);
    let stdout = ok(&out);
    assert!(stdout.contains("Installed session"), "{stdout}");
    assert!(stdout.contains("Resume it with: cline --id"), "{stdout}");
    assert!(cline_dir.join("sessions").is_dir());
}

#[test]
fn install_devin_into_claude_with_dirs_claiming() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let claude_dir = tmp.path().join("claude-data");
    let cline_dir = tmp.path().join("cline-data");
    seed_devin(&db, &[session("s1", "first session")]);
    seed_cline(&cline_dir, &session("c1", "cline session"), "c1");

    // --to claude + --claude-dir claims claude, so --data-dir resolves
    // the source to cline rather than devin: install c1 into claude.
    let out = sepia(&[
        "install",
        "c1",
        "--to",
        "claude",
        "--claude-dir",
        &path(&claude_dir),
        "--data-dir",
        &path(&cline_dir),
        "--db",
        &path(&db),
    ]);
    assert!(ok(&out).contains("Installed session c1 into the claude store"));
    let out = sepia(&["list", "--claude-dir", &path(&claude_dir)]);
    assert!(ok(&out).contains("c1\tcline session"));
}

#[test]
fn install_cline_into_devin() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let cline_dir = tmp.path().join("cline-data");
    empty_devin(&db);
    seed_cline(&cline_dir, &session("c1", "cline session"), "c1");

    let out = sepia(&[
        "install",
        "c1",
        "--to",
        "devin",
        "--db",
        &path(&db),
        "--data-dir",
        &path(&cline_dir),
    ]);
    // --to devin claims devin; --data-dir resolves the source to cline.
    assert!(ok(&out).contains("Installed session c1 into the devin store"));
    let out = sepia(&["list", "--db", &path(&db)]);
    assert!(ok(&out).contains("c1\tcline session"));
}

#[test]
fn install_explicit_id() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    let cline_dir = tmp.path().join("cline-data");
    seed_devin(&db, &[session("s1", "first session")]);

    let out = sepia(&[
        "install",
        "s1",
        "--db",
        &path(&db),
        "--data-dir",
        &path(&cline_dir),
        "--id",
        "chosen-id",
    ]);
    assert!(ok(&out).contains("Installed session chosen-id"));
    assert!(cline_dir.join("sessions").join("chosen-id").is_dir());
}

// --- delete ---------------------------------------------------------------------

#[test]
fn delete_devin_session() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sessions.db");
    seed_devin(&db, &[session("s1", "first session")]);

    let out = sepia(&["delete", "s1", "--db", &path(&db)]);
    assert!(ok(&out).contains("Deleted session s1 from the devin store"));

    let out = sepia(&["list", "--db", &path(&db)]);
    assert_eq!(ok(&out).trim(), "No sessions found");
}

#[test]
fn delete_cline_session_via_dir_selector() {
    let tmp = TempDir::new().unwrap();
    let cline_dir = tmp.path().join("cline-data");
    seed_cline(&cline_dir, &session("c1", "cline session"), "c1");

    let out = sepia(&["delete", "c1", "--data-dir", &path(&cline_dir)]);
    assert!(ok(&out).contains("Deleted session c1 from the cline store"));

    let out = sepia(&["list", "--data-dir", &path(&cline_dir)]);
    assert_eq!(ok(&out).trim(), "No sessions found");
}
