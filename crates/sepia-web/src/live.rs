//! The live transcript — a pure reducer over `sepia_proto::SessionEvent`
//! frames. The UI renders `entries` in order; deltas accumulate into the
//! entry keyed by `message_id`/`tool_call_id`, so a re-parsed stream
//! stays append-only.

use sepia_proto::SessionEvent;

/// What a live row is: streamed assistant text, streamed reasoning, or
/// a tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveKind {
    Assistant,
    Reasoning,
    Tool,
}

/// One row of the live transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveEntry {
    /// `message_id` or `tool_call_id` — the dedupe key.
    pub key: String,
    pub kind: LiveKind,
    /// Tool name for `Tool` rows, `"thinking"` for reasoning.
    pub title: String,
    /// Accumulated text/args.
    pub text: String,
    /// Result payload once `ToolCallResult` lands.
    pub result: Option<String>,
    /// Start→End seen, or run finished.
    pub done: bool,
    /// `ToolCallEnd { status: "error" }` (or a failed run edge).
    pub error: bool,
}

/// The accumulated live state for the detail page's SSE subscription.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveTranscript {
    pub entries: Vec<LiveEntry>,
    /// Between `RunStarted` and `RunFinished`.
    pub running: bool,
}

impl LiveTranscript {
    fn entry(&mut self, key: &str, kind: LiveKind, title: &str) -> &mut LiveEntry {
        let pos = self
            .entries
            .iter()
            .position(|e| e.key == key)
            .unwrap_or_else(|| {
                self.entries.push(LiveEntry {
                    key: key.to_string(),
                    kind,
                    title: title.to_string(),
                    text: String::new(),
                    result: None,
                    done: false,
                    error: false,
                });
                self.entries.len() - 1
            });
        &mut self.entries[pos]
    }

    /// Optimistic local echo — the prompt API doesn't stream the user's
    /// own message back, so the UI appends it itself.
    pub fn push_user(&mut self, text: &str) {
        self.entries.push(LiveEntry {
            key: format!("local-{}", self.entries.len()),
            kind: LiveKind::Assistant,
            title: "you".to_string(),
            text: text.to_string(),
            result: None,
            done: true,
            error: false,
        });
    }

    /// Fold one SSE frame into the transcript.
    pub fn apply(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::RunStarted { .. } => {
                self.running = true;
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
                ..
            } => {
                self.entry(tool_call_id, LiveKind::Tool, tool_call_name);
            }
            SessionEvent::ToolCallArgs {
                tool_call_id,
                delta,
                tool_call_name,
            } => {
                let name = tool_call_name.as_deref().unwrap_or("tool");
                self.entry(tool_call_id, LiveKind::Tool, name)
                    .text
                    .push_str(delta);
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
                ..
            } => {
                let name = tool_call_name.as_deref().unwrap_or("tool");
                let entry = self.entry(tool_call_id, LiveKind::Tool, name);
                entry.done = true;
                entry.error = status.as_deref() == Some("error");
            }
            SessionEvent::RunFinished { .. } => {
                self.running = false;
                for entry in &mut self.entries {
                    entry.done = true;
                }
            }
            // `acp:*` escapes and unknown frames carry no transcript text.
            SessionEvent::Custom { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.entries[0].text, "hello world");
        assert!(t.entries[0].done);
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
}
