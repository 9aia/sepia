//! `Translator` — ACP session updates → protocol-v2 `SessionEvent`s.
//! The state machine mints message ids, closes open frames on
//! transitions, and synthesizes a `ToolCallStart` for mid-attach
//! updates.

use std::collections::HashSet;

use sepia_acp::{AcpSessionUpdate, PermissionRequest};
use sepia_proto::SessionEvent;
use serde_json::Value;

pub struct Translator {
    thread_id: String,
    run_id: String,
    message_id_base: String,
    open_message_id: Option<String>,
    open_reasoning_id: Option<String>,
    open_tool_calls: HashSet<String>,
    message_seq: u64,
    turn_ended: bool,
}

impl Translator {
    pub fn new(thread_id: &str) -> Self {
        Self {
            thread_id: thread_id.to_string(),
            run_id: format!("run_{}", uuid::Uuid::new_v4()),
            message_id_base: format!("message_{}", uuid::Uuid::new_v4()),
            open_message_id: None,
            open_reasoning_id: None,
            open_tool_calls: HashSet::new(),
            message_seq: 0,
            turn_ended: false,
        }
    }

    fn next_message_id(&mut self) -> String {
        self.message_seq += 1;
        format!("{}_{}", self.message_id_base, self.message_seq)
    }

    fn close_text(&mut self, events: &mut Vec<SessionEvent>) {
        if let Some(id) = self.open_message_id.take() {
            events.push(SessionEvent::TextMessageEnd { message_id: id });
        }
    }

    fn close_reasoning(&mut self, events: &mut Vec<SessionEvent>) {
        if let Some(id) = self.open_reasoning_id.take() {
            events.push(SessionEvent::ReasoningMessageEnd { message_id: id });
        }
    }

    fn close_tool_calls(&mut self, events: &mut Vec<SessionEvent>) {
        for tool_call_id in self.open_tool_calls.drain() {
            events.push(SessionEvent::ToolCallEnd {
                tool_call_id,
                status: None,
                tool_call_name: None,
                locations: None,
                diffs: None,
                contents: None,
            });
        }
    }

    /// A turn boundary: open frames are reset, `RunStarted` emitted.
    pub fn start_run(&mut self) -> Vec<SessionEvent> {
        self.open_message_id = None;
        self.open_reasoning_id = None;
        self.open_tool_calls.clear();
        self.turn_ended = false;
        vec![SessionEvent::RunStarted {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
        }]
    }

    /// Turn end: closes every open frame then `RunFinished`; the second
    /// call is a no-op (attach emits around a possibly-no-op load).
    pub fn end_turn(&mut self) -> Vec<SessionEvent> {
        if self.turn_ended {
            return Vec::new();
        }
        self.turn_ended = true;
        let mut events = Vec::new();
        self.close_text(&mut events);
        self.close_reasoning(&mut events);
        self.close_tool_calls(&mut events);
        events.push(SessionEvent::RunFinished {
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
        });
        events
    }

    pub fn permission_request(request: &PermissionRequest) -> Vec<SessionEvent> {
        vec![SessionEvent::Custom {
            name: "acp:permission_request".into(),
            value: serde_json::to_value(request).unwrap_or(Value::Null),
        }]
    }

    pub fn translate(&mut self, update: &AcpSessionUpdate) -> Vec<SessionEvent> {
        match update {
            AcpSessionUpdate::AgentMessageChunk { text } => {
                let mut events = Vec::new();
                self.close_reasoning(&mut events);
                if self.open_message_id.is_none() {
                    let id = self.next_message_id();
                    events.push(SessionEvent::TextMessageStart {
                        message_id: id.clone(),
                        role: "assistant".into(),
                    });
                    self.open_message_id = Some(id);
                }
                events.push(SessionEvent::TextMessageContent {
                    message_id: self.open_message_id.clone().unwrap_or_default(),
                    delta: text.clone(),
                });
                events
            }
            AcpSessionUpdate::AgentThoughtChunk { text } => {
                let mut events = Vec::new();
                self.close_text(&mut events);
                if self.open_reasoning_id.is_none() {
                    let id = self.next_message_id();
                    events.push(SessionEvent::ReasoningMessageStart {
                        message_id: id.clone(),
                        role: "reasoning".into(),
                    });
                    self.open_reasoning_id = Some(id);
                }
                events.push(SessionEvent::ReasoningMessageContent {
                    message_id: self.open_reasoning_id.clone().unwrap_or_default(),
                    delta: text.clone(),
                });
                events
            }
            AcpSessionUpdate::UserMessageChunk { .. } => {
                // The client already renders the user's own input —
                // echoing it here would duplicate it.
                Vec::new()
            }
            AcpSessionUpdate::ToolCall {
                tool_call_id,
                title,
                raw_input,
                locations,
                diffs,
                contents,
                ..
            } => {
                let mut events = Vec::new();
                self.close_text(&mut events);
                self.close_reasoning(&mut events);
                events.push(SessionEvent::ToolCallStart {
                    tool_call_id: tool_call_id.clone(),
                    tool_call_name: title.clone(),
                    locations: (!locations.is_empty()).then(|| locations.clone()),
                    diffs: (!diffs.is_empty()).then(|| diffs.clone()),
                    contents: contents.as_ref().filter(|c| !c.is_empty()).map(|c| {
                        c.iter()
                            .map(serde_json::to_value)
                            .map(|r| r.unwrap_or(Value::Null))
                            .collect()
                    }),
                });
                events.push(SessionEvent::ToolCallArgs {
                    tool_call_id: tool_call_id.clone(),
                    delta: serde_json::to_string(raw_input).unwrap_or_else(|_| "null".into()),
                    tool_call_name: None,
                });
                self.open_tool_calls.insert(tool_call_id.clone());
                events
            }
            AcpSessionUpdate::ToolCallUpdate {
                tool_call_id,
                title,
                status,
                raw_input,
                raw_output,
                locations,
                diffs,
                contents,
            } => {
                let mut events = Vec::new();
                if !self.open_tool_calls.contains(tool_call_id) {
                    // Attached mid-call — synthesize START so the stream
                    // stays well-formed.
                    events.push(SessionEvent::ToolCallStart {
                        tool_call_id: tool_call_id.clone(),
                        tool_call_name: title.clone().unwrap_or_else(|| tool_call_id.clone()),
                        locations: None,
                        diffs: None,
                        contents: None,
                    });
                    self.open_tool_calls.insert(tool_call_id.clone());
                }
                if let Some(raw_input) = raw_input {
                    events.push(SessionEvent::ToolCallArgs {
                        tool_call_id: tool_call_id.clone(),
                        delta: serde_json::to_string(raw_input).unwrap_or_else(|_| "null".into()),
                        tool_call_name: title.clone(),
                    });
                }
                if status == "completed" || status == "failed" {
                    if let Some(raw_output) = raw_output {
                        events.push(SessionEvent::ToolCallResult {
                            message_id: self.next_message_id(),
                            tool_call_id: tool_call_id.clone(),
                            content: serde_json::to_string(raw_output)
                                .unwrap_or_else(|_| "null".into()),
                        });
                    }
                    events.push(SessionEvent::ToolCallEnd {
                        tool_call_id: tool_call_id.clone(),
                        status: Some(status.clone()),
                        tool_call_name: title.clone(),
                        locations: locations.clone(),
                        diffs: diffs.clone(),
                        contents: contents.as_ref().map(|c| {
                            c.iter()
                                .map(serde_json::to_value)
                                .map(|r| r.unwrap_or(Value::Null))
                                .collect()
                        }),
                    });
                    self.open_tool_calls.remove(tool_call_id);
                } else if locations.is_some() || diffs.is_some() || contents.is_some() {
                    // A mid-call file/content update has no dedicated
                    // frame — a named custom carries it.
                    events.push(SessionEvent::Custom {
                        name: "acp:tool_call_update".into(),
                        value: serde_json::json!({
                            "toolCallId": tool_call_id,
                            "title": title,
                            "locations": locations,
                            "diffs": diffs,
                            "contents": contents,
                        }),
                    });
                }
                events
            }
            AcpSessionUpdate::Plan { .. } => vec![SessionEvent::Custom {
                name: "acp:plan".into(),
                value: serde_json::to_value(update).unwrap_or(Value::Null),
            }],
            AcpSessionUpdate::CurrentModeUpdate { .. } => vec![SessionEvent::Custom {
                name: "acp:current_mode_update".into(),
                value: serde_json::to_value(update).unwrap_or(Value::Null),
            }],
            AcpSessionUpdate::AvailableCommandsUpdate { .. } => vec![SessionEvent::Custom {
                name: "acp:available_commands_update".into(),
                value: serde_json::to_value(update).unwrap_or(Value::Null),
            }],
            AcpSessionUpdate::Other {
                session_update,
                raw,
                ..
            } => vec![SessionEvent::Custom {
                name: format!("acp:{session_update}"),
                value: raw.clone(),
            }],
        }
    }
}
