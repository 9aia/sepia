//! Driver discovery — scan driver directories and PATH for
//! `sepia-driver-*` binaries, then probe each with `--manifest`.

use std::path::{Path, PathBuf};

use sepia_driver_sdk::manifest::DriverManifest;
use tokio::process::Command;

/// Directory search order — first hit wins on name collision.
/// `$SEPIA_DRIVER_DIR` → `~/.local/share/sepia/drivers` →
/// `/usr/local/lib/sepia/drivers` → PATH scan.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("SEPIA_DRIVER_DIR") {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/share/sepia/drivers"));
    }
    dirs.push(PathBuf::from("/usr/local/lib/sepia/drivers"));
    dirs
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
pub fn find_driver_binaries() -> Vec<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    let mut found = Vec::new();
    for dir in search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
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
    // PATH scan — a driver installed with a package manager lands here.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
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
