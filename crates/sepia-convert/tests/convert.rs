#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sepia_convert::{ImportedHistoryMessage, import_session, session_from_history};
use sepia_core::storage::SessionRepository;
use sepia_core::{Role, Session, StorageError};
use sepia_testkit::contract;
use serde_json::json;

struct MemStore {
    sessions: Mutex<HashMap<String, Session>>,
}

#[async_trait]
impl SessionRepository for MemStore {
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        self.sessions
            .lock()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        Ok(())
    }
    async fn get_by_id(&self, id: &str, _a: Option<&str>) -> Result<Option<Session>, StorageError> {
        Ok(self.sessions.lock().unwrap().get(id).cloned())
    }
    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        Ok(self.sessions.lock().unwrap().values().cloned().collect())
    }
    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        self.sessions.lock().unwrap().remove(id);
        Ok(())
    }
    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.sessions.lock().unwrap().contains_key(id))
    }
}

fn msg(role: Role, content: &str, created_at: f64) -> ImportedHistoryMessage {
    ImportedHistoryMessage {
        role,
        content: content.into(),
        blocks: None,
        created_at,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_status: None,
        exit_code: None,
        duration_ms: None,
    }
}

#[test]
fn rebuilds_linear_nodes_and_prompt_history() {
    let session = session_from_history(
        "s1",
        "T",
        "/work",
        "m",
        &[
            msg(Role::User, "first", 1_700_000_000_000.0),
            msg(Role::Assistant, "reply", 1_700_000_010_000.0),
            msg(Role::Tool, "result", 1_700_000_020_000.0),
        ],
    );
    assert_eq!(session.nodes.len(), 3);
    assert_eq!(session.nodes[1].parent_node_id, Some(0));
    assert_eq!(session.nodes[2].parent_node_id, Some(1));
    // ms → seconds timestamps.
    assert_eq!(session.nodes[0].created_at, 1_700_000_000.0);
    assert_eq!(session.last_activity_at, 1_700_000_020.0);
    // Prompt history keeps only user messages.
    assert_eq!(session.prompt_history.len(), 1);
    assert_eq!(session.prompt_history[0].content, "first");
    assert_eq!(session.backend_type, "windsurf");
}

#[test]
fn preserves_thinking_signature_and_tool_result() {
    let mut m = msg(Role::Assistant, "", 1_700_000_000_000.0);
    m.thinking = Some("reasoning".into());
    m.thinking_signature = Some("sealed.v1.xyz".into());
    let mut t = msg(Role::Tool, "out", 1_700_000_001_000.0);
    t.tool_status = Some(sepia_core::ToolCallStatus::Success);
    t.exit_code = Some(0);
    let session = session_from_history("s1", "T", "/work", "m", &[m, t]);
    assert_eq!(session.nodes[0].thinking.as_deref(), Some("reasoning"));
    assert_eq!(
        session.nodes[0].thinking_signature.as_deref(),
        Some("sealed.v1.xyz")
    );
    let result = session.nodes[1].tool_result.as_ref().unwrap();
    assert_eq!(result.status, sepia_core::ToolCallStatus::Success);
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn import_session_is_idempotent_and_grafts_cogs() {
    let mut donor = contract::session("donor", "Donor", 100.0);
    donor.cogs_json = r#"[{"lifetime":{"Unique":"core/model"},"model":"donor-model"}]"#.into();
    let store: Arc<dyn SessionRepository> = Arc::new(MemStore {
        sessions: Mutex::new(HashMap::from([(donor.id.clone(), donor)])),
    });
    let mut imported = contract::session("new", "New", 100.0);
    imported.working_directory = "/contract".into(); // same cwd → preferred donor
    imported.cogs_json = "[]".into();
    let id = import_session(&store, &imported).await.unwrap();
    assert_eq!(id, "new");
    let saved = store.get_by_id("new", None).await.unwrap().unwrap();
    assert!(saved.cogs_json.contains("donor-model"));
    // Second import leaves the stored session untouched.
    let mut changed = saved.clone();
    changed.title = "changed".into();
    import_session(&store, &changed).await.unwrap();
    assert_eq!(
        store.get_by_id("new", None).await.unwrap().unwrap().title,
        "New"
    );
}

#[tokio::test]
async fn import_fills_model_cog_without_a_donor() {
    let store: Arc<dyn SessionRepository> = Arc::new(MemStore {
        sessions: Mutex::new(HashMap::new()),
    });
    let mut imported = contract::session("alone", "Alone", 100.0);
    imported.model = "my-model".into();
    imported.cogs_json = r#"[{"lifetime":{"Unique":"core/model"},"model":null}]"#.into();
    import_session(&store, &imported).await.unwrap();
    let saved = store.get_by_id("alone", None).await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&saved.cogs_json).unwrap()[0]["model"],
        json!("my-model")
    );
}
