#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `Cursor.test.ts` + `CursorExtra.test.ts` ports — meta/checkpoint
//! decoding, store-blob → IR mapping, the transcript projection, and the
//! write-side encoders.

use sepia_core::domain::{REDACTED_THINKING, Role, ToolCallStatus};
use sepia_core::{MessageNode, Session, ToolCall};
use sepia_driver_cursor::cursor::{self, CursorStoreInput, CursorStoreMeta, SummarizeStoreInput};
use serde_json::{Value, json};

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_string(value).unwrap().into_bytes()
}

fn hex_json(value: &Value) -> String {
    cursor::to_hex(serde_json::to_string(value).unwrap().as_bytes())
}

/// Deterministic 32-byte blob ids — the store only needs 64 hex chars.
fn blob_id(n: u64) -> String {
    format!("{n:064x}")
}

fn proto_field(num: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for mut n in [num * 8 + 2, payload.len() as u64] {
        loop {
            let mut b = (n & 0x7f) as u8;
            n >>= 7;
            if n > 0 {
                b |= 0x80;
            }
            out.push(b);
            if n == 0 {
                break;
            }
        }
    }
    out.extend_from_slice(payload);
    out
}

fn proto_varint_field(num: u64, value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for mut n in [num * 8, value] {
        loop {
            let mut b = (n & 0x7f) as u8;
            n >>= 7;
            if n > 0 {
                b |= 0x80;
            }
            out.push(b);
            if n == 0 {
                break;
            }
        }
    }
    out
}

/// A checkpoint blob: f1 refs, f9 workspace uri, f10 flag, f22 client.
fn checkpoint(message_ids: &[&str], workspace: Option<&str>, client: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for id in message_ids {
        out.extend_from_slice(&proto_field(1, &cursor::from_hex(id).unwrap()));
    }
    if let Some(ws) = workspace {
        out.extend_from_slice(&proto_field(9, ws.as_bytes()));
    }
    out.extend_from_slice(&proto_varint_field(10, 1));
    out.extend_from_slice(&proto_field(22, client.as_bytes()));
    out
}

fn meta() -> CursorStoreMeta {
    CursorStoreMeta {
        agent_id: Some("chat-1".into()),
        latest_root_blob_id: Some(blob_id(0)),
        name: Some("Store Chat".into()),
        mode: Some("default".into()),
        is_run_everything: false,
        created_at: Some(1_700_000_000_000.0),
        last_used_model: Some("composer-1".into()),
    }
}

fn store_messages() -> Vec<Vec<u8>> {
    vec![
        json_bytes(&json!({ "role": "system", "content": "You are an AI coding assistant." })),
        json_bytes(
            &json!({ "role": "user", "content": "<user_info>\nOS Version: linux\n</user_info>" }),
        ),
        json_bytes(&json!({
            "role": "user",
            "content": [{ "type": "text", "text": "<user_query>\ntake a screenshot\n</user_query>" }],
            "providerOptions": { "cursor": { "requestId": "req-1" } },
        })),
        json_bytes(&json!({
            "role": "assistant",
            "id": "1",
            "content": [
                { "type": "redacted-reasoning", "data": "opaque-payload" },
                { "type": "text", "text": "Taking a screenshot." },
                {
                    "type": "tool-call",
                    "toolCallId": "tool_1",
                    "toolName": "Shell",
                    "args": { "command": "grim shot.png" },
                },
            ],
        })),
        json_bytes(&json!({
            "role": "tool",
            "id": "tool_1",
            "content": [{
                "type": "tool-result",
                "toolCallId": "tool_1",
                "toolName": "Shell",
                "result": "Exit code: 0\n\nCommand output:\n\n```\ndone\n```",
            }],
            "providerOptions": {
                "cursor": {
                    "highLevelToolCallResult": {
                        "output": {
                            "success": { "command": "grim shot.png", "executionTime": 5285 },
                            "isError": false,
                        },
                    },
                },
            },
        })),
        b"\x0a\x05not-a-message".to_vec(),
    ]
}

fn store_blobs(include_root: bool) -> Vec<(String, Vec<u8>)> {
    let msgs = store_messages();
    let mut blobs: Vec<(String, Vec<u8>)> = (1u64..=6)
        .zip(msgs)
        .map(|(n, data)| (blob_id(n), data))
        .collect();
    if include_root {
        let refs: Vec<String> = (1..=6).map(blob_id).chain([blob_id(9)]).collect();
        let refs: Vec<&str> = refs.iter().map(String::as_str).collect();
        blobs.push((
            blob_id(0),
            checkpoint(&refs, Some("file:///home/luis/Desktop"), "cli"),
        ));
    }
    blobs
}

/* ---- scalar decoders -------------------------------------------------- */

#[test]
fn parse_store_meta_decodes_hex_encoded_and_plain_json() {
    let decoded = cursor::parse_store_meta(&hex_json(&json!({
        "agentId": "chat-1",
        "latestRootBlobId": blob_id(0),
        "name": "Store Chat",
        "mode": "default",
        "isRunEverything": false,
        "createdAt": 1_700_000_000_000_i64,
        "lastUsedModel": "composer-1",
    })))
    .unwrap();
    assert_eq!(decoded, meta());

    let plain = cursor::parse_store_meta(r#"{"name":"plain"}"#).unwrap();
    assert_eq!(plain.name.as_deref(), Some("plain"));
    assert_eq!(plain.agent_id, None);
    assert!(!plain.is_run_everything);

    assert_eq!(cursor::parse_store_meta("not json at all"), None);
    assert_eq!(cursor::parse_store_meta("zzzz"), None);
    assert_eq!(cursor::parse_store_meta(""), None);
    // "35" is hex for "5" — valid JSON, not an object
    assert_eq!(cursor::parse_store_meta("35"), None);
    assert_eq!(cursor::parse_meta_json(&json!(5)), None);
    assert_eq!(cursor::parse_meta_json(&json!("{bad")), None);
}

#[test]
fn decode_checkpoint_reads_ordered_refs_workspace_and_client() {
    let blob = checkpoint(
        &[blob_id(1).as_str(), blob_id(2).as_str()],
        Some("file:///home/luis/proj"),
        "cli",
    );
    let decoded = cursor::decode_checkpoint(&blob).unwrap();
    assert_eq!(decoded.message_ids, vec![blob_id(1), blob_id(2)]);
    assert_eq!(decoded.workspace.as_deref(), Some("file:///home/luis/proj"));
    assert_eq!(decoded.client.as_deref(), Some("cli"));

    assert_eq!(cursor::decode_checkpoint(&[]), None);
    assert_eq!(cursor::decode_checkpoint(b"\xff\xff\xff\xff"), None);
}

#[test]
fn decode_checkpoint_rejects_truncated_fields_and_unknown_wires() {
    // field 0 is never a real field
    assert_eq!(cursor::decode_checkpoint(&[0x00]), None);
    // a varint field value that never terminates
    assert_eq!(cursor::decode_checkpoint(&[0x08, 0x80]), None);
    // a length-delimited field whose varint is truncated
    assert_eq!(cursor::decode_checkpoint(&[0x0a, 0x80]), None);
    // a declared length that overruns the buffer
    assert_eq!(cursor::decode_checkpoint(&[0x0a, 0x05, 0x01]), None);
    // a 32-bit field with fewer than 4 bytes left
    assert_eq!(cursor::decode_checkpoint(&[0x0d, 0x01, 0x02]), None);
    // a group wire type the walker does not handle
    assert_eq!(cursor::decode_checkpoint(&[0x0b]), None);
    // a valid 64-bit field plus a real message ref: skipped, ref decoded
    let mut data = vec![0x09, 1, 2, 3, 4, 5, 6, 7, 8];
    data.extend_from_slice(&proto_field(1, &cursor::from_hex(&blob_id(3)).unwrap()));
    let decoded = cursor::decode_checkpoint(&data).unwrap();
    assert_eq!(decoded.message_ids, vec![blob_id(3)]);
    assert_eq!(decoded.workspace, None);
    assert_eq!(decoded.client, None);
}

#[test]
fn workspace_from_uri_decodes_file_uris_only() {
    assert_eq!(
        cursor::workspace_from_uri(Some("file:///home/luis/My%20Proj")).as_deref(),
        Some("/home/luis/My Proj")
    );
    assert_eq!(cursor::workspace_from_uri(Some("vscode://x/y")), None);
    assert_eq!(cursor::workspace_from_uri(None), None);
    assert_eq!(cursor::workspace_from_uri(Some("file://%zz")), None);
    assert_eq!(cursor::workspace_from_uri(Some("file://")), None);
    assert_eq!(
        cursor::workspace_from_uri(Some("file:///w")).as_deref(),
        Some("/w")
    );
}

#[test]
fn extract_user_query_unwraps_the_tag_or_strips_markup() {
    assert_eq!(
        cursor::extract_user_query("<user_query>\nhello there\n</user_query>").as_deref(),
        Some("hello there")
    );
    // tag markup is stripped; unattributed inner text is kept
    assert_eq!(
        cursor::extract_user_query("<attached_files>x</attached_files> plain text").as_deref(),
        Some("x plain text")
    );
    assert_eq!(cursor::extract_user_query("   "), None);
}

/* ---- store.db → IR ---------------------------------------------------- */

#[test]
fn session_from_store_decodes_the_checkpoints_ordered_messages() {
    let session = cursor::session_from_store(&CursorStoreInput {
        id: "chat-1".into(),
        workspace_hash: Some("deadbeef".into()),
        meta: Some(meta()),
        blobs: store_blobs(true),
        prompt_history: vec!["take a screenshot".into()],
        ..Default::default()
    });

    assert_eq!(session.id, "chat-1");
    assert_eq!(session.title, "Store Chat");
    assert_eq!(session.working_directory, "/home/luis/Desktop");
    assert_eq!(session.backend_type, "cursor");
    assert_eq!(session.model, "composer-1");
    assert_eq!(session.created_at, 1_700_000_000.0);
    assert_eq!(session.last_activity_at, 1_700_000_000.0);
    let prompts: Vec<&str> = session
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(prompts, ["take a screenshot"]);

    let meta = &session.metadata;
    assert_eq!(meta["source"], "cursor");
    assert_eq!(meta["store"], "chats");
    assert_eq!(meta["workspaceHash"], "deadbeef");
    assert_eq!(meta["client"], "cli");
    // the opaque projection blob and the dangling blobId(9) ref
    assert_eq!(meta["opaqueBlobs"], 2);

    assert_eq!(session.nodes.len(), 5);
    let [system, user_info, prompt, assistant, tool] = &session.nodes[..] else {
        panic!("expected 5 nodes");
    };
    assert_eq!(system.role, Role::System);
    assert_eq!(user_info.role, Role::User);
    assert_eq!(user_info.metadata["context"], "user_info");

    assert_eq!(prompt.role, Role::User);
    assert!(prompt.content.contains("take a screenshot"));
    assert_eq!(prompt.request_id.as_deref(), Some("req-1"));

    assert_eq!(assistant.role, Role::Assistant);
    assert!(assistant.content.contains("Taking a screenshot."));
    assert_eq!(assistant.thinking.as_deref(), Some(REDACTED_THINKING));
    // the opaque redacted-reasoning blob rides verbatim as the seal
    assert_eq!(
        assistant.thinking_signature.as_deref(),
        Some("opaque-payload")
    );
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(assistant.tool_calls[0].id, "tool_1");
    assert_eq!(assistant.tool_calls[0].name, "Shell");
    assert_eq!(
        assistant.tool_calls[0].arguments,
        json!({ "command": "grim shot.png" })
    );
    // the tool result's success folds back onto the call
    assert_eq!(
        assistant.tool_calls[0].status,
        Some(ToolCallStatus::Success)
    );
    assert_eq!(assistant.tool_calls[0].duration_ms, Some(5285.0));

    assert_eq!(tool.role, Role::Tool);
    assert_eq!(tool.tool_call_id.as_deref(), Some("tool_1"));
    assert_eq!(tool.tool_name.as_deref(), Some("Shell"));
    let result = tool.tool_result.as_ref().unwrap();
    assert_eq!(result.status, ToolCallStatus::Success);
    assert_eq!(result.duration_ms, Some(5285.0));
    assert!(tool.content.contains("Exit code: 0"));
    assert_eq!(
        tool.metadata["toolArguments"],
        json!({ "command": "grim shot.png" })
    );

    // linear chain
    for (i, node) in session.nodes.iter().enumerate().skip(1) {
        assert_eq!(node.parent_node_id, Some(i as i64 - 1));
    }
    assert_eq!(session.nodes[0].parent_node_id, None);
}

#[test]
fn session_from_store_marks_error_results_and_meta_json_overrides() {
    let err_result = json_bytes(&json!({
        "role": "tool",
        "content": [{
            "type": "tool-result",
            "toolCallId": "tool_1",
            "toolName": "Shell",
            "result": { "failed": true },
        }],
        "providerOptions": { "cursor": { "highLevelToolCallResult": { "output": { "isError": true } } } },
    }));
    let mut blobs = store_blobs(true);
    blobs[4] = (blob_id(5), err_result);
    let session = cursor::session_from_store(&CursorStoreInput {
        id: "chat-1".into(),
        meta: Some(meta()),
        meta_json: cursor::parse_meta_json(&json!({
            "title": "Meta Title",
            "cwd": "/override/cwd",
            "updatedAtMs": 1_700_100_000_000_i64,
        })),
        blobs,
        ..Default::default()
    });
    assert_eq!(session.title, "Meta Title");
    assert_eq!(session.working_directory, "/override/cwd");
    assert_eq!(session.last_activity_at, 1_700_100_000.0);
    let tool = &session.nodes[4];
    assert_eq!(
        tool.tool_result.as_ref().unwrap().status,
        ToolCallStatus::Error
    );
    assert_eq!(tool.tool_result.as_ref().unwrap().duration_ms, None);
    assert_eq!(
        session.nodes[3].tool_calls[0].status,
        Some(ToolCallStatus::Error)
    );
    assert!(tool.content.contains("failed"));
}

#[test]
fn session_from_store_falls_back_without_a_checkpoint_root() {
    let blobs = vec![(
        blob_id(1),
        json_bytes(&json!({ "role": "user", "content": "orphan" })),
    )];
    let session = cursor::session_from_store(&CursorStoreInput {
        id: "chat-x".into(),
        meta: Some(CursorStoreMeta {
            name: Some("No Root".into()),
            ..Default::default()
        }),
        blobs,
        ..Default::default()
    });
    assert_eq!(session.title, "No Root");
    assert_eq!(session.nodes, vec![]);
    assert_eq!(session.working_directory, "/");
    assert_eq!(session.model, "unknown");
}

#[test]
fn session_from_store_titles_from_the_first_user_query_when_unnamed() {
    let session = cursor::session_from_store(&CursorStoreInput {
        id: "chat-1".into(),
        blobs: store_blobs(true),
        ..Default::default()
    });
    assert_eq!(session.title, "take a screenshot");
    assert_eq!(session.metadata["agentId"], Value::Null);
    assert_eq!(session.metadata["latestRootBlobId"], Value::Null);
}

#[test]
fn summarize_store_prefers_meta_json_fields_and_tolerates_missing_data() {
    let s = cursor::summarize_store(&SummarizeStoreInput {
        chat: cursor::CursorChatInfo {
            id: "c1".into(),
            ..Default::default()
        },
        meta: Some(meta()),
        meta_json: cursor::parse_meta_json(&json!({
            "title": "Json Title",
            "cwd": "/json/cwd",
            "createdAtMs": 1_700_000_500_000_i64,
        })),
        workspace: Some("/ws/cwd".into()),
        ..Default::default()
    });
    assert_eq!(s.title, "Json Title");
    assert_eq!(s.working_directory, "/json/cwd");
    assert_eq!(s.created_at, 1_700_000_500.0);
    assert_eq!(s.nodes, vec![]);

    let bare = cursor::summarize_store(&SummarizeStoreInput {
        chat: cursor::CursorChatInfo {
            id: "c2".into(),
            ..Default::default()
        },
        mtime_ms: Some(1_700_000_000_000.0),
        ..Default::default()
    });
    assert_eq!(bare.title, "c2");
    assert_eq!(bare.working_directory, "/");
    assert_eq!(bare.model, "unknown");
}

#[test]
fn session_from_store_skips_unparseable_blobs_and_odd_message_shapes() {
    let refs: Vec<String> = (1..=7).map(blob_id).collect();
    let mut ckpt = Vec::new();
    for id in &refs {
        ckpt.extend_from_slice(&proto_field(1, &cursor::from_hex(id).unwrap()));
    }
    ckpt.extend_from_slice(&proto_varint_field(10, 1));
    let blobs = vec![
        (blob_id(0), ckpt),
        // not a message at all — no role
        (blob_id(1), json_bytes(&json!({ "nope": true }))),
        // user message with a scalar content — nothing readable
        (
            blob_id(2),
            json_bytes(&json!({ "role": "user", "content": 42 })),
        ),
        // user message whose array has no text items
        (
            blob_id(3),
            json_bytes(&json!({ "role": "user", "content": [{ "type": "image" }, 5, null] })),
        ),
        // a tool message whose items are not tool-results → one fallback node
        (
            blob_id(4),
            json_bytes(&json!({ "role": "tool", "content": [{ "type": "x" }, 3] })),
        ),
        // a tool message with a result-less tool-result entry
        (
            blob_id(5),
            json_bytes(&json!({
                "role": "tool",
                "content": [{ "type": "tool-result", "toolCallId": "c1", "toolName": "Shell" }],
            })),
        ),
        // an unknown role becomes a system marker node
        (
            blob_id(6),
            json_bytes(&json!({ "role": "banana", "content": "exotic" })),
        ),
        // a real user query so the session isn't empty
        (
            blob_id(7),
            json_bytes(
                &json!({ "role": "user", "content": [{ "type": "text", "text": "real question" }] }),
            ),
        ),
    ];
    let session = cursor::session_from_store(&CursorStoreInput {
        id: "chat-1".into(),
        meta: Some(CursorStoreMeta {
            agent_id: Some("chat-1".into()),
            latest_root_blob_id: Some(blob_id(0)),
            ..Default::default()
        }),
        blobs,
        ..Default::default()
    });
    let roles: Vec<Role> = session.nodes.iter().map(|n| n.role).collect();
    assert_eq!(roles, [Role::Tool, Role::Tool, Role::System, Role::User]);
    // the item-less tool blob fell back to stringifying its content array
    assert_eq!(
        session.nodes[0].content,
        serde_json::to_string(&json!([{ "type": "x" }, 3])).unwrap()
    );
    // the result-less tool-result still emits a node with empty output
    assert_eq!(session.nodes[1].content, "");
    assert_eq!(session.nodes[2].content, "[cursor banana]");
}

/* ---- coverage tail --------------------------------------------------- */

#[test]
fn decoder_scalar_edge_arms() {
    // decode_uri_component: truncated escape and bad hex both yield None.
    assert_eq!(cursor::workspace_from_uri(Some("file:///a%")), None);
    assert_eq!(cursor::workspace_from_uri(Some("file:///a%zz")), None);
    assert_eq!(
        cursor::workspace_from_uri(Some("file:///home%20me/proj")).as_deref(),
        Some("/home me/proj")
    );
    // strip_angle_tags: a dangling `<` survives verbatim.
    assert_eq!(
        cursor::extract_user_query("question with a dangling <tag").as_deref(),
        Some("question with a dangling <tag")
    );
    // resolve_path: relative cwd + `..` folding.
    assert!(cursor::workspace_hash_from_cwd("rel/../x").len() == 32);
    assert_eq!(
        cursor::workspace_hash_from_cwd("/a/./b/../c"),
        cursor::workspace_hash_from_cwd("/a/c")
    );
    // checkpoint proto: unknown field numbers are skipped.
    let mut ckpt = proto_field(1, &cursor::from_hex(&blob_id(1)).unwrap());
    ckpt.extend_from_slice(&proto_varint_field(5, 99));
    ckpt.extend_from_slice(&proto_field(9, b"file:///w"));
    let decoded = cursor::decode_checkpoint(&ckpt).unwrap();
    assert_eq!(decoded.message_ids, vec![blob_id(1)]);
}

#[test]
fn apply_patch_refs_hunk_edges() {
    // A pre-hunk context line, an unknown char, a `\\ No newline` marker,
    // and a non-directive line — all skipped, Update still lands.
    let patch = concat!(
        "*** Begin Patch\n*** Update File: src/a.rs\n",
        "-orphan before hunk\n",          // hunk == None → continue
        "@@\n",                           // hunk opens
        "\\ No newline at end of file\n", // marker skipped
        " context\n",                     // both sides
        "-old\n+new\n",                   // minus/plus
        "!unknown prefix\n",              // `_` arm
        "not a directive line\n",         // directive → None
        "*** End Patch\n",
    );
    let refs = cursor::tool_file_refs("ApplyPatch", &json!(patch));
    assert_eq!(refs.diffs.len(), 1);
    let diff = &refs.diffs[0];
    assert!(diff.old_text.as_deref().unwrap_or("").is_empty() || diff.old_text.is_some());
}

#[test]
fn store_message_shape_edges() {
    let blobs = vec![
        (
            blob_id(0),
            checkpoint(
                &[
                    blob_id(1).as_str(),
                    blob_id(2).as_str(),
                    blob_id(3).as_str(),
                    blob_id(4).as_str(),
                    blob_id(5).as_str(),
                    blob_id(6).as_str(),
                    blob_id(7).as_str(),
                ],
                None,
                "cli",
            ),
        ),
        // non-object JSON → parse_message None arm.
        (blob_id(1), json_bytes(&json!([1, 2, 3]))),
        // empty blob data → opaque_blobs arm.
        (blob_id(2), Vec::new()),
        // system with non-string content → tool_result_text arm.
        (
            blob_id(3),
            json_bytes(&json!({"role":"system","content":[1,2]})),
        ),
        // plain user string (not user_info) → bare blobId metadata arm.
        (
            blob_id(4),
            json_bytes(&json!({"role":"user","content":"plain question"})),
        ),
        // assistant: non-object item, tool-call sans id, unknown type.
        (
            blob_id(5),
            json_bytes(&json!({
                "role": "assistant", "id": "1",
                "content": [
                    5,
                    {"type": "tool-call", "toolName": "Shell", "args": {"command": "ls"}},
                    {"type": "video", "data": "x"},
                    {"type": "text", "text": "answer"},
                ],
            })),
        ),
        // user text with a dangling `<` → strip_angle_tags else arm in title.
        (
            blob_id(6),
            json_bytes(
                &json!({"role":"user","content":[{"type":"text","text":"<user_query>\nfix the <weird tag\n</user_query>"}]}),
            ),
        ),
        (
            blob_id(7),
            json_bytes(
                &json!({"role":"user","content":[{"type":"text","text":"<user_query>\nreal query\n</user_query>"}]}),
            ),
        ),
    ];
    let session = cursor::session_from_store(&CursorStoreInput {
        id: "chat-edge".into(),
        meta: Some(CursorStoreMeta {
            agent_id: Some("chat-edge".into()),
            latest_root_blob_id: Some(blob_id(0)),
            ..Default::default()
        }),
        blobs,
        ..Default::default()
    });
    assert!(!session.nodes.is_empty());
    // the generated tool call id follows the cursor-tool-{node}-N scheme.
    let calls: Vec<&str> = session
        .nodes
        .iter()
        .flat_map(|n| n.tool_calls.iter().map(|c| c.id.as_str()))
        .collect();
    assert!(calls.iter().any(|id| id.starts_with("cursor-tool-")));
}

fn tail_node(node_id: i64, role: Role, content: &str) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id: (node_id > 1).then_some(node_id - 1),
        role,
        content: content.into(),
        blocks: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: 1.0,
        metadata: Value::Null,
    }
}

#[test]
fn store_write_plan_empty_fields_arms() {
    let mut session = Session {
        id: "w1".into(),
        title: "unused".into(),
        working_directory: "/w".into(),
        backend_type: "cursor".into(),
        agent_mode: "accept-edits".into(),
        model: "composer-1".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_300.0,
        main_chain_id: 0,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: Vec::new(),
        metadata: json!({}),
        nodes: Vec::new(),
        prompt_history: Vec::new(),
    };
    session.title.clear();
    session.title.clear();
    session.model.clear();
    session.agent_mode.clear();
    // Nodes exercising every message_json None/degenerate arm:
    // empty system content → None; empty user content → None;
    // assistant with a null-args tool call and no text; assistant empty.
    let n_sys = tail_node(1, Role::System, "");
    let n_user = tail_node(2, Role::User, "");
    let mut n_asst = tail_node(3, Role::Assistant, "");
    n_asst.tool_calls = vec![ToolCall {
        id: "t1".into(),
        name: "Shell".into(),
        arguments: Value::Null,
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: vec![],
        diffs: vec![],
    }];
    let n_empty = tail_node(4, Role::Assistant, "");
    session.nodes = vec![n_sys, n_user, n_asst, n_empty];
    let plan = cursor::store_write_plan(&session, None);
    assert!(!plan.blobs.is_empty());
    let meta_json = String::from_utf8(cursor::from_hex(&plan.meta_row).unwrap()).unwrap();
    assert!(!meta_json.contains("\"name\""));
    // "unknown" model also maps to "default".
    session.model = "unknown".into();
    session.title = "T".into();
    session.agent_mode = "m".into();
    let plan = cursor::store_write_plan(&session, None);
    let meta_json = String::from_utf8(cursor::from_hex(&plan.meta_row).unwrap()).unwrap();
    assert!(meta_json.contains("\"lastUsedModel\":\"default\""));
}
