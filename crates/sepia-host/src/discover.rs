//! Driver discovery — scan driver directories and PATH for
//! `sepia-driver-*` binaries, then probe each with `--manifest`.

use std::path::{Path, PathBuf};

use sepia_driver::manifest::DriverManifest;
use tokio::process::Command;

/// Directory search order — first hit wins on name collision.
/// `$SEPIA_DRIVER_DIR` → `<platform data dir>/sepia/drivers` →
/// `/usr/local/lib/sepia/drivers` → PATH scan.
///
/// The platform data dir honors `XDG_DATA_HOME` on unix-likes
/// (`$HOME/.local/share` fallback) — no hardcoded XDG paths so
/// win/mac can land their own resolution later.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("SEPIA_DRIVER_DIR") {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }
    if let Some(data) = data_dir() {
        dirs.push(data.join("sepia/drivers"));
    }
    dirs.push(PathBuf::from("/usr/local/lib/sepia/drivers"));
    dirs
}

fn data_dir() -> Option<PathBuf> {
    data_dir_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

/// The env-free half of [`data_dir`] — tests inject the vars.
fn data_dir_from(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(xdg) = xdg {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg));
        }
    }
    home.map(|home| Path::new(&home).join(".local/share"))
}

fn is_driver_binary(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !name.starts_with("sepia-driver-") {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Find every `sepia-driver-*` binary reachable from the search dirs,
/// deduplicated by file name (first dir wins).
/// Scan one directory for `sepia-driver-*` binaries (first hit wins on
/// name collision, tracked by `seen`).
fn scan_dir(dir: &Path, seen: &mut std::collections::BTreeSet<String>, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_driver_binary(&path) && seen.insert(name.to_string()) {
            found.push(path);
        }
    }
}

/// The env-free half of [`find_driver_binaries`] — tests inject the dirs.
pub fn find_driver_binaries_in(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    let mut found = Vec::new();
    for dir in dirs {
        scan_dir(dir, &mut seen, &mut found);
    }
    found
}

/// Find every `sepia-driver-*` binary reachable from the search dirs,
/// deduplicated by file name (first dir wins).
pub fn find_driver_binaries() -> Vec<PathBuf> {
    let mut found = find_driver_binaries_in(&search_dirs());
    // PATH scan — a driver installed with a package manager lands here.
    if let Ok(path_var) = std::env::var("PATH") {
        let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
        let mut seen: std::collections::BTreeSet<String> = found
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        for dir in &dirs {
            scan_dir(dir, &mut seen, &mut found);
        }
    }
    found
}

/// Run `binary --manifest` and parse the printed [`DriverManifest`].
pub async fn probe_manifest(binary: &Path) -> Result<DriverManifest, String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Command::new(binary).arg("--manifest").output(),
    )
    .await
    .map_err(|_| format!("{} --manifest timed out", binary.display()))?
    .map_err(|e| format!("{} --manifest failed: {e}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} --manifest exited {}: {}",
            binary.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let manifest: DriverManifest = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("{} --manifest printed invalid JSON: {e}", binary.display()))?;
    manifest.validate()?;
    Ok(manifest)
}
