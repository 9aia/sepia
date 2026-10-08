//! `DriverClient` — a spawned driver subprocess: stdin requests,
//! stdout responses, notifications broadcast, kill-on-drop.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sepia_driver_sdk::rpc::{self, Inbound, Notification, RpcError};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio::task::JoinHandle;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const STDERR_RING: usize = 64;

type PendingMap = HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>;

struct Inner {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Mutex<PendingMap>,
    next_id: AtomicU64,
    notifications: broadcast::Sender<Notification>,
    timeout: Duration,
    stderr_tail: Mutex<std::collections::VecDeque<String>>,
    reader: Mutex<JoinHandle<()>>,
    stderr_reader: Mutex<JoinHandle<()>>,
    /// Set by the reader task when the child's stdout closes — the
    /// registry watches it to respawn dead drivers.
    closed: std::sync::atomic::AtomicBool,
}

/// A live driver subprocess. Dropping it kills the child.
pub struct DriverClient {
    inner: Arc<Inner>,
}

impl DriverClient {
    /// Spawn `binary` with `args`, wire the reader tasks.
    ///
    /// # Errors
    /// Fails when the process cannot be spawned or stdio is missing.
    pub async fn spawn(binary: &Path, args: &[&str]) -> std::io::Result<Self> {
        Self::spawn_with_env(binary, args, &[]).await
    }

    /// Spawn with extra environment pairs.
    ///
    /// # Errors
    /// Fails when the process cannot be spawned or stdio is missing.
    pub async fn spawn_with_env(
        binary: &Path,
        args: &[&str],
        env: &[(String, String)],
    ) -> std::io::Result<Self> {
        let mut child = Command::new(binary)
            .args(args)
            .envs(env.iter().cloned())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "driver stdout missing")
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "driver stdin missing")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "driver stderr missing")
        })?;

        let (notifications, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            notifications,
            timeout: DEFAULT_TIMEOUT,
            stderr_tail: Mutex::new(std::collections::VecDeque::new()),
            reader: Mutex::new(tokio::spawn(async {})),
            stderr_reader: Mutex::new(tokio::spawn(async {})),
            closed: std::sync::atomic::AtomicBool::new(false),
        });

        let reader = {
            let inner = Arc::clone(&inner);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    inner.dispatch(&line).await;
                }
                inner
                    .closed
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                inner.fail_all_pending("driver stdout closed").await;
            })
        };
        *inner.reader.lock().await = reader;

        let stderr_reader = {
            let inner = Arc::clone(&inner);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut tail = inner.stderr_tail.lock().await;
                    tail.push_back(line);
                    while tail.len() > STDERR_RING {
                        tail.pop_front();
                    }
                }
            })
        };
        *inner.stderr_reader.lock().await = stderr_reader;

        Ok(Self { inner })
    }

    /// Last stderr lines the driver emitted — for error context.
    pub async fn stderr_tail(&self) -> Vec<String> {
        self.inner
            .stderr_tail
            .lock()
            .await
            .iter()
            .cloned()
            .collect()
    }

    /// Kill the child process (restart supervision, crash tests).
    /// The reader task marks the client closed; callers drop it.
    pub async fn kill(&self) {
        let _ = self.inner.child.lock().await.start_kill();
    }

    /// Whether the driver process exited (stdout closed) — callers
    /// should drop this client and respawn.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Subscribe to driver notifications.
    pub fn notifications(&self) -> broadcast::Receiver<Notification> {
        self.inner.notifications.subscribe()
    }

    /// Call `method` with `params`; times out after the configured budget.
    ///
    /// # Errors
    /// `RpcError` on a driver-reported error, transport failure, or timeout.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);
        let request = rpc::Request::new(id, method, params);
        let line =
            rpc::encode_line(&request).map_err(|e| RpcError::internal(e.to_string()))? + "\n";
        {
            let mut stdin = self.inner.stdin.lock().await;
            if let Err(e) = stdin.write_all(line.as_bytes()).await {
                self.inner.pending.lock().await.remove(&id);
                return Err(RpcError::internal(format!("driver stdin: {e}")));
            }
            let _ = stdin.flush().await;
        }
        match tokio::time::timeout(self.inner.timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_closed)) => Err(RpcError::internal("driver response channel closed")),
            Err(_elapsed) => {
                self.inner.pending.lock().await.remove(&id);
                Err(RpcError::internal(format!(
                    "driver call {method} timed out"
                )))
            }
        }
    }
}

impl Inner {
    async fn dispatch(&self, line: &str) {
        match rpc::decode_line(line) {
            Ok(Inbound::Response(response)) => {
                let sender = self.pending.lock().await.remove(&response.id);
                if let Some(tx) = sender {
                    let result = match (response.result, response.error) {
                        (_, Some(error)) => Err(error),
                        (result, None) => Ok(result.unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(result);
                }
            }
            Ok(Inbound::Notification(notification)) => {
                let _ = self.notifications.send(notification);
            }
            Ok(Inbound::Request(_)) | Err(_) => {}
        }
    }

    async fn fail_all_pending(&self, reason: &str) {
        let mut pending = self.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(RpcError::internal(reason)));
        }
    }
}

impl Drop for DriverClient {
    fn drop(&mut self) {
        self.inner.reader.try_lock().map(|r| r.abort()).ok();
        self.inner.stderr_reader.try_lock().map(|r| r.abort()).ok();
    }
}
