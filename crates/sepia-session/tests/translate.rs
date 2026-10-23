#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `Translator` — every ACP update variant → protocol-v2 `SessionEvent`
//! frames, including the open-frame close transitions and the mid-attach
//! synthesized `ToolCallStart`.

use sepia_acp::{
    AcpSessionUpdate, AvailableCommand, PermissionOption, PermissionRequest, PlanEntry,
    ToolCallContent,
};
use sepia_core::{ToolCallDiff, ToolCallLocation};
use sepia_proto::SessionEvent;
use sepia_session::Translator;
use serde_json::json;

fn loc() -> ToolCallLocation {
    ToolCallLocation {
        path: "/a.rs".into(),
        line: Some(3),
    }
}

fn diff() -> ToolCallDiff {
    ToolCallDiff {
        path: "/a.rs".into(),
        old_text: Some("old".into()),
        new_text: Some("new".into()),
    }
}

#[test]
fn run_boundaries_and_double_end() {
    let mut t = Translator::new("th-1");
    let start = t.start_run();
    assert!(matches!(start[0], SessionEvent::RunStarted { .. }));
    let end = t.end_turn();
    assert!(matches!(end.last(), Some(SessionEvent::RunFinished { .. })));
    // A second end_turn with nothing open emits nothing.
    assert!(t.end_turn().is_empty());
}

#[test]
fn message_and_thought_frames_close_each_other() {
    let mut t = Translator::new("th");
    t.start_run();
    let ev = t.translate(&AcpSessionUpdate::AgentMessageChunk { text: "hi".into() });
    assert!(matches!(ev[0], SessionEvent::TextMessageStart { .. }));
    assert!(matches!(ev[1], SessionEvent::TextMessageContent { .. }));
    // Second chunk continues the open message — no new start.
    let ev = t.translate(&acp_message("again"));
    assert_eq!(ev.len(), 1);
    assert!(matches!(ev[0], SessionEvent::TextMessageContent { .. }));
    // A thought closes the text frame and opens a reasoning one.
    let ev = t.translate(&AcpSessionUpdate::AgentThoughtChunk { text: "hmm".into() });
    assert!(matches!(ev[0], SessionEvent::TextMessageEnd { .. }));
    assert!(matches!(ev[1], SessionEvent::ReasoningMessageStart { .. }));
    assert!(matches!(
        ev[2],
        SessionEvent::ReasoningMessageContent { .. }
    ));
    let ev = t.translate(&AcpSessionUpdate::AgentThoughtChunk {
        text: "more".into(),
    });
    assert_eq!(ev.len(), 1);
    assert!(matches!(
        ev[0],
        SessionEvent::ReasoningMessageContent { .. }
    ));
    // End closes the reasoning frame.
    let end = t.end_turn();
    assert!(matches!(end[0], SessionEvent::ReasoningMessageEnd { .. }));
}

fn acp_message(text: &str) -> AcpSessionUpdate {
    AcpSessionUpdate::AgentMessageChunk { text: text.into() }
}

#[test]
fn user_chunk_is_swallowed() {
    let mut t = Translator::new("th");
    assert!(
        t.translate(&AcpSessionUpdate::UserMessageChunk { text: "u".into() })
            .is_empty()
    );
}

#[test]
fn tool_call_start_args_and_end() {
    let mut t = Translator::new("th");
    t.start_run();
    let ev = t.translate(&AcpSessionUpdate::ToolCall {
        tool_call_id: "tc1".into(),
        title: "Write".into(),
        status: "pending".into(),
        tool_kind: "edit".into(),
        raw_input: json!({"path": "/a.rs"}),
        locations: vec![loc()],
        diffs: vec![diff()],
        contents: Some(vec![ToolCallContent::Text { text: "x".into() }]),
    });
    assert!(matches!(ev[0], SessionEvent::ToolCallStart { .. }));
    assert!(matches!(ev[1], SessionEvent::ToolCallArgs { .. }));
    // A mid-call file update emits the custom frame, not an end.
    let ev = t.translate(&AcpSessionUpdate::ToolCallUpdate {
        tool_call_id: "tc1".into(),
        status: "in_progress".into(),
        title: Some("Write".into()),
        raw_input: Some(json!({"path": "/b.rs"})),
        raw_output: None,
        locations: Some(vec![loc()]),
        diffs: None,
        contents: None,
    });
    assert!(
        matches!(&ev[0], SessionEvent::ToolCallArgs { tool_call_name: Some(n), .. } if n == "Write")
    );
    assert!(matches!(&ev[1], SessionEvent::Custom { name, .. } if name == "acp:tool_call_update"));
    // Completion with output → result + end.
    let ev = t.translate(&AcpSessionUpdate::ToolCallUpdate {
        tool_call_id: "tc1".into(),
        status: "completed".into(),
        title: Some("Write".into()),
        raw_input: None,
        raw_output: Some(json!({"ok": true})),
        locations: Some(vec![loc()]),
        diffs: Some(vec![diff()]),
        contents: Some(vec![ToolCallContent::Terminal {
            terminal_id: "t1".into(),
            output: None,
        }]),
    });
    assert!(matches!(ev[0], SessionEvent::ToolCallResult { .. }));
    assert!(
        matches!(&ev[1], SessionEvent::ToolCallEnd { status: Some(s), .. } if s == "completed")
    );
    // Failed status without output → just the end frame.
    let ev = t.translate(&AcpSessionUpdate::ToolCallUpdate {
        tool_call_id: "tc2".into(),
        status: "failed".into(),
        title: None,
        raw_input: None,
        raw_output: None,
        locations: None,
        diffs: None,
        contents: None,
    });
    // tc2 was never open — a synthesized START leads the END.
    assert!(matches!(ev[0], SessionEvent::ToolCallStart { .. }));
    assert!(matches!(&ev[1], SessionEvent::ToolCallEnd { status: Some(s), .. } if s == "failed"));
}

#[test]
fn in_progress_update_without_payload_emits_nothing_extra() {
    let mut t = Translator::new("th");
    t.start_run();
    // Unknown tool call, in-progress, no payload → synthesized START only.
    let ev = t.translate(&AcpSessionUpdate::ToolCallUpdate {
        tool_call_id: "tc9".into(),
        status: "in_progress".into(),
        title: None,
        raw_input: None,
        raw_output: None,
        locations: None,
        diffs: None,
        contents: None,
    });
    assert_eq!(ev.len(), 1);
    assert!(
        matches!(&ev[0], SessionEvent::ToolCallStart { tool_call_id, tool_call_name, .. } if tool_call_id == "tc9" && tool_call_name == "tc9")
    );
    // Same call again, still in progress, nothing to report → empty.
    let ev = t.translate(&AcpSessionUpdate::ToolCallUpdate {
        tool_call_id: "tc9".into(),
        status: "in_progress".into(),
        title: None,
        raw_input: None,
        raw_output: None,
        locations: None,
        diffs: None,
        contents: None,
    });
    assert!(ev.is_empty());
    // end_turn closes the still-open tool call.
    let end = t.end_turn();
    assert!(matches!(end[0], SessionEvent::ToolCallEnd { .. }));
}

#[test]
fn remaining_variants_become_custom_events() {
    let mut t = Translator::new("th");
    let ev = t.translate(&AcpSessionUpdate::Plan {
        entries: vec![PlanEntry {
            content: "step".into(),
            status: "pending".into(),
        }],
    });
    assert!(matches!(&ev[0], SessionEvent::Custom { name, .. } if name == "acp:plan"));
    let ev = t.translate(&AcpSessionUpdate::CurrentModeUpdate {
        mode_id: "m".into(),
    });
    assert!(
        matches!(&ev[0], SessionEvent::Custom { name, .. } if name == "acp:current_mode_update")
    );
    let ev = t.translate(&AcpSessionUpdate::AvailableCommandsUpdate {
        commands: vec![AvailableCommand {
            name: "x".into(),
            description: None,
        }],
    });
    assert!(
        matches!(&ev[0], SessionEvent::Custom { name, .. } if name == "acp:available_commands_update")
    );
    let ev = t.translate(&AcpSessionUpdate::Other {
        session_update: "weird".into(),
        raw: json!({"k": 1}),
    });
    assert!(
        matches!(&ev[0], SessionEvent::Custom { name, value } if name == "acp:weird" && value["k"] == 1)
    );
}

#[test]
fn permission_request_wraps_to_custom() {
    let req = PermissionRequest {
        request_id: "r1".into(),
        session_id: "s1".into(),
        tool_call_id: Some("tc".into()),
        title: "Allow?".into(),
        options: vec![PermissionOption {
            option_id: "allow".into(),
            name: "Allow".into(),
            kind: "allow_once".into(),
        }],
    };
    let ev = Translator::permission_request(&req);
    assert!(
        matches!(&ev[0], SessionEvent::Custom { name, .. } if name == "acp:permission_request")
    );
}

#[test]
fn tool_call_without_locations_or_contents_omits_them() {
    let mut t = Translator::new("th");
    t.start_run();
    let ev = t.translate(&AcpSessionUpdate::ToolCall {
        tool_call_id: "tc".into(),
        title: "Run".into(),
        status: "pending".into(),
        tool_kind: "execute".into(),
        raw_input: json!("plain"),
        locations: vec![],
        diffs: vec![],
        contents: Some(vec![]),
    });
    let SessionEvent::ToolCallStart {
        locations,
        diffs,
        contents,
        ..
    } = &ev[0]
    else {
        panic!("expected ToolCallStart");
    };
    assert!(locations.is_none() && diffs.is_none() && contents.is_none());
}
