//! The real disk/git seam for `restore` — plain fs plus the workspace's
//! own `git` binary (checkpoint refs live in its object store).

use std::path::Path;

use crate::types::{GitResult, RestoreExec};

// Checkpoint blobs can be whole workspaces of file content — a bounded
// buffer keeps a pathological ref from growing the process without limit.
const GIT_MAX_OUTPUT: usize = 64 * 1024 * 1024;

pub struct DefaultRestoreExec {
    /// The git binary invoked for checkpoint work — test seam for the
    /// spawn-failure arm.
    #[doc(hidden)]
    pub git: &'static str,
}

impl Default for DefaultRestoreExec {
    fn default() -> Self {
        Self { git: "git" }
    }
}

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
        let output = tokio::process::Command::new(self.git)
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[tokio::test]
    async fn exec_error_arms_and_truncate_edge() {
        let exec = DefaultRestoreExec {
            git: "/nonexistent/git",
        };
        // A missing git binary → the spawn map_err arm.
        let err = exec.git(Path::new("/"), &["status"]).await.unwrap_err();
        assert!(!err.is_empty());
        // Write under a path that is a file → create_dir_all fails.
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a-file");
        tokio::fs::write(&file, b"x").await.unwrap();
        let exec = DefaultRestoreExec::default();
        let err = exec
            .write_file(&file.join("nested"), b"x")
            .await
            .unwrap_err();
        assert!(!err.is_empty());
        // Write TO a directory path → create_dir_all passes, write fails.
        let err = exec.write_file(tmp.path(), b"x").await.unwrap_err();
        assert!(!err.is_empty());
        // remove_file on a real file → Ok; missing → NotFound → Ok.
        exec.remove_file(&file).await.unwrap();
        exec.remove_file(&file).await.unwrap();
        // remove_file on a directory → an error that is not NotFound.
        let _ = exec.remove_file(tmp.path()).await;
        // The 64MiB+ output arm of `truncate`.
        assert_eq!(truncate(b"abc", 10), b"abc");
        assert_eq!(truncate(b"abcdef", 3), b"abc");
    }
}
