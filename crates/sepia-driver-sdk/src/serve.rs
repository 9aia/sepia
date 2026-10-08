//! The driver serve loop — a driver binary is:
//!
//! ```ignore
//! let store = MyStore::open(config)?;
//! sepia_driver_sdk::serve_store(manifest(), store, &[]).await
//! ```
//!
//! Reads ndjson JSON-RPC requests on stdin, writes responses on stdout.
//! Requests are dispatched sequentially — store reads are cheap and
//! ordering keeps per-session state sane.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use sepia_core::storage::{NodesWindowOptions, SessionRepository};
use sepia_core::{Session, StorageError};

use crate::manifest::{Capability, DriverManifest};
use crate::methods;
use crate::rpc::{self, Inbound, RpcError};

/// What a driver binary exposes — `StoreDriver` covers the common case of
/// "a `SessionRepository` plus a manifest"; implement `Driver` directly
/// for custom methods.
#[async_trait]
pub trait Driver: Send + Sync {
    fn manifest(&self) -> &DriverManifest;

    /// Handle one RPC method; return the result payload.
    async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError>;
}

/// A driver wrapping a [`SessionRepository`]: session reads/writes map
/// onto the port; capabilities gate optional methods.
pub struct StoreDriver {
    manifest: DriverManifest,
    store: Arc<dyn SessionRepository>,
}

impl StoreDriver {
    pub fn new(manifest: DriverManifest, store: Arc<dyn SessionRepository>) -> Self {
        Self { manifest, store }
    }

    fn require(&self, capability: &Capability) -> Result<(), RpcError> {
        if self.manifest.capabilities.contains(capability) {
            Ok(())
        } else {
            Err(RpcError::new(
                rpc::CAPABILITY_UNSUPPORTED,
                format!("driver {} lacks {capability:?}", self.manifest.id),
            ))
        }
    }

    fn store_err(e: StorageError) -> RpcError {
        RpcError::store(e.message)
    }

    async fn session_or_404(&self, params: methods::IdParams) -> Result<Session, RpcError> {
        self.store
            .get_by_id(&params.id, params.agent_id.as_deref())
            .await
            .map_err(Self::store_err)?
            .ok_or_else(|| RpcError::new(rpc::SESSION_NOT_FOUND, params.id))
    }
}

#[async_trait]
impl Driver for StoreDriver {
    fn manifest(&self) -> &DriverManifest {
        &self.manifest
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            methods::DRIVER_MANIFEST => {
                serde_json::to_value(&self.manifest).map_err(|e| RpcError::internal(e.to_string()))
            }
            methods::SESSION_LIST => {
                let sessions = self.store.list().await.map_err(Self::store_err)?;
                serde_json::to_value(sessions).map_err(|e| RpcError::internal(e.to_string()))
            }
            methods::SESSION_GET => {
                let p: methods::IdParams = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let session = self.session_or_404(p).await?;
                serde_json::to_value(session).map_err(|e| RpcError::internal(e.to_string()))
            }
            methods::SESSION_SUMMARY => {
                let p: methods::IdParams = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let summary = self
                    .store
                    .summary(&p.id, p.agent_id.as_deref())
                    .await
                    .map_err(Self::store_err)?
                    .ok_or_else(|| RpcError::new(rpc::SESSION_NOT_FOUND, p.id))?;
                serde_json::to_value(summary).map_err(|e| RpcError::internal(e.to_string()))
            }
            methods::SESSION_HISTORY => {
                let p: methods::HistoryParams = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let window = self
                    .store
                    .nodes_window(
                        &p.id,
                        &NodesWindowOptions {
                            limit: p.limit,
                            before: p.before,
                            agent_id: p.agent_id,
                        },
                    )
                    .await
                    .map_err(Self::store_err)?
                    .ok_or_else(|| RpcError::new(rpc::SESSION_NOT_FOUND, p.id))?;
                serde_json::to_value(window).map_err(|e| RpcError::internal(e.to_string()))
            }
            methods::SESSION_SAVE => {
                self.require(&Capability::SessionWrite)?;
                let session: Session = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                self.store.save(&session).await.map_err(Self::store_err)?;
                Ok(Value::Null)
            }
            methods::SESSION_DELETE => {
                self.require(&Capability::SessionWrite)?;
                let p: methods::IdParams = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                self.store.delete(&p.id).await.map_err(Self::store_err)?;
                Ok(Value::Null)
            }
            methods::SESSION_CHECKPOINTS => {
                let p: methods::IdParams = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let session = self.session_or_404(p).await?;
                Ok(serde_json::json!({ "checkpoints": session.checkpoints }))
            }
            methods::SESSION_RENAME => {
                self.require(&Capability::SessionWrite)?;
                let p: methods::RenameParams = serde_json::from_value(params)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
                let mut session = self
                    .session_or_404(methods::IdParams {
                        id: p.id.clone(),
                        agent_id: None,
                    })
                    .await?;
                session.title = p.title;
                self.store.save(&session).await.map_err(Self::store_err)?;
                Ok(Value::Null)
            }
            methods::SESSION_REWIND | methods::FILE_RESTORE => Err(RpcError::new(
                rpc::CAPABILITY_UNSUPPORTED,
                format!("{method} is not implemented by this driver"),
            )),
            _ => Err(RpcError::new(
                rpc::METHOD_NOT_FOUND,
                format!("unknown method {method}"),
            )),
        }
    }
}

/// Run `driver` on stdio until EOF. Returns when the host closes stdin.
///
/// # Errors
/// Propagates stdout write failures — a dead host means the driver exits.
pub async fn serve<D: Driver>(driver: D) -> std::io::Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut lines = stdin.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match rpc::decode_line(&line) {
            Ok(Inbound::Request(req)) => match driver.handle(&req.method, req.params).await {
                Ok(result) => rpc::Response::ok(req.id, result),
                Err(error) => rpc::Response::err(req.id, error),
            },
            Ok(Inbound::Notification(_) | Inbound::Response(_)) => continue,
            Err((id, error)) => rpc::Response::err(id.unwrap_or(0), error),
        };
        let mut out = rpc::encode_line(&response).unwrap_or_else(|_| {
            rpc::encode_line(&rpc::Response::err(
                response.id,
                RpcError::internal("response serialization failed"),
            ))
            .unwrap_or_default()
        });
        out.push('\n');
        stdout.write_all(out.as_bytes()).await?;
        stdout.flush().await?;
    }
    Ok(())
}

/// Convenience: serve a [`SessionRepository`] as a driver binary.
///
/// # Errors
/// Propagates stdio failures from [`serve`].
pub async fn serve_store(
    manifest: DriverManifest,
    store: Arc<dyn SessionRepository>,
) -> std::io::Result<()> {
    serve(StoreDriver::new(manifest, store)).await
}
