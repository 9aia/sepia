#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Transcript projection and write-side encoder tests — the second half of
//! the `Cursor.test.ts` / `CursorExtra.test.ts` port.

use sepia_core::domain::{
    MessageNode, REDACTED_THINKING, Role, Session, ToolCall, ToolCallDiff, ToolCallLocation,
};
use sepia_driver_cursor::cursor::{
    self, CheckpointWriteInput, CursorStoreMeta, CursorTranscriptSource, MetaJsonWriteInput,
    StoreMetaWriteInput,
};
use serde_json::{Value, json};

fn blob_id(n: u64) -> String {
    format!("{n:064x}")
}

fn make_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    role: Role,
    content: &str,
    metadata: Value,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
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
        metadata,
    }
}

fn tool_call(id: &str, name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: Vec::new(),
        diffs: Vec::new(),
    }
}

fn make_session(id: &str, nodes: Vec<MessageNode>) -> Session {
    Session {
        id: id.into(),
        title: "t".into(),
        working_directory: "/w".into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: "m".into(),
        created_at: 1.0,
        last_activity_at: 2.0,
        main_chain_id: nodes.len() as i64 - 1,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: Vec::new(),
        metadata: Value::Null,
        nodes,
        prompt_history: Vec::new(),
    }
}

fn loc(path: &str) -> ToolCallLocation {
    ToolCallLocation {
        path: path.into(),
        line: None,
    }
}

/* ---- transcript projection ------------------------------------------- */

fn transcript_fixture() -> String {
    [
        serde_json::to_string(&json!({
            "role": "user",
            "message": { "content": [{ "type": "text", "text": "<user_query>\nexplore the repo\n</user_query>" }] },
        }))
        .unwrap(),
        serde_json::to_string(&json!({
            "role": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "I'll look around.\n\n[REDACTED]" },
                    { "type": "tool_use", "name": "Glob", "input": { "glob_pattern": "src/**" } },
                ],
            },
        }))
        .unwrap(),
        r#"{"role":"assistant" BAD"#.to_string(),
        serde_json::to_string(&json!({
            "type": "turn_ended",
            "status": "error",
            "error": { "message": "rate limited" },
        }))
        .unwrap(),
    ]
    .join("\n")
}

#[test]
fn from_transcript_jsonl_maps_the_lossy_projection_honestly() {
    let session = cursor::from_transcript_jsonl(
        &transcript_fixture(),
        &CursorTranscriptSource {
            id: "chat-9".into(),
            project_slug: "home-luis-Desktop-cheloni-v4".into(),
            mtime_ms: Some(1_700_000_000_000.0),
            ..Default::default()
        },
    );
    assert_eq!(session.id, "chat-9");
    assert_eq!(session.title, "explore the repo");
    assert_eq!(session.working_directory, "/home/luis/Desktop/cheloni/v4");
    assert_eq!(session.backend_type, "cursor");
    assert_eq!(session.created_at, 1_700_000_000.0);
    assert_eq!(session.metadata["store"], "transcript");
    assert_eq!(session.metadata["lossy"], true);
    assert_eq!(
        session.metadata["turnErrors"],
        json!([{ "message": "rate limited" }])
    );

    assert_eq!(session.nodes.len(), 2);
    let [user, assistant] = &session.nodes[..] else {
        panic!("expected 2 nodes");
    };
    assert_eq!(user.role, Role::User);
    assert_eq!(assistant.content, "I'll look around.");
    assert_eq!(assistant.thinking.as_deref(), Some(REDACTED_THINKING));
    // the lossy projection keeps no blob — marker only, no signature
    assert_eq!(assistant.thinking_signature, None);
    assert_eq!(assistant.tool_calls[0].name, "Glob");
    assert_eq!(
        assistant.tool_calls[0].arguments,
        json!({ "glob_pattern": "src/**" })
    );
    assert!(assistant.tool_calls[0].id.starts_with("cursor-tool-"));
    let prompts: Vec<&str> = session
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(prompts, ["explore the repo"]);
}

#[test]
fn transcript_subagent_source_records_the_parent_chat() {
    let session = cursor::from_transcript_jsonl(
        &transcript_fixture(),
        &CursorTranscriptSource {
            id: "sub-1".into(),
            project_slug: "home-luis-proj".into(),
            parent_session_id: Some("chat-9".into()),
            ..Default::default()
        },
    );
    assert_eq!(session.parent_session_id.as_deref(), Some("chat-9"));

    let summary = cursor::summarize_transcript_jsonl(
        &transcript_fixture(),
        &CursorTranscriptSource {
            id: "sub-1".into(),
            project_slug: "home-luis-proj".into(),
            parent_session_id: Some("chat-9".into()),
            ..Default::default()
        },
    );
    assert_eq!(summary.title, "explore the repo");
    assert_eq!(summary.nodes, vec![]);
}

#[test]
fn transcript_file_edit_tool_calls_carry_locations_and_revertable_diffs() {
    let raw = [
        json!({ "role": "user", "message": { "content": [{ "type": "text", "text": "change it" }] } }),
        json!({
            "role": "assistant",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "id": "t1",
                        "name": "StrReplace",
                        "input": { "path": "/w/a.ts", "old_string": "o", "new_string": "n" },
                    },
                    {
                        "type": "tool_use",
                        "id": "t2",
                        "name": "Write",
                        "input": { "path": "/w/b.ts", "contents": "made" },
                    },
                    { "type": "tool_use", "id": "t3", "name": "Delete", "input": { "path": "/w/c.ts" } },
                    {
                        "type": "tool_use",
                        "id": "t4",
                        "name": "Glob",
                        "input": { "target_directory": "/w", "glob_pattern": "**/*" },
                    },
                    {
                        "type": "tool_use",
                        "id": "t5",
                        "name": "ApplyPatch",
                        "input": "*** Begin Patch\n*** Update File: /w/d.ts\n@@\n ctx\n-old\n+new\n*** Add File: /w/e.ts\n+whole\n+file\n*** Delete File: /w/f.ts\n*** End Patch",
                    },
                ],
            },
        }),
    ]
    .iter()
    .map(|l| serde_json::to_string(l).unwrap())
    .collect::<Vec<_>>()
    .join("\n");
    let session = cursor::from_transcript_jsonl(
        &raw,
        &CursorTranscriptSource {
            id: "c1".into(),
            project_slug: "w".into(),
            ..Default::default()
        },
    );
    let [str_replace, write, del, glob, patch] = &session.nodes[1].tool_calls[..] else {
        panic!("expected 5 tool calls");
    };
    assert_eq!(str_replace.locations, vec![loc("/w/a.ts")]);
    assert_eq!(
        str_replace.diffs,
        vec![ToolCallDiff {
            path: "/w/a.ts".into(),
            old_text: Some("o".into()),
            new_text: Some("n".into()),
        }]
    );
    assert_eq!(
        write.diffs,
        vec![ToolCallDiff {
            path: "/w/b.ts".into(),
            old_text: None,
            new_text: Some("made".into()),
        }]
    );
    // a delete records the path it removed — nothing to revert with
    assert_eq!(del.locations, vec![loc("/w/c.ts")]);
    assert_eq!(del.diffs, vec![]);
    assert_eq!(glob.locations, vec![loc("/w")]);
    assert_eq!(
        patch.diffs,
        vec![
            ToolCallDiff {
                path: "/w/d.ts".into(),
                old_text: Some("ctx\nold".into()),
                new_text: Some("ctx\nnew".into()),
            },
            ToolCallDiff {
                path: "/w/e.ts".into(),
                old_text: None,
                new_text: Some("whole\nfile".into()),
            },
            // a delete section with no `-` payload keeps the change on record
            ToolCallDiff {
                path: "/w/f.ts".into(),
                old_text: None,
                new_text: None,
            },
        ]
    );
}

#[test]
fn tool_file_refs_covers_renames_deletes_sentinel_lines_and_edge_args() {
    let refs = cursor::tool_file_refs(
        "ApplyPatch",
        &json!(
            "@@ stray before any section\n*** Begin Patch\n*** Update File: /w/old.ts\n*** Move to: /w/new.ts\n@@ class Foo\n-x\n+y\n\\ No newline at end of file\n*** Delete File: /w/gone.ts\n-gone body\n*** End Patch"
        ),
    );
    assert_eq!(
        refs.diffs,
        vec![
            // hunks land on the rename target — the rename itself is no diff
            ToolCallDiff {
                path: "/w/new.ts".into(),
                old_text: Some("x".into()),
                new_text: Some("y".into()),
            },
            // a delete's `-` payload is the only content that can resurrect it
            ToolCallDiff {
                path: "/w/gone.ts".into(),
                old_text: Some("gone body".into()),
                new_text: None,
            },
        ]
    );
    assert_eq!(
        refs.locations,
        vec![loc("/w/old.ts"), loc("/w/new.ts"), loc("/w/gone.ts")]
    );

    // args that carry no patch text
    assert_eq!(
        cursor::tool_file_refs("ApplyPatch", &json!({})),
        cursor::ToolFileRefs::default()
    );
    assert_eq!(
        cursor::tool_file_refs("ApplyPatch", &json!(42)),
        cursor::ToolFileRefs::default()
    );
    // non-object args to a regular tool
    assert_eq!(
        cursor::tool_file_refs("Read", &json!("raw")),
        cursor::ToolFileRefs::default()
    );
    // hunk tools without a path record nothing
    assert_eq!(
        cursor::tool_file_refs("StrReplace", &json!({ "old_string": "a" })).diffs,
        Vec::<ToolCallDiff>::new()
    );
    // one-sided and string-free calls stay honest about what was recorded
    assert_eq!(
        cursor::tool_file_refs(
            "StrReplace",
            &json!({ "path": "/w/a.ts", "new_string": "n" })
        )
        .diffs,
        vec![ToolCallDiff {
            path: "/w/a.ts".into(),
            old_text: None,
            new_text: Some("n".into()),
        }]
    );
    assert_eq!(
        cursor::tool_file_refs("StrReplace", &json!({ "path": "/w/a.ts" })).diffs,
        Vec::<ToolCallDiff>::new()
    );
    // Write tolerates both `contents` and `content`, but needs one of them
    assert_eq!(
        cursor::tool_file_refs("Write", &json!({ "path": "/w/b.ts", "content": "x" })).diffs,
        vec![ToolCallDiff {
            path: "/w/b.ts".into(),
            old_text: None,
            new_text: Some("x".into()),
        }]
    );
    assert_eq!(
        cursor::tool_file_refs("Write", &json!({ "path": "/w/b.ts" })).diffs,
        Vec::<ToolCallDiff>::new()
    );
    // path lists contribute locations; junk entries drop
    assert_eq!(
        cursor::tool_file_refs(
            "SemanticSearch",
            &json!({ "target_directories": ["/w", "", 42] })
        )
        .locations,
        vec![loc("/w")]
    );
}

#[test]
fn empty_and_malformed_transcripts_yield_a_bare_session() {
    let session = cursor::from_transcript_jsonl(
        "\nnot json\n",
        &CursorTranscriptSource {
            id: "t".into(),
            project_slug: "home-x".into(),
            ..Default::default()
        },
    );
    assert_eq!(session.nodes, vec![]);
    assert_eq!(session.title, "t");
    assert_eq!(session.prompt_history, vec![]);
}

#[test]
fn from_transcript_jsonl_skips_empty_users_junk_items_and_empty_assistants() {
    let tl = |entry: &Value| serde_json::to_string(entry).unwrap();
    let raw = [
        tl(&json!({ "role": "user", "message": { "content": "" } })),
        tl(&json!({ "role": "user", "message": { "content": [{ "type": "text", "text": "  " }] } })),
        // markup-only text extracts no query — dropped from history entirely
        tl(&json!({ "role": "user", "message": { "content": [{ "type": "text", "text": "<br>" }] } })),
        tl(&json!({ "role": "user", "message": { "content": [{ "type": "text", "text": "real" }] } })),
        // assistant items that are not objects are skipped
        tl(&json!({
            "role": "assistant",
            "message": { "content": ["junk", { "type": "text", "text": "plain text" }] },
        })),
        // an assistant entry with nothing usable emits no node
        tl(&json!({ "role": "assistant", "message": { "content": [{ "type": "image" }] } })),
        tl(&json!({ "role": "assistant", "message": { "content": "not an array" } })),
        // a turn-level error is recorded on the session metadata
        tl(&json!({ "type": "turn_ended", "status": "error", "error": { "message": "boom" } })),
    ]
    .join("\n");
    let session = cursor::from_transcript_jsonl(
        &raw,
        &CursorTranscriptSource {
            id: "t1".into(),
            project_slug: "home-x".into(),
            mtime_ms: Some(1_700_000_000_000.0),
            ..Default::default()
        },
    );
    // whitespace/markup-only user text still emits a node — the skip only
    // applies to prompt history, where no query can be extracted
    let roles: Vec<Role> = session.nodes.iter().map(|n| n.role).collect();
    assert_eq!(roles, [Role::User, Role::User, Role::User, Role::Assistant]);
    let contents: Vec<&str> = session.nodes.iter().map(|n| n.content.as_str()).collect();
    assert_eq!(contents, ["  ", "<br>", "real", "plain text"]);
    let prompts: Vec<&str> = session
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(prompts, ["real"]);
    assert_eq!(
        session.metadata["turnErrors"],
        json!([{ "message": "boom" }])
    );
}

/* ---- write path — agent-transcripts projection ------------------------ */

#[test]
fn project_slug_from_cwd_flattens_separators_like_the_real_layout() {
    assert_eq!(
        cursor::project_slug_from_cwd("/home/luis/Desktop/cheloni-v4"),
        "home-luis-Desktop-cheloni-v4"
    );
    assert_eq!(
        cursor::project_slug_from_cwd("/tmp/0b0ce061-35f7"),
        "tmp-0b0ce061-35f7"
    );
    // every non-alphanumeric flattens, including dots and spaces
    assert_eq!(
        cursor::project_slug_from_cwd("/w/my proj.v2"),
        "w-my-proj-v2"
    );
    assert_eq!(cursor::project_slug_from_cwd("/"), "root");
}

#[test]
fn to_transcript_jsonl_encodes_only_what_the_projection_carries() {
    let mut assistant = make_node(
        2,
        Some(1),
        Role::Assistant,
        "I'll look around.",
        Value::Null,
    );
    assistant.thinking = Some(REDACTED_THINKING.into());
    assistant.tool_calls = vec![tool_call(
        "call-1",
        "Glob",
        json!({ "glob_pattern": "src/**" }),
    )];
    let session = Session {
        title: "title lives nowhere in the projection".into(),
        nodes: vec![
            make_node(0, None, Role::System, "sys", Value::Null),
            make_node(1, None, Role::User, "explore the repo", Value::Null),
            assistant,
            make_node(3, Some(2), Role::Tool, "result text", Value::Null),
            make_node(4, Some(3), Role::User, "", Value::Null),
            make_node(5, Some(4), Role::Assistant, "", Value::Null),
        ],
        ..make_session("new-chat", vec![])
    };

    let out = cursor::to_transcript_jsonl(&session);
    let lines: Vec<&str> = out.trim().split('\n').collect();
    // system, tool-result and empty nodes have no slot in the projection
    assert_eq!(lines.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(lines[0]).unwrap(),
        json!({
            "role": "user",
            "message": { "content": [{ "type": "text", "text": "explore the repo" }] },
        })
    );
    assert_eq!(
        serde_json::from_str::<Value>(lines[1]).unwrap(),
        json!({
            "role": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "I'll look around.\n\n[REDACTED]" },
                    { "type": "tool_use", "id": "call-1", "name": "Glob", "input": { "glob_pattern": "src/**" } },
                ],
            },
        })
    );
}

#[test]
fn to_transcript_jsonl_marks_thinking_even_without_visible_text() {
    let mut assistant = make_node(0, None, Role::Assistant, "", Value::Null);
    assistant.thinking = Some("hidden reasoning".into());
    let session = Session {
        nodes: vec![assistant],
        ..make_session("s", vec![])
    };
    assert_eq!(
        serde_json::from_str::<Value>(cursor::to_transcript_jsonl(&session).trim()).unwrap(),
        json!({
            "role": "assistant",
            "message": { "content": [{ "type": "text", "text": "[REDACTED]" }] },
        })
    );
}

/* ---- write path — store.db encoders ----------------------------------- */

#[test]
fn blob_ids_and_workspace_hashes_match_the_real_content_addressing() {
    // sha256("") is the empty blob every real store keeps
    assert_eq!(
        cursor::blob_id_for(&[]),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    // md5 of the raw path — both values observed on a live ~/.cursor/chats
    assert_eq!(
        cursor::workspace_hash_from_cwd("/home/luis"),
        "bd2ea8ea8fb4de9e940176cfdc5cd7bd"
    );
    assert_eq!(
        cursor::workspace_hash_from_cwd("/home/luis/Desktop"),
        "e4f40720b3eb141b3f86d6db5f82b915"
    );
    // the file:// uri encodes each segment and decodes back to the path
    let uri = cursor::workspace_uri_from_cwd("/home/luis/My Proj");
    assert_eq!(uri, "file:///home/luis/My%20Proj");
    assert_eq!(
        cursor::workspace_from_uri(Some(&uri)).as_deref(),
        Some("/home/luis/My Proj")
    );
    assert_eq!(
        cursor::workspace_uri_from_cwd("/home/luis/Desktop"),
        "file:///home/luis/Desktop"
    );
}

#[test]
fn encode_checkpoint_round_trips_through_decode_checkpoint() {
    let blob = cursor::encode_checkpoint(&CheckpointWriteInput {
        message_ids: vec![blob_id(1), blob_id(2)],
        workspace: Some("file:///home/luis/proj".into()),
        ..Default::default()
    });
    let decoded = cursor::decode_checkpoint(&blob).unwrap();
    assert_eq!(decoded.message_ids, vec![blob_id(1), blob_id(2)]);
    assert_eq!(decoded.workspace.as_deref(), Some("file:///home/luis/proj"));
    assert_eq!(decoded.client.as_deref(), Some("cli"));

    // malformed refs are skipped; a bare checkpoint still records the client
    let bare = cursor::encode_checkpoint(&CheckpointWriteInput {
        message_ids: vec!["not-hex".into(), blob_id(3)],
        client: Some("x".into()),
        ..Default::default()
    });
    let decoded = cursor::decode_checkpoint(&bare).unwrap();
    assert_eq!(decoded.message_ids, vec![blob_id(3)]);
    assert_eq!(decoded.workspace, None);
    assert_eq!(decoded.client.as_deref(), Some("x"));

    // absent fields are omitted
    let sparse = cursor::encode_checkpoint(&CheckpointWriteInput {
        message_ids: vec!["abcd".into(), blob_id(7)],
        ..Default::default()
    });
    let decoded = cursor::decode_checkpoint(&sparse).unwrap();
    assert_eq!(decoded.message_ids, vec![blob_id(7)]);
    assert_eq!(decoded.workspace, None);
    assert_eq!(decoded.client.as_deref(), Some("cli"));
}

#[test]
fn encode_store_meta_and_encode_meta_json_round_trip_through_parsers() {
    let meta = cursor::parse_store_meta(&cursor::encode_store_meta(&StoreMetaWriteInput {
        agent_id: "chat-1".into(),
        latest_root_blob_id: blob_id(7),
        name: Some("T".into()),
        mode: Some("default".into()),
        is_run_everything: Some(false),
        created_at: Some(1_700_000_000_000.0),
        last_used_model: Some("default".into()),
    }))
    .unwrap();
    assert_eq!(
        meta,
        CursorStoreMeta {
            agent_id: Some("chat-1".into()),
            latest_root_blob_id: Some(blob_id(7)),
            name: Some("T".into()),
            mode: Some("default".into()),
            is_run_everything: false,
            created_at: Some(1_700_000_000_000.0),
            last_used_model: Some("default".into()),
        }
    );

    let sidecar = cursor::parse_meta_json(
        &serde_json::from_str::<Value>(&cursor::encode_meta_json(&MetaJsonWriteInput {
            created_at_ms: 1_700_000_000_000.0,
            updated_at_ms: 1_700_000_300_000.0,
            title: Some("T".into()),
            has_conversation: true,
            cwd: Some("/w".into()),
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(sidecar.schema_version, Some(1.0));
    assert_eq!(sidecar.created_at_ms, Some(1_700_000_000_000.0));
    assert_eq!(sidecar.updated_at_ms, Some(1_700_000_300_000.0));
    assert_eq!(sidecar.title.as_deref(), Some("T"));
    assert!(sidecar.has_conversation);
    assert_eq!(sidecar.cwd.as_deref(), Some("/w"));

    // optional fields stay absent rather than serialising as nulls
    let sparse: Value = serde_json::from_str(&cursor::encode_meta_json(&MetaJsonWriteInput {
        created_at_ms: 1.0,
        updated_at_ms: 2.0,
        has_conversation: false,
        title: Some(String::new()),
        ..Default::default()
    }))
    .unwrap();
    assert!(sparse.get("title").is_none());
    assert!(sparse.get("cwd").is_none());
    assert_eq!(sparse["hasConversation"], false);

    // bare meta emits only the required keys
    let meta = cursor::parse_store_meta(&cursor::encode_store_meta(&StoreMetaWriteInput {
        agent_id: "a".into(),
        latest_root_blob_id: blob_id(1),
        ..Default::default()
    }))
    .unwrap();
    assert_eq!(meta.agent_id.as_deref(), Some("a"));
    assert_eq!(
        meta.latest_root_blob_id.as_deref(),
        Some(blob_id(1).as_str())
    );
    assert_eq!(meta.name, None);
    assert_eq!(meta.mode, None);
    assert_eq!(meta.created_at, None);
    assert_eq!(meta.last_used_model, None);
}

#[test]
fn store_tool_calls_carry_locations_and_diffs_from_args() {
    fn json_bytes(value: &Value) -> Vec<u8> {
        serde_json::to_string(value).unwrap().into_bytes()
    }
    let mut ckpt = Vec::new();
    ckpt.extend_from_slice(&{
        let mut out = Vec::new();
        let id = cursor::from_hex(&blob_id(1)).unwrap();
        out.push(0x0a); // field 1, wire 2
        out.push(32); // len
        out.extend_from_slice(&id);
        out
    });
    ckpt.extend_from_slice(&[0x50, 0x01]); // field 10 varint 1
    let session = cursor::session_from_store(&cursor::CursorStoreInput {
        id: "chat-2".into(),
        meta: Some(CursorStoreMeta {
            latest_root_blob_id: Some(blob_id(0)),
            ..Default::default()
        }),
        blobs: vec![
            (blob_id(0), ckpt),
            (
                blob_id(1),
                json_bytes(&json!({
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool-call",
                            "toolCallId": "tool_edit",
                            "toolName": "StrReplace",
                            "args": { "path": "/w/a.ts", "old_string": "o", "new_string": "n" },
                        },
                        {
                            "type": "tool-call",
                            "toolCallId": "tool_patch",
                            "toolName": "ApplyPatch",
                            "args": {
                                "patch": "*** Begin Patch\n*** Update File: /w/b.ts\n@@\n-a\n+b\n*** End Patch",
                            },
                        },
                        {
                            "type": "tool-call",
                            "toolCallId": "tool_read",
                            "toolName": "ReadLints",
                            "args": { "paths": ["/w/x.ts", "/w/y.ts"] },
                        },
                    ],
                })),
            ),
        ],
        ..Default::default()
    });
    let calls = &session.nodes[0].tool_calls;
    assert_eq!(
        calls[0].diffs,
        vec![ToolCallDiff {
            path: "/w/a.ts".into(),
            old_text: Some("o".into()),
            new_text: Some("n".into()),
        }]
    );
    // object-shaped patch args decode through the same parser
    assert_eq!(
        calls[1].diffs,
        vec![ToolCallDiff {
            path: "/w/b.ts".into(),
            old_text: Some("a".into()),
            new_text: Some("b".into()),
        }]
    );
    assert_eq!(calls[2].locations, vec![loc("/w/x.ts"), loc("/w/y.ts")]);
    assert_eq!(calls[2].diffs, vec![]);
}

fn writable_nodes() -> Vec<MessageNode> {
    let mut assistant = make_node(
        1,
        Some(0),
        Role::Assistant,
        "I'll look around.",
        Value::Null,
    );
    assistant.thinking = Some(REDACTED_THINKING.into());
    assistant.tool_calls = vec![tool_call(
        "call-1",
        "Glob",
        json!({ "glob_pattern": "src/**" }),
    )];
    vec![
        make_node(0, None, Role::User, "explore the repo", Value::Null),
        assistant,
    ]
}

fn writable_session(id: &str, cwd: &str) -> Session {
    Session {
        id: id.into(),
        title: "unused title".into(),
        working_directory: cwd.into(),
        model: "composer-1".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_300.0,
        main_chain_id: 1,
        metadata: json!({ "source": "import" }),
        nodes: writable_nodes(),
        ..make_session(id, vec![])
    }
}

fn blob_json(blob: &cursor::StoreBlobWrite) -> Value {
    serde_json::from_slice(&blob.data).unwrap()
}

#[test]
fn message_blobs_from_session_emits_the_shapes_the_reader_decodes() {
    let mut user_info = make_node(
        1,
        None,
        Role::User,
        "<user_info>\nOS: linux\n</user_info>",
        json!({ "context": "user_info" }),
    );
    user_info.request_id = None;
    let mut query = make_node(
        2,
        None,
        Role::User,
        "<user_query>\ndo it\n</user_query>",
        Value::Null,
    );
    query.request_id = Some("req-9".into());
    let mut assistant = make_node(3, None, Role::Assistant, "on it", Value::Null);
    assistant.thinking = Some(REDACTED_THINKING.into());
    assistant.thinking_signature = Some("sig-1".into());
    assistant.tool_calls = vec![tool_call("tool_x", "Read", json!({ "path": "/w/a.ts" }))];
    let mut result = make_node(4, None, Role::Tool, "result text", Value::Null);
    result.tool_call_id = Some("tool_x".into());
    result.tool_name = Some("Read".into());
    result.tool_result = Some(sepia_core::domain::ToolResultInfo {
        status: sepia_core::domain::ToolCallStatus::Error,
        exit_code: None,
        duration_ms: Some(41.0),
    });
    let session = Session {
        nodes: vec![
            make_node(0, None, Role::System, "be brief", Value::Null),
            user_info,
            query,
            assistant,
            result,
        ],
        ..make_session("s-1", vec![])
    };

    let blobs = cursor::message_blobs_from_session(&session);
    let parsed: Vec<Value> = blobs.iter().map(blob_json).collect();
    assert_eq!(parsed.len(), 5);
    assert_eq!(
        parsed[0],
        json!({ "role": "system", "content": "be brief" })
    );
    // `<user_info>` context keeps the raw-string content form
    assert_eq!(
        parsed[1],
        json!({ "role": "user", "content": "<user_info>\nOS: linux\n</user_info>" })
    );
    assert_eq!(parsed[2]["role"], "user");
    assert_eq!(
        parsed[2]["content"],
        json!([{ "type": "text", "text": "<user_query>\ndo it\n</user_query>" }])
    );
    assert_eq!(parsed[2]["providerOptions"]["cursor"]["requestId"], "req-9");
    assert_eq!(parsed[3]["role"], "assistant");
    assert_eq!(parsed[3]["id"], "1");
    assert_eq!(
        parsed[3]["content"],
        json!([
            { "type": "redacted-reasoning", "data": "sig-1" },
            { "type": "text", "text": "on it" },
            { "type": "tool-call", "toolCallId": "tool_x", "toolName": "Read", "args": { "path": "/w/a.ts" } },
        ])
    );
    assert_eq!(parsed[4]["role"], "tool");
    assert_eq!(parsed[4]["id"], "tool_x");
    assert_eq!(
        parsed[4]["content"],
        json!([{
            "type": "tool-result",
            "toolCallId": "tool_x",
            "toolName": "Read",
            "result": "result text",
        }])
    );
    assert_eq!(
        parsed[4]["providerOptions"]["cursor"]["highLevelToolCallResult"]["output"],
        json!({ "isError": true, "success": { "executionTime": 41 } })
    );

    // every emitted blob's id is the sha256 of its own bytes
    for blob in &blobs {
        assert_eq!(blob.id, cursor::blob_id_for(&blob.data));
    }
}

#[test]
fn message_blobs_from_session_regroups_same_blob_tool_nodes_and_derives_ids() {
    let mut a = make_node(0, None, Role::Tool, "a", json!({ "blobId": "deadbeef" }));
    a.tool_call_id = Some("t1".into());
    let mut b = make_node(1, None, Role::Tool, "b", json!({ "blobId": "deadbeef" }));
    b.tool_call_id = Some("t2".into());
    let session = Session {
        nodes: vec![a, b],
        ..make_session("s-2", vec![])
    };
    let blobs = cursor::message_blobs_from_session(&session);
    assert_eq!(blobs.len(), 1);
    let parsed = blob_json(&blobs[0]);
    let results = parsed["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["result"], "a");
    assert_eq!(results[1]["result"], "b");

    // requestIds derive deterministically from session+node, not randomly
    let session = Session {
        nodes: vec![make_node(0, None, Role::User, "q", Value::Null)],
        ..make_session("s-3", vec![])
    };
    let first = cursor::message_blobs_from_session(&session);
    let second = cursor::message_blobs_from_session(&session);
    assert_eq!(first[0].id, second[0].id);
    let other = Session {
        id: "different".into(),
        nodes: vec![make_node(0, None, Role::User, "q", Value::Null)],
        ..make_session("different", vec![])
    };
    assert_ne!(
        cursor::message_blobs_from_session(&other)[0].id,
        first[0].id
    );
}

#[test]
fn store_write_plan_reuses_the_empty_blob_root_for_a_messageless_session() {
    let session = Session {
        nodes: vec![],
        prompt_history: vec![sepia_core::domain::PromptHistoryEntry {
            content: "kept".into(),
            timestamp: 1.0,
            is_shell: false,
        }],
        ..writable_session("empty-chat", "/tmp/empty")
    };
    let plan = cursor::store_write_plan(&session, None);
    // sha256("") — a chat with no messages still roots at the empty blob
    assert_eq!(
        plan.root_blob_id,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let meta = cursor::parse_store_meta(&plan.meta_row).unwrap();
    assert_eq!(
        meta.latest_root_blob_id.as_deref(),
        Some(plan.root_blob_id.as_str())
    );
    assert_eq!(meta.agent_id.as_deref(), Some("empty-chat"));
    assert_eq!(meta.created_at, Some(1_700_000_000_000.0));
    assert_eq!(meta.last_used_model.as_deref(), Some("composer-1"));
    assert_eq!(plan.prompt_history_json.as_deref(), Some(r#"["kept"]"#));

    // prior creation stamp wins on rewrite
    let plan = cursor::store_write_plan(&session, Some(1_600_000_000_000.0));
    let meta = cursor::parse_store_meta(&plan.meta_row).unwrap();
    assert_eq!(meta.created_at, Some(1_600_000_000_000.0));
    let sidecar: Value = serde_json::from_str(&plan.meta_json).unwrap();
    assert_eq!(sidecar["createdAtMs"], 1_600_000_000_000_i64);
}

#[test]
fn store_write_plan_derives_prompts_from_queries_and_encodes_the_root() {
    let session = writable_session("s-1", "/tmp/proj");
    let plan = cursor::store_write_plan(&session, None);
    // empty blob + 2 message blobs + the checkpoint
    assert_eq!(plan.blobs.len(), 4);
    let checkpoint = cursor::decode_checkpoint(&plan.blobs[3].data).unwrap();
    assert_eq!(
        checkpoint.message_ids,
        vec![plan.blobs[1].id.clone(), plan.blobs[2].id.clone()]
    );
    assert_eq!(checkpoint.workspace.as_deref(), Some("file:///tmp/proj"));
    assert_eq!(checkpoint.client.as_deref(), Some("cli"));
    assert_eq!(plan.root_blob_id, cursor::blob_id_for(&plan.blobs[3].data));
    assert_eq!(
        plan.prompt_history_json.as_deref(),
        Some(r#"["explore the repo"]"#)
    );
}
