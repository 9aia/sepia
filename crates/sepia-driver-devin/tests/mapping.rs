#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `mapping.rs` — the devin↔IR converters: ACP content blocks, tool-call
//! display/status mapping, metrics blobs, chat-message build + parse.

use sepia_core::{
    Block, MessageNode, Role, ToolCall, ToolCallDiff, ToolCallLocation, ToolCallStatus,
};
use sepia_driver_devin::mapping::{
    blocks_from_acp, build_chat_message, from_acp_tool_call_status, parse_chat_message,
    parse_json_or, session_from_devin_row, to_acp_tool_call_status,
};
use serde_json::{Value, json};

fn node(role: Role) -> MessageNode {
    MessageNode {
        node_id: 1,
        parent_node_id: None,
        role,
        content: "body".into(),
        blocks: vec![],
        tool_calls: vec![],
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: 1_700_000_000.0,
        metadata: Value::Null,
    }
}

#[test]
fn blocks_from_acp_every_variant() {
    // Non-array → empty; all-text → empty (duplicates `content`).
    assert!(blocks_from_acp(&json!({"x": 1})).is_empty());
    assert!(blocks_from_acp(&json!([{"type": "text", "text": "hi"}])).is_empty());
    let blocks = blocks_from_acp(&json!([
        {"type": "text", "text": "hi"},
        {"type": "image", "data": "Zg==", "mimeType": "image/png", "uri": "file://x"},
        {"type": "audio", "data": "QQ==", "mimeType": "audio/wav"},
        {"type": "resource_link", "uri": "file://a", "name": "a.rs", "mimeType": "text/x", "size": 3},
        {"type": "resource_link", "uri": "file://b", "title": "b.rs"},
        {"type": "resource", "resource": {"uri": "file://c", "mimeType": "text/plain", "text": "body", "blob": "Yg=="}},
        {"type": "bogus"},
        {"notype": true},
        42,
    ]));
    assert_eq!(blocks.len(), 6);
    assert!(matches!(blocks[1], Block::Image { .. }));
    assert!(matches!(blocks[2], Block::Audio { .. }));
    assert!(matches!(blocks[3], Block::File { .. }));
    assert!(matches!(blocks[5], Block::File { .. }));
}

#[test]
fn tool_call_status_both_directions() {
    assert_eq!(
        from_acp_tool_call_status(Some("completed")),
        Some(ToolCallStatus::Success)
    );
    assert_eq!(
        from_acp_tool_call_status(Some("failed")),
        Some(ToolCallStatus::Error)
    );
    assert_eq!(
        from_acp_tool_call_status(Some("pending")),
        Some(ToolCallStatus::Pending)
    );
    assert_eq!(
        from_acp_tool_call_status(Some("in_progress")),
        Some(ToolCallStatus::Pending)
    );
    assert_eq!(from_acp_tool_call_status(Some("weird")), None);
    assert_eq!(from_acp_tool_call_status(None), None);
    assert_eq!(
        to_acp_tool_call_status(ToolCallStatus::Success),
        "completed"
    );
    assert_eq!(to_acp_tool_call_status(ToolCallStatus::Error), "failed");
    assert_eq!(to_acp_tool_call_status(ToolCallStatus::Pending), "pending");
}

#[test]
fn build_chat_message_roles_and_tool_calls() {
    let mut user = node(Role::User);
    user.blocks = vec![Block::Image {
        data: Some("Zg==".into()),
        mime_type: Some("image/png".into()),
        uri: None,
    }];
    let m = build_chat_message(&user, "m1");
    assert_eq!(m["role"], "user");
    assert!(m["metadata"]["extensions"]["chisel/acp-content-blocks"].is_array());

    let system = build_chat_message(&node(Role::System), "m1");
    assert!(system["metadata"].is_null() || system.get("metadata").is_none());

    // Assistant node flagged rendered carries the tool-call extension.
    let mut asst = node(Role::Assistant);
    asst.metadata = json!({"summarized_from": "x"});
    asst.usage = Some(sepia_core::TokenUsage {
        input: 3.0,
        output: 4.0,
        cache_read: Some(1.0),
        cache_write: None,
        thinking: Some(2.0),
        cost: Some(0.5),
    });
    asst.thinking = Some("hmm".into());
    asst.model = Some("gpt".into());
    asst.request_id = Some("r1".into());
    asst.finish_reason = Some("stop".into());
    for name in [
        "read", "exec", "grep", "webfetch", "edit", "write", "custom",
    ] {
        asst.tool_calls.push(ToolCall {
            id: format!("tc-{name}"),
            name: name.into(),
            arguments: json!({"file_path": "/a.rs"}),
            index: 0,
            kind: "function".into(),
            status: Some(ToolCallStatus::Error),
            exit_code: Some(2),
            duration_ms: Some(9.0),
            locations: vec![ToolCallLocation {
                path: "/a.rs".into(),
                line: Some(1),
            }],
            diffs: vec![ToolCallDiff {
                path: "a.rs".into(),
                old_text: Some("o".into()),
                new_text: Some("n".into()),
            }],
        });
    }
    let m = build_chat_message(&asst, "m1");
    let ext = &m["metadata"]["extensions"]["chisel/tool_call_content"];
    assert!(
        ext["tc-read"]["title"].as_str().unwrap().contains("Read"),
        "{ext}"
    );
    assert_eq!(ext["tc-custom"]["kind"], "function", "{ext}");
    assert!(ext["tc-edit"]["content"].is_array(), "{ext}");

    // Unrendered assistant (is_system_prefix) skips the extension.
    let mut prefix = node(Role::Assistant);
    prefix.metadata = json!({"summarized_from": "x", "is_system_prefix": true});
    prefix.tool_calls = asst.tool_calls.clone();
    let m = build_chat_message(&prefix, "m1");
    assert!(
        m["metadata"]["extensions"]
            .get("chisel/tool_call_content")
            .is_none(),
        "{m}"
    );
}

#[test]
fn parse_chat_message_round_trip() {
    // A built message parses back — metrics→usage, diffs→tool diffs,
    // extensions→blocks/tool_result.
    let mut asst = node(Role::Assistant);
    asst.metadata = json!({"summarized_from": "x"});
    asst.usage = Some(sepia_core::TokenUsage {
        input: 3.0,
        output: 4.0,
        cache_read: None,
        cache_write: None,
        thinking: None,
        cost: None,
    });
    asst.tool_calls = vec![ToolCall {
        id: "tc1".into(),
        name: "edit".into(),
        arguments: json!({"file_path": "/a.rs"}),
        index: 0,
        kind: "function".into(),
        status: Some(ToolCallStatus::Success),
        exit_code: None,
        duration_ms: None,
        locations: vec![],
        diffs: vec![ToolCallDiff {
            path: "a.rs".into(),
            old_text: None,
            new_text: Some("n".into()),
        }],
    }];
    let m = build_chat_message(&asst, "m1");
    let parsed = parse_chat_message(&m, &Value::Null, 7, None, 1.5);
    assert_eq!(parsed.role, Role::Assistant);
    assert_eq!(parsed.node_id, 7);
    assert!(!parsed.tool_calls.is_empty(), "{parsed:?}");

    // A user message with content blocks parses them back.
    let mut user = node(Role::User);
    user.blocks = vec![Block::File {
        uri: Some("file://x".into()),
        name: Some("x.txt".into()),
        mime_type: None,
        size: None,
        text: None,
        data: None,
    }];
    let m = build_chat_message(&user, "m1");
    let parsed = parse_chat_message(&m, &Value::Null, 2, None, 1.0);
    assert_eq!(parsed.role, Role::User);
    assert!(!parsed.blocks.is_empty(), "{parsed:?}");

    // Degenerate rows → defaults, not panics.
    let parsed = parse_chat_message(&json!({}), &Value::Null, 0, None, 0.0);
    assert_eq!(parsed.role, Role::System);
    let parsed = parse_chat_message(&json!({"role": "tool"}), &json!({"k": 1}), 0, None, 0.0);
    assert_eq!(parsed.role, Role::Tool);
}

#[test]
fn parse_json_or_and_session_row_edges() {
    assert_eq!(parse_json_or(None, json!([])), json!([]));
    assert_eq!(parse_json_or(Some("{\"a\":1}"), json!([])), json!({"a": 1}));
    assert_eq!(parse_json_or(Some("bad"), json!({"d":1})), json!({"d":1}));

    // session_from_devin_row fills defaults from a sparse row.
    let row = sepia_driver_devin::mapping::SessionRow {
        id: "s1".into(),
        working_directory: "/w".into(),
        backend_type: "windsurf".into(),
        model: "m".into(),
        agent_mode: "accept-edits".into(),
        created_at: 1.0,
        last_activity_at: 2.0,
        title: None,
        main_chain_id: None,
        shell_last_seen_index: None,
        cogs_json: None,
        workspace_dirs: None,
        hidden: 0,
        metadata: None,
    };
    let s = session_from_devin_row(&row, vec![], vec![], None, None);
    assert_eq!(s.id, "s1");
    assert_eq!(s.title, "s1"); // absent title falls back to the id
}

#[test]
fn build_chat_message_full_feature_nodes() {
    // Blocks of every variant → block_to_acp arms.
    let mut n = node(Role::Assistant);
    n.metadata = json!({"summarized_from": null});
    n.blocks = vec![
        Block::Text { text: "t".into() },
        Block::Image {
            data: Some("d".into()),
            mime_type: Some("image/png".into()),
            uri: Some("u".into()),
        },
        Block::Audio {
            data: Some("a".into()),
            mime_type: Some("audio/wav".into()),
        },
        Block::File {
            uri: Some("u".into()),
            name: None,
            mime_type: Some("text/x".into()),
            size: None,
            text: Some("body".into()),
            data: None,
        },
        Block::File {
            uri: None,
            name: None,
            mime_type: None,
            size: None,
            text: None,
            data: Some("Yg==".into()),
        },
        Block::File {
            uri: Some("file://l".into()),
            name: Some("n".into()),
            mime_type: None,
            size: None,
            text: None,
            data: None,
        },
    ];
    n.tool_calls = vec![ToolCall {
        id: "c1".into(),
        name: "edit".into(),
        arguments: json!({}),
        index: 0,
        kind: "function".into(),
        status: Some(ToolCallStatus::Success),
        exit_code: None,
        duration_ms: None,
        locations: vec![ToolCallLocation {
            path: "a".into(),
            line: Some(1),
        }],
        diffs: vec![ToolCallDiff {
            path: "a".into(),
            old_text: Some("o".into()),
            new_text: Some("n".into()),
        }],
    }];
    n.thinking_signature = Some("sig".into());
    let blob = build_chat_message(&n, "model");
    assert!(blob["metadata"]["extensions"]["chisel/tool_call_content"].is_object());
    assert!(blob["thinking"]["signature"].is_string());

    // A Tool node with a full tool_result → the Role::Tool arm.
    let mut t = node(Role::Tool);
    t.tool_name = Some("exec".into());
    t.tool_call_id = Some("c1".into());
    t.tool_result = Some(sepia_core::ToolResultInfo {
        status: ToolCallStatus::Error,
        exit_code: Some(1),
        duration_ms: Some(5.0),
    });
    let blob = build_chat_message(&t, "model");
    assert!(blob["metadata"]["extensions"]["chisel/tool_result_meta"].is_object());

    // to_iso overflow → empty string rather than panic.
    n.created_at = 1e30;
    let _ = build_chat_message(&n, "model");
}

#[test]
fn parse_chat_message_metrics_and_edge_extensions() {
    // metrics object without token fields → no usage.
    let parsed = parse_chat_message(
        &json!({"role":"assistant","content":"x","metadata":{"metrics":{"cache":1}}}),
        &Value::Null,
        1,
        None,
        0.0,
    );
    assert!(parsed.usage.is_none());
    // tool_call_content entries whose content items aren't diffs drop out.
    let parsed = parse_chat_message(
        &json!({"role":"tool","content":"x","metadata":{"extensions":{
            "chisel/tool_call_content": {"c1": {"content": [{"type":"text"}]}},
            "chisel/tool_result_meta": {}
        }}}),
        &Value::Null,
        1,
        None,
        0.0,
    );
    assert!(parsed.tool_result.is_none());
}

#[test]
fn rendered_tool_call_without_diffs_omits_the_content_entry() {
    // The `if !tc.diffs.is_empty()` false arm — a rendered call with no
    // diffs gets no `content` key in the extension entry.
    let mut asst = node(Role::Assistant);
    asst.metadata = json!({"summarized_from": "x"});
    asst.tool_calls = vec![ToolCall {
        id: "tc-plain".into(),
        name: "read".into(),
        arguments: json!({"file_path": "/a.rs"}),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: Vec::new(),
        diffs: Vec::new(),
    }];
    let m = build_chat_message(&asst, "m1");
    let ext = &m["metadata"]["extensions"]["chisel/tool_call_content"];
    assert!(ext["tc-plain"].is_object(), "{ext}");
    assert!(ext["tc-plain"].get("content").is_none(), "{ext}");
}
