//! The real disk/git seam for `restore` — plain fs plus the workspace's
//! own `git` binary (checkpoint refs live in its object store).

use std::path::Path;

use crate::types::{GitResult, RestoreExec};

// Checkpoint blobs can be whole workspaces of file content — a bounded
// buffer keeps a pathological ref from growing the process without limit.
const GIT_MAX_OUTPUT: usize = 64 * 1024 * 1024;

pub struct DefaultRestoreExec;

#[async_trait::async_trait]
impl RestoreExec for DefaultRestoreExec {
    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, String> {
        // Any failure reads as "absent": ENOENT is the common case; an
        // unreadable file can't have its after-state verified anyway.
        Ok(tokio::fs::read(path).await.ok())
    }

    async fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::write(path, content)
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_file(&self, path: &Path) -> Result<(), String> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn git(&self, cwd: &Path, args: &[&str]) -> Result<GitResult, String> {
        let output = tokio::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        // A non-zero exit is git answering "no" — reported via `code`.
        Ok(GitResult {
            code: output.status.code().unwrap_or(-1),
            stdout: truncate(&output.stdout, GIT_MAX_OUTPUT),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

fn truncate(bytes: &[u8], max: usize) -> Vec<u8> {
    if bytes.len() <= max {
        bytes.to_vec()
    } else {
        bytes[..max].to_vec()
    }
}
