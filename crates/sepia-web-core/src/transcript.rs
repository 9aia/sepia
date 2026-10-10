//! The live transcript — a pure reducer over `sepia_proto::SessionEvent`
//! frames. The UI renders `entries` in order; deltas accumulate into the
//! entry keyed by `message_id`/`tool_call_id`, so a re-parsed stream
//! stays append-only.
//!
//! (Moved verbatim from `sepia-web`'s `live` module — it was already
//! DOM-free.)

use sepia_core::{ToolCallDiff, ToolCallLocation};
use sepia_proto::SessionEvent;
use serde_json::Value;

/// What a live row is: streamed assistant text, streamed reasoning, a
/// tool call, or a thin run-boundary marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveKind {
    Assistant,
    Reasoning,
    Tool,
    /// `RunStarted`/`RunFinished` — a provenance divider, no body.
    Marker,
}

/// One row of the live transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveEntry {
    /// `message_id` or `tool_call_id` — the dedupe key.
    pub key: String,
    pub kind: LiveKind,
    /// Tool name for `Tool` rows, `"thinking"` for reasoning, the marker
    /// label for `Marker` rows.
    pub title: String,
    /// Accumulated text/args.
    pub text: String,
    /// Result payload once `ToolCallResult` lands.
    pub result: Option<String>,
    /// Start→End seen, or run finished.
    pub done: bool,
    /// `ToolCallEnd { status: "error" | "failed" }` (or a failed run
    /// edge).
    pub error: bool,
    /// Files the call touched (`ToolCallStart`/`End`/`acp:tool_call_update`).
    pub locations: Vec<ToolCallLocation>,
    /// Recorded before/after payloads — the live diff view.
    pub diffs: Vec<ToolCallDiff>,
    /// Non-diff ACP `ToolCallContent` entries the stream forwarded.
    pub contents: Vec<Value>,
}

impl LiveEntry {
    fn new(key: &str, kind: LiveKind, title: &str) -> Self {
        Self {
            key: key.to_string(),
            kind,
            title: title.to_string(),
            text: String::new(),
            result: None,
            done: false,
            error: false,
            locations: Vec::new(),
            diffs: Vec::new(),
            contents: Vec::new(),
        }
    }
}

/// One clickable answer on a permission card — the `options` entries of
/// `acp:permission_request` (`{optionId, name, kind}`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionOptionDto {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// A pending `acp:permission_request` — `sepia_acp::PermissionRequest`'s
/// wire form (`{requestId, sessionId, toolCallId?, title, options}`).
/// Answered via `POST /api/sessions/{id}/permission`.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PendingPermission {
    pub request_id: String,
    pub session_id: String,
    pub tool_call_id: Option<String>,
    pub title: String,
    pub options: Vec<PermissionOptionDto>,
}

/// The accumulated live state for the detail page's SSE subscription.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveTranscript {
    pub entries: Vec<LiveEntry>,
    /// Between `RunStarted` and `RunFinished`.
    pub running: bool,
    /// The agent is blocked on approval — rendered as an inline card
    /// until answered (or the run ends).
    pub pending_permission: Option<PendingPermission>,
}

impl LiveTranscript {
    fn entry(&mut self, key: &str, kind: LiveKind, title: &str) -> &mut LiveEntry {
        let pos = self
            .entries
            .iter()
            .position(|e| e.key == key)
            .unwrap_or_else(|| {
                self.entries.push(LiveEntry::new(key, kind, title));
                self.entries.len() - 1
            });
        &mut self.entries[pos]
    }

    /// Optimistic local echo — the prompt API doesn't stream the user's
    /// own message back, so the UI appends it itself.
    pub fn push_user(&mut self, text: &str) {
        let mut entry = LiveEntry::new(
            &format!("local-{}", self.entries.len()),
            LiveKind::Assistant,
            "you",
        );
        entry.text = text.to_string();
        entry.done = true;
        self.entries.push(entry);
    }

    /// Fold one SSE frame into the transcript.
    pub fn apply(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::RunStarted { .. } => {
                self.running = true;
                let key = format!("run-start-{}", self.entries.len());
                let mut marker = LiveEntry::new(&key, LiveKind::Marker, "run started");
                marker.done = true;
                self.entries.push(marker);
            }
            SessionEvent::TextMessageStart { message_id, .. } => {
                self.entry(message_id, LiveKind::Assistant, "assistant");
            }
            SessionEvent::TextMessageContent { message_id, delta } => {
                self.entry(message_id, LiveKind::Assistant, "assistant")
                    .text
                    .push_str(delta);
            }
            SessionEvent::TextMessageEnd { message_id } => {
                self.entry(message_id, LiveKind::Assistant, "assistant")
                    .done = true;
            }
            SessionEvent::ReasoningMessageStart { message_id, .. } => {
                self.entry(message_id, LiveKind::Reasoning, "thinking");
            }
            SessionEvent::ReasoningMessageContent { message_id, delta } => {
                self.entry(message_id, LiveKind::Reasoning, "thinking")
                    .text
                    .push_str(delta);
            }
            SessionEvent::ReasoningMessageEnd { message_id } => {
                self.entry(message_id, LiveKind::Reasoning, "thinking").done = true;
            }
            SessionEvent::ToolCallStart {
                tool_call_id,
                tool_call_name,
                locations,
                diffs,
                contents,
            } => {
                let entry = self.entry(tool_call_id, LiveKind::Tool, tool_call_name);
                if let Some(locations) = locations {
                    entry.locations.clone_from(locations);
                }
                if let Some(diffs) = diffs {
                    entry.diffs.clone_from(diffs);
                }
                if let Some(contents) = contents {
                    entry.contents.clone_from(contents);
                }
            }
            SessionEvent::ToolCallArgs {
                tool_call_id,
                delta,
                tool_call_name,
            } => {
                let name = tool_call_name.as_deref().unwrap_or("tool");
                let entry = self.entry(tool_call_id, LiveKind::Tool, name);
                // A later frame may carry the real name — replace the
                // placeholder an early Args frame set.
                if entry.title == "tool" && name != "tool" {
                    entry.title = name.to_string();
                }
                entry.text.push_str(delta);
            }
            SessionEvent::ToolCallResult {
                tool_call_id,
                content,
                ..
            } => {
                self.entry(tool_call_id, LiveKind::Tool, "tool").result = Some(content.clone());
            }
            SessionEvent::ToolCallEnd {
                tool_call_id,
                status,
                tool_call_name,
                locations,
                diffs,
                contents,
            } => {
                let name = tool_call_name.as_deref().unwrap_or("tool");
                let entry = self.entry(tool_call_id, LiveKind::Tool, name);
                if entry.title == "tool" && name != "tool" {
                    entry.title = name.to_string();
                }
                entry.done = true;
                // ACP terminal states are `completed`/`failed`; the
                // proto doc also permits `error`.
                entry.error = matches!(status.as_deref(), Some("error" | "failed"));
                if let Some(locations) = locations {
                    entry.locations.clone_from(locations);
                }
                if let Some(diffs) = diffs {
                    entry.diffs.clone_from(diffs);
                }
                if let Some(contents) = contents {
                    entry.contents.clone_from(contents);
                }
            }
            SessionEvent::RunFinished { .. } => {
                self.running = false;
                // A turn ending settles any open request — a card left
                // over would be unanswerable anyway.
                self.pending_permission = None;
                for entry in &mut self.entries {
                    entry.done = true;
                }
                let key = format!("run-end-{}", self.entries.len());
                let mut marker = LiveEntry::new(&key, LiveKind::Marker, "run ended");
                marker.done = true;
                self.entries.push(marker);
            }
            // `acp:*` escapes carry no transcript text — except the
            // permission request (an answerable card) and mid-call file
            // updates, which fold into the open tool row.
            SessionEvent::Custom { name, value } => {
                if name == "acp:permission_request"
                    && let Ok(p) = serde_json::from_value::<PendingPermission>(value.clone())
                    && !p.request_id.is_empty()
                {
                    self.pending_permission = Some(p);
                } else if name == "acp:tool_call_update" {
                    let id = value
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if !id.is_empty() {
                        let title = value
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let entry = self.entry(&id, LiveKind::Tool, &title);
                        if entry.title == "tool" && title != "tool" {
                            entry.title.clone_from(&title);
                        }
                        if let Ok(loc) = serde_json::from_value::<Vec<ToolCallLocation>>(
                            value.get("locations").cloned().unwrap_or(Value::Null),
                        ) {
                            if !loc.is_empty() {
                                entry.locations = loc;
                            }
                        }
                        if let Ok(d) = serde_json::from_value::<Vec<ToolCallDiff>>(
                            value.get("diffs").cloned().unwrap_or(Value::Null),
                        ) {
                            if !d.is_empty() {
                                entry.diffs = d;
                            }
                        }
                        if let Some(c) = value.get("contents").and_then(Value::as_array) {
                            if !c.is_empty() {
                                entry.contents.clone_from(c);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn text_deltas_accumulate() {
        let mut t = LiveTranscript::default();
        t.apply(&SessionEvent::RunStarted {
            thread_id: "s".into(),
            run_id: "r".into(),
        });
        t.apply(&SessionEvent::TextMessageStart {
            message_id: "m1".into(),
            role: "assistant".into(),
        });
        t.apply(&SessionEvent::TextMessageContent {
            message_id: "m1".into(),
            delta: "hello ".into(),
        });
        t.apply(&SessionEvent::TextMessageContent {
            message_id: "m1".into(),
            delta: "world".into(),
        });
        t.apply(&SessionEvent::TextMessageEnd {
            message_id: "m1".into(),
        });
        t.apply(&SessionEvent::RunFinished {
            thread_id: "s".into(),
            run_id: "r".into(),
        });
        assert!(!t.running);
        // run-started marker + the message + run-ended marker.
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[0].kind, LiveKind::Marker);
        assert_eq!(t.entries[0].title, "run started");
        assert_eq!(t.entries[1].kind, LiveKind::Assistant);
        assert_eq!(t.entries[1].text, "hello world");
        assert!(t.entries[1].done);
        assert_eq!(t.entries[2].title, "run ended");
    }

    #[test]
    fn reasoning_and_tool_rows() {
        let mut t = LiveTranscript::default();
        t.apply(&SessionEvent::ReasoningMessageContent {
            message_id: "th".into(),
            delta: "plan".into(),
        });
        t.apply(&SessionEvent::ToolCallStart {
            tool_call_id: "tc".into(),
            tool_call_name: "edit_file".into(),
            locations: None,
            diffs: None,
            contents: None,
        });
        t.apply(&SessionEvent::ToolCallArgs {
            tool_call_id: "tc".into(),
            delta: "{}".into(),
            tool_call_name: None,
        });
        t.apply(&SessionEvent::ToolCallResult {
            message_id: "m".into(),
            tool_call_id: "tc".into(),
            content: "done".into(),
        });
        t.apply(&SessionEvent::ToolCallEnd {
            tool_call_id: "tc".into(),
            status: Some("error".into()),
            tool_call_name: None,
            locations: None,
            diffs: None,
            contents: None,
        });
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0].kind, LiveKind::Reasoning);
        assert_eq!(t.entries[1].kind, LiveKind::Tool);
        assert_eq!(t.entries[1].title, "edit_file");
        assert_eq!(t.entries[1].result.as_deref(), Some("done"));
        assert!(t.entries[1].done);
        assert!(t.entries[1].error);
    }

    #[test]
    fn tool_call_end_carries_locations_diffs_and_failed_status() {
        let mut t = LiveTranscript::default();
        t.apply(&SessionEvent::ToolCallStart {
            tool_call_id: "tc".into(),
            tool_call_name: "edit".into(),
            locations: Some(vec![ToolCallLocation {
                path: "/a.rs".into(),
                line: Some(3),
            }]),
            diffs: None,
            contents: None,
        });
        t.apply(&SessionEvent::Custom {
            name: "acp:tool_call_update".into(),
            value: serde_json::json!({
                "toolCallId": "tc",
                "diffs": [{"path": "/a.rs", "oldText": "x", "newText": "y"}],
            }),
        });
        t.apply(&SessionEvent::ToolCallEnd {
            tool_call_id: "tc".into(),
            status: Some("failed".into()),
            tool_call_name: None,
            locations: None,
            diffs: None,
            contents: Some(vec![
                serde_json::json!({"type": "terminal", "terminalId": "t1"}),
            ]),
        });
        let e = &t.entries[0];
        assert_eq!(e.locations[0].path, "/a.rs");
        assert_eq!(e.diffs[0].path, "/a.rs");
        assert_eq!(e.diffs[0].new_text.as_deref(), Some("y"));
        assert_eq!(e.contents.len(), 1);
        assert!(e.error);
    }

    #[test]
    fn permission_request_becomes_pending() {
        let mut t = LiveTranscript::default();
        t.apply(&SessionEvent::Custom {
            name: "acp:permission_request".into(),
            value: serde_json::json!({
                "requestId": "r1",
                "sessionId": "s1",
                "toolCallId": "tc-1",
                "title": "Run it",
                "options": [
                    { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                    { "optionId": "deny", "name": "Deny", "kind": "reject_once" }
                ]
            }),
        });
        let p = t.pending_permission.clone().unwrap_or_default();
        assert_eq!(p.request_id, "r1");
        assert_eq!(p.tool_call_id.as_deref(), Some("tc-1"));
        assert_eq!(p.options.len(), 2);
        assert_eq!(p.options[0].option_id, "allow");
        // Unrelated custom frames leave it alone; run end settles it.
        t.apply(&SessionEvent::Custom {
            name: "acp:plan".into(),
            value: serde_json::Value::Null,
        });
        assert!(t.pending_permission.is_some());
        t.apply(&SessionEvent::RunFinished {
            thread_id: "s".into(),
            run_id: "r".into(),
        });
        assert!(t.pending_permission.is_none());
    }
}
