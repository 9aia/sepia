#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_acp::normalize::{normalize_permission, normalize_update};
use sepia_acp::types::{AcpSessionUpdate, ToolCallContent};
use serde_json::json;

#[test]
fn normalizes_the_three_message_chunk_variants() {
    let content = json!({ "type": "text", "text": "hello" });
    for (update_kind, expected) in [
        ("user_message_chunk", "user"),
        ("agent_message_chunk", "agent"),
        ("agent_thought_chunk", "thought"),
    ] {
        let update = normalize_update(&json!({ "sessionUpdate": update_kind, "content": content }));
        let text = match &update {
            AcpSessionUpdate::UserMessageChunk { text }
            | AcpSessionUpdate::AgentMessageChunk { text }
            | AcpSessionUpdate::AgentThoughtChunk { text } => text.clone(),
            other => panic!("wrong variant {other:?}"),
        };
        let _ = expected;
        assert_eq!(text, "hello");
    }
}

#[test]
fn normalizes_tool_call() {
    let update = normalize_update(&json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "t1",
        "title": "Read file",
        "status": "in_progress",
        "kind": "read",
        "rawInput": { "path": "/a" },
        "locations": [{ "path": "/a", "line": 3 }],
    }));
    let AcpSessionUpdate::ToolCall {
        tool_call_id,
        title,
        status,
        tool_kind,
        locations,
        diffs,
        contents,
        ..
    } = update
    else {
        panic!("wrong variant")
    };
    assert_eq!(tool_call_id, "t1");
    assert_eq!(title, "Read file");
    assert_eq!(status, "in_progress");
    assert_eq!(tool_kind, "read");
    assert_eq!(locations[0].path, "/a");
    assert_eq!(locations[0].line, Some(3));
    assert!(diffs.is_empty());
    assert!(contents.is_none());
}

#[test]
fn normalizes_diff_content_on_tool_call_and_update() {
    let content = json!([
        { "type": "diff", "path": "/a", "oldText": "x", "newText": "y" },
        { "type": "diff", "path": "/b", "newText": "new file" },
        { "type": "content", "content": { "type": "text", "text": "done" } },
        { "type": "diff" }
    ]);
    let update = normalize_update(&json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "t1",
        "title": "Edit file",
        "status": "in_progress",
        "kind": "edit",
        "rawInput": {},
        "content": content,
    }));
    let AcpSessionUpdate::ToolCall {
        diffs, contents, ..
    } = update
    else {
        panic!("wrong variant")
    };
    assert_eq!(diffs.len(), 2);
    assert_eq!(diffs[0].old_text.as_deref(), Some("x"));
    assert_eq!(diffs[1].new_text.as_deref(), Some("new file"));
    assert_eq!(
        contents.unwrap(),
        [ToolCallContent::Text {
            text: "done".into()
        }]
    );

    let update = normalize_update(&json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "t1",
        "status": "completed",
        "content": content,
        "locations": [{ "path": "/a" }],
        "rawInput": { "path": "/a" },
    }));
    let AcpSessionUpdate::ToolCallUpdate {
        diffs, locations, ..
    } = update
    else {
        panic!("wrong variant")
    };
    assert_eq!(diffs.unwrap().len(), 2);
    assert_eq!(locations.unwrap()[0].path, "/a");
}

#[test]
fn normalizes_terminal_and_embedded_content() {
    let content = json!([
        { "type": "terminal", "terminalId": "term-1" },
        { "type": "terminal", "terminalId": "term-2", "output": "build ok" },
        { "type": "content", "content": { "type": "text", "text": "result text" } },
        { "type": "content", "content": { "type": "image", "data": "aGk=", "mimeType": "image/png" } },
        { "type": "content", "content": { "type": "resource_link", "uri": "file:///a" } },
        { "type": "terminal" },
        { "type": "diff", "path": "/a", "newText": "y" }
    ]);
    let update = normalize_update(&json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "t1",
        "title": "exec",
        "status": "in_progress",
        "kind": "execute",
        "rawInput": {},
        "content": content,
    }));
    let AcpSessionUpdate::ToolCall {
        contents, diffs, ..
    } = update
    else {
        panic!("wrong variant")
    };
    assert_eq!(diffs.len(), 1);
    assert_eq!(diffs[0].path, "/a");
    let contents = contents.unwrap();
    assert_eq!(contents.len(), 4);
    assert_eq!(
        contents[0],
        ToolCallContent::Terminal {
            terminal_id: "term-1".into(),
            output: None
        }
    );
    assert_eq!(
        contents[1],
        ToolCallContent::Terminal {
            terminal_id: "term-2".into(),
            output: Some("build ok".into())
        }
    );
    assert_eq!(
        contents[3],
        ToolCallContent::Image {
            data: Some("aGk=".into()),
            uri: None,
            mime_type: Some("image/png".into())
        }
    );
}

#[test]
fn normalizes_tool_call_update_with_and_without_title() {
    let update = normalize_update(&json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "t1",
        "status": "completed",
        "title": "Done",
        "rawOutput": { "ok": true },
    }));
    let AcpSessionUpdate::ToolCallUpdate {
        title, raw_output, ..
    } = update
    else {
        panic!("wrong variant")
    };
    assert_eq!(title.as_deref(), Some("Done"));
    assert_eq!(raw_output.unwrap()["ok"], json!(true));

    let update = normalize_update(&json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "t1"
    }));
    let AcpSessionUpdate::ToolCallUpdate {
        status, raw_output, ..
    } = update
    else {
        panic!("wrong variant")
    };
    assert_eq!(status, "");
    assert!(raw_output.is_none());
}

#[test]
fn normalizes_plan_current_mode_and_commands() {
    let update = normalize_update(&json!({
        "sessionUpdate": "plan",
        "entries": [
            { "content": "step", "status": "pending", "priority": "high" },
            { "content": "done", "status": "completed", "priority": "low" }
        ],
    }));
    let AcpSessionUpdate::Plan { entries } = update else {
        panic!("wrong variant")
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].content, "step");
    assert_eq!(entries[1].status, "completed");

    let update = normalize_update(&json!({
        "sessionUpdate": "current_mode_update",
        "currentModeId": "plan"
    }));
    let AcpSessionUpdate::CurrentModeUpdate { mode_id } = update else {
        panic!("wrong variant")
    };
    assert_eq!(mode_id, "plan");

    let update = normalize_update(&json!({
        "sessionUpdate": "available_commands_update",
        "availableCommands": [
            { "name": "init", "description": "Init the repo" },
            { "name": "help" }
        ],
    }));
    let AcpSessionUpdate::AvailableCommandsUpdate { commands } = update else {
        panic!("wrong variant")
    };
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].description.as_deref(), Some("Init the repo"));
    assert!(commands[1].description.is_none());
}

#[test]
fn falls_back_to_other_for_unknown_updates() {
    let raw = json!({ "sessionUpdate": "notice", "message": "hi" });
    let update = normalize_update(&raw);
    let AcpSessionUpdate::Other {
        session_update,
        raw: kept,
    } = update
    else {
        panic!("wrong variant")
    };
    assert_eq!(session_update, "notice");
    assert_eq!(kept, raw);
}

#[test]
fn normalizes_a_permission_request() {
    let request = normalize_permission(&json!({
        "sessionId": "s1",
        "toolCall": { "toolCallId": "t1", "title": "Run command" },
        "options": [{ "optionId": "allow", "name": "Allow", "kind": "allow_once" }],
    }));
    assert_eq!(request.session_id, "s1");
    assert_eq!(request.tool_call_id.as_deref(), Some("t1"));
    assert_eq!(request.title, "Run command");
    assert_eq!(request.options[0].option_id, "allow");
    assert!(request.request_id.starts_with("s1:t1:"));
}

#[test]
fn normalizes_a_permission_request_without_a_tool_call() {
    let request = normalize_permission(&json!({ "sessionId": "s2" }));
    assert!(request.tool_call_id.is_none());
    assert_eq!(request.title, "");
    assert!(request.options.is_empty());
    assert!(request.request_id.starts_with("s2:none:"));
}

#[test]
fn normalization_defaults_missing_fields_to_empty_strings() {
    let update = normalize_update(&json!({
        "sessionUpdate": "user_message_chunk",
        "content": 5
    }));
    assert!(matches!(
        update,
        AcpSessionUpdate::UserMessageChunk { ref text } if text.is_empty()
    ));
    let update = normalize_update(&json!({ "sessionUpdate": "tool_call" }));
    let AcpSessionUpdate::ToolCall {
        tool_call_id,
        title,
        status,
        tool_kind,
        locations,
        diffs,
        ..
    } = update
    else {
        panic!("wrong variant")
    };
    assert!(
        [tool_call_id, title, status, tool_kind]
            .iter()
            .all(String::is_empty)
    );
    assert!(locations.is_empty() && diffs.is_empty());

    let update = normalize_update(&json!({
        "sessionUpdate": "plan",
        "entries": [{ "content": "x" }, { "status": "done" }, {}]
    }));
    let AcpSessionUpdate::Plan { entries } = update else {
        panic!("wrong variant")
    };
    assert_eq!(entries[0].content, "x");
    assert_eq!(entries[0].status, "");
    assert_eq!(entries[1].content, "");
    assert_eq!(entries[1].status, "done");

    let update = normalize_update(&json!({
        "sessionUpdate": "available_commands_update",
        "availableCommands": [{ "name": "a" }, { "description": "b" }, {}]
    }));
    let AcpSessionUpdate::AvailableCommandsUpdate { commands } = update else {
        panic!("wrong variant")
    };
    assert_eq!(commands[0].name, "a");
    assert_eq!(commands[1].name, "");
    assert_eq!(commands[1].description.as_deref(), Some("b"));

    let update = normalize_update(&json!({}));
    assert!(matches!(
        update,
        AcpSessionUpdate::Other { ref session_update, .. } if session_update.is_empty()
    ));
}

#[test]
fn content_entries_drop_unusable_shapes_and_keep_partial() {
    let update = normalize_update(&json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "t",
        "content": [
            { "type": "content", "content": { "type": "text", "text": "hi" } },
            { "type": "content", "content": { "type": "text" } },
            { "type": "content", "content": { "type": "image" } },
            { "type": "content", "content": { "type": "image", "uri": "https://x.png" } },
            { "type": "content", "content": { "type": "image", "data": "AAAA", "mimeType": "image/png" } },
            { "type": "content", "content": { "type": "audio", "data": "x" } },
            { "type": "diff", "path": "/a.ts", "newText": "n" },
            { "type": "diff", "oldText": "o" },
            { "type": "location" }
        ],
        "locations": [{ "path": "/b.ts" }],
    }));
    let AcpSessionUpdate::ToolCall {
        contents,
        locations,
        ..
    } = update
    else {
        panic!("wrong variant")
    };
    let contents = contents.unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(
        contents[1],
        ToolCallContent::Image {
            data: None,
            uri: Some("https://x.png".into()),
            mime_type: None
        }
    );
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].path, "/b.ts");
    assert!(locations[0].line.is_none());
}

#[test]
fn normalize_permission_defaults_absent_fields() {
    let request = normalize_permission(&json!({}));
    assert_eq!(request.session_id, "");
    assert!(request.tool_call_id.is_none());
    assert_eq!(request.title, "");
    assert!(request.options.is_empty());
}
