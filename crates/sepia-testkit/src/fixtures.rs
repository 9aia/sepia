//! Golden-fixture loading — `fixtures/<agent>/<case>/` dirs carry a
//! `store/` (files verbatim) or `store.sql` (applied via rusqlite) plus
//! `list.json` / `export.<id>.json` expected outputs.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Root of the committed fixtures — resolved via `CARGO_MANIFEST_DIR` so
/// tests work from any crate.
pub fn fixture_dir(agent: &str, case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(agent)
        .join(case)
}

/// Read an expected JSON payload from a fixture case dir.
///
/// # Errors
/// Fails on missing file or invalid JSON — always a fixture bug, so tests
/// should let the error surface.
pub fn load_expected(case_dir: &Path, name: &str) -> std::io::Result<Value> {
    let path = case_dir.join(name);
    let text = std::fs::read_to_string(&path)?;
    serde_json::from_str(&text).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )
    })
}

/// Materialize a fixture store into a scratch dir, returning the path the
/// driver should open:
/// - `store/` → copied verbatim to `<scratch>/store`
/// - `store.sql` → executed into a fresh `<scratch>/store.db` via rusqlite
///
/// # Errors
/// Fails when the case has neither fixture form, or the sql/sqlite fails.
pub fn materialize_store(case_dir: &Path, scratch: &Path) -> std::io::Result<PathBuf> {
    let files_dir = case_dir.join("store");
    if files_dir.is_dir() {
        let target = scratch.join("store");
        copy_dir(&files_dir, &target)?;
        return Ok(target);
    }
    let sql_path = case_dir.join("store.sql");
    if sql_path.exists() {
        let target = scratch.join("store.db");
        apply_sql(&sql_path, &target)?;
        return Ok(target);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{} has no store/ or store.sql fixture", case_dir.display()),
    ))
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn apply_sql(sql_path: &Path, db_path: &Path) -> std::io::Result<()> {
    let sql = std::fs::read_to_string(sql_path)?;
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| std::io::Error::other(format!("open {}: {e}", db_path.display())))?;
    conn.execute_batch(&sql)
        .map_err(|e| std::io::Error::other(format!("apply {}: {e}", sql_path.display())))
}
