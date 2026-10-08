//! Permission broker — holds `session/request_permission` requests until
//! a human answers; `respond` settles them out of band.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::normalize::normalize_permission;
use crate::types::PermissionRequest;

/// The outcome payload a permission response carries back to the agent.
pub fn outcome_selected(option_id: &str) -> Value {
    json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
}

pub fn outcome_cancelled() -> Value {
    json!({ "outcome": { "outcome": "cancelled" } })
}

#[derive(Default)]
struct State {
    pending: HashMap<String, oneshot::Sender<Result<Value, String>>>,
    dead: Option<String>,
}

/// Settle permission requests minted by [`normalize_permission`].
pub struct PermissionBroker {
    state: Mutex<State>,
}

impl PermissionBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a permission request; the returned receiver resolves when
    /// a human (or failure) settles it.
    pub fn begin(
        &self,
        params: &Value,
    ) -> (PermissionRequest, oneshot::Receiver<Result<Value, String>>) {
        let request = normalize_permission(params);
        let (tx, rx) = oneshot::channel();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(dead) = &state.dead {
            let _ = tx.send(Err(dead.clone()));
        } else {
            state.pending.insert(request.request_id.clone(), tx);
        }
        (request, rx)
    }

    /// Settle `request_id` — `None` option means cancelled. Returns false
    /// when the id is unknown.
    pub fn respond(&self, request_id: &str, option_id: Option<&str>) -> bool {
        let tx = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .remove(request_id);
        let Some(tx) = tx else { return false };
        let outcome = match option_id {
            Some(id) => outcome_selected(id),
            None => outcome_cancelled(),
        };
        let _ = tx.send(Ok(outcome));
        true
    }

    /// Fail every pending request — agent exit path.
    pub fn fail_all(&self, reason: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.dead = Some(reason.to_string());
        for (_, tx) in state.pending.drain() {
            let _ = tx.send(Err(reason.to_string()));
        }
    }
}

impl Default for PermissionBroker {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
        }
    }
}
