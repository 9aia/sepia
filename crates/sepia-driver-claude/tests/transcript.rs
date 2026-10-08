#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Port of `packages/claude/tests/ClaudeCode.test.ts` +
//! `ClaudeCodeWriter.test.ts` — the JSONL reader/writer behavior spec.

use sepia_core::domain::{Block, MessageNode, REDACTED_THINKING, Role, Session, ToolCallStatus};
use sepia_driver_claude::{
    ClaudeSourceInfo, decode_project_dir, from_jsonl, summarize_jsonl, to_jsonl,
};
use sepia_testkit::assert_json_eq;
use serde_json::{Value, json};

fn line(entry: Value) -> String {
    serde_json::to_string(&entry).unwrap()
}

fn source(id: &str) -> ClaudeSourceInfo {
    ClaudeSourceInfo {
        id: id.into(),
        ..Default::default()
    }
}

fn user_entry(uuid: &str, parent: Value, content: Value, extra: Value) -> Value {
    let mut entry = json!({
        "type": "user",
        "uuid": uuid,
        "parentUuid": parent,
        "sessionId": "sess-1",
        "isSidechain": false,
        "cwd": "/work/proj",
        "gitBranch": "main",
        "version": "2.1.0",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "message": { "role": "user", "content": content },
    });
    if let Value::Object(extra) = extra {
        entry.as_object_mut().unwrap().extend(extra);
    }
    entry
}

fn assistant_entry(uuid: &str, parent: &str, content: Value, message_extra: Value) -> Value {
    let mut entry = json!({
        "type": "assistant",
        "uuid": uuid,
        "parentUuid": parent,
        "sessionId": "sess-1",
        "isSidechain": false,
        "requestId": "req_1",
        "timestamp": "2026-01-01T00:00:01.000Z",
        "message": {
            "id": "msg_1",
            "role": "assistant",
            "model": "claude-opus-4-5",
            "content": content,
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 40,
                "cache_read_input_tokens": 12,
                "cache_creation_input_tokens": 8,
            },
        },
    });
    if let Value::Object(extra) = message_extra {
        entry
            .pointer_mut("/message")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .extend(extra);
    }
    entry
}

#[test]
fn maps_entries_to_a_linked_node_tree() {
    let session = from_jsonl(
        &[
            line(json!({ "type": "summary", "summary": "Fix the login bug", "leafUuid": "u3" })),
            line(user_entry("u1", Value::Null, json!("fix the login bug please"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([
                    { "type": "thinking", "thinking": "look at auth", "signature": "sig-1" },
                    { "type": "text", "text": "I'll check the auth module" },
                    { "type": "tool_use", "id": "toolu_1", "name": "Read", "input": { "file_path": "/src/a.ts" } },
                ]),
                json!({}),
            )),
            line(user_entry(
                "u3",
                json!("u2"),
                json!([{ "type": "tool_result", "tool_use_id": "toolu_1", "content": "file contents", "is_error": false }]),
                json!({}),
            )),
            line(assistant_entry(
                "u4",
                "u3",
                json!([{ "type": "text", "text": "done" }]),
                json!({ "usage": { "input_tokens": 5, "output_tokens": 2 } }),
            )),
        ]
        .join("\n"),
        &source("sess-1"),
    );

    assert_eq!(session.id, "sess-1");
    assert_eq!(session.title, "Fix the login bug");
    assert_eq!(session.working_directory, "/work/proj");
    assert_eq!(session.backend_type, "claude");
    assert_eq!(session.agent_mode, "accept-edits");
    assert_eq!(session.model, "claude-opus-4-5");
    assert_eq!(session.created_at, 1767225600.0);
    assert_eq!(session.last_activity_at, 1767225601.0);
    assert_eq!(session.main_chain_id, session.nodes.len() as i64 - 1);
    assert!(session.parent_session_id.is_none());
    assert_eq!(session.metadata["gitBranch"], "main");
    assert_eq!(session.metadata["claudeVersion"], "2.1.0");
    assert_eq!(session.metadata["source"], "claude-code");

    let [user, assistant, tool, last] = &session.nodes[..] else {
        panic!("expected 4 nodes");
    };
    assert_eq!(user.role, Role::User);
    assert!(user.parent_node_id.is_none());
    assert_eq!(user.content, "fix the login bug please");

    assert_eq!(assistant.role, Role::Assistant);
    assert_eq!(assistant.parent_node_id, Some(user.node_id));
    assert_eq!(assistant.thinking.as_deref(), Some("look at auth"));
    assert_eq!(assistant.thinking_signature.as_deref(), Some("sig-1"));
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(assistant.tool_calls[0].name, "Read");
    assert_json_eq(
        &assistant.tool_calls[0].arguments,
        &json!({ "file_path": "/src/a.ts" }),
        "tool arguments",
    );
    // the tool result's success is folded back onto the call
    assert_eq!(
        assistant.tool_calls[0].status,
        Some(ToolCallStatus::Success)
    );
    assert_eq!(assistant.request_id.as_deref(), Some("req_1"));
    assert_eq!(assistant.finish_reason.as_deref(), Some("end_turn"));
    assert_eq!(assistant.model.as_deref(), Some("claude-opus-4-5"));
    assert_json_eq(
        &serde_json::to_value(&assistant.usage).unwrap(),
        &json!({ "input": 100, "output": 40, "cacheRead": 12, "cacheWrite": 8 }),
        "usage",
    );

    assert_eq!(tool.role, Role::Tool);
    assert_eq!(tool.parent_node_id, Some(assistant.node_id));
    assert_eq!(tool.tool_call_id.as_deref(), Some("toolu_1"));
    assert_eq!(tool.tool_name.as_deref(), Some("Read"));
    assert_eq!(
        tool.tool_result.as_ref().unwrap().status,
        ToolCallStatus::Success
    );
    assert_eq!(tool.content, "file contents");
    assert_json_eq(
        &tool.metadata["toolArguments"],
        &json!({ "file_path": "/src/a.ts" }),
        "toolArguments",
    );

    assert_eq!(last.role, Role::Assistant);
    assert_json_eq(
        &serde_json::to_value(&last.usage).unwrap(),
        &json!({ "input": 5, "output": 2 }),
        "usage",
    );

    // prompt history is the user's own text, not tool results
    let history: Vec<&str> = session
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(history, ["fix the login bug please"]);
}

#[test]
fn usage_decodes_nested_ephemeral_cache_creation_tiers() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([{ "type": "text", "text": "hey" }]),
                json!({
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 3,
                        "cache_creation": {
                            "ephemeral_5m_input_tokens": 7,
                            "ephemeral_1h_input_tokens": 11,
                        },
                    },
                }),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_json_eq(
        &serde_json::to_value(&session.nodes[1].usage).unwrap(),
        &json!({ "input": 10, "output": 3, "cacheWrite": 18 }),
        "usage",
    );
}

#[test]
fn user_block_arrays_become_nodes_with_attachments_in_blocks() {
    let session = from_jsonl(
        &line(user_entry(
            "u1",
            Value::Null,
            json!([
                { "type": "text", "text": "see this" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAA=" } },
                { "type": "document", "source": { "type": "text", "media_type": "text/plain", "text": "doc body" }, "title": "notes.txt" },
                { "type": "image", "source": { "type": "url", "url": "https://x/img.png" } },
                { "type": "image" },
                { "type": "image", "data": "BBB", "mimeType": "image/jpeg" },
                { "type": "image", "url": "https://x/direct.png", "media_type": "image/gif" },
                { "type": "document", "url": "https://x/doc.pdf", "media_type": "application/pdf" },
                { "type": "document", "data": "RA==" },
                { "type": "document" },
                { "type": "unknown-block", "payload": 1 },
                "not-an-object",
            ]),
            json!({}),
        )),
        &source("s"),
    );
    let node = &session.nodes[0];
    assert_eq!(node.role, Role::User);
    assert_eq!(node.content, "see this");
    assert_json_eq(
        &serde_json::to_value(&node.blocks).unwrap(),
        &json!([
            { "type": "text", "text": "see this" },
            { "type": "image", "data": "AAA=", "mimeType": "image/png" },
            { "type": "file", "text": "doc body", "name": "notes.txt", "mimeType": "text/plain" },
            { "type": "image", "uri": "https://x/img.png" },
            { "type": "image", "data": "BBB", "mimeType": "image/jpeg" },
            { "type": "image", "uri": "https://x/direct.png", "mimeType": "image/gif" },
            { "type": "file", "uri": "https://x/doc.pdf", "mimeType": "application/pdf" },
            { "type": "file", "data": "RA==" },
        ]),
        "blocks",
    );
}

#[test]
fn all_text_content_arrays_leave_blocks_empty() {
    let session = from_jsonl(
        &line(user_entry(
            "u1",
            Value::Null,
            json!([{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }]),
            json!({}),
        )),
        &source("s"),
    );
    assert_eq!(session.nodes[0].content, "a\nb");
    assert!(session.nodes[0].blocks.is_empty());
}

#[test]
fn errored_tool_results_mark_the_call_and_the_tool_node_as_error() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("run it"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([{ "type": "tool_use", "id": "toolu_9", "name": "Bash", "input": { "command": "false" } }]),
                json!({}),
            )),
            line(user_entry(
                "u3",
                json!("u2"),
                json!([{ "type": "tool_result", "tool_use_id": "toolu_9", "content": [{ "type": "text", "text": "exit 1" }], "is_error": true }]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let tool = &session.nodes[2];
    assert_eq!(
        tool.tool_result.as_ref().unwrap().status,
        ToolCallStatus::Error
    );
    assert_eq!(tool.content, "exit 1");
    assert_eq!(
        session.nodes[1].tool_calls[0].status,
        Some(ToolCallStatus::Error)
    );
}

#[test]
fn orphaned_tool_results_keep_output_without_a_resolved_tool_name() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(user_entry(
                "u2",
                json!("u1"),
                json!([
                    { "type": "tool_result", "tool_use_id": "toolu_gone", "content": "late output" },
                    { "type": "tool_result", "content": 42 },
                ]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let orphan = &session.nodes[1];
    assert_eq!(orphan.role, Role::Tool);
    assert_eq!(orphan.tool_call_id.as_deref(), Some("toolu_gone"));
    assert!(orphan.tool_name.is_none());
    assert_eq!(orphan.metadata["toolArguments"], Value::Null);
    let bare = &session.nodes[2];
    assert_eq!(bare.role, Role::Tool);
    assert!(bare.tool_call_id.is_none());
    assert_eq!(bare.content, "42");
}

#[test]
fn non_text_tool_result_content_serializes_and_text_joins() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(user_entry(
                "u2",
                json!("u1"),
                json!([
                    { "type": "tool_result", "tool_use_id": "t1", "content": [{ "type": "text", "text": "first" }, { "type": "text", "text": "second" }] },
                    { "type": "tool_result", "tool_use_id": "t2", "content": [{ "type": "image", "source": { "type": "base64", "data": "AA" } }] },
                ]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_eq!(session.nodes[1].content, "first\nsecond");
    assert!(session.nodes[2].content.contains("image"));
}

#[test]
fn system_entries_become_system_nodes_with_subtype_metadata() {
    let session = from_jsonl(
        &[
            line(json!({
                "type": "system", "subtype": "init", "uuid": "s0", "parentUuid": null,
                "sessionId": "sess-1", "timestamp": "2026-01-01T00:00:00.000Z",
            })),
            line(user_entry("u1", json!("s0"), json!("hello"), json!({}))),
            line(json!({
                "type": "system", "subtype": "compact_boundary",
                "content": "Conversation compacted", "level": "info",
                "compactMetadata": { "trigger": "auto", "preTokens": 1000 },
                "uuid": "s1", "parentUuid": "u1", "timestamp": "2026-01-01T00:00:02.000Z",
            })),
        ]
        .join("\n"),
        &source("s"),
    );
    let init = &session.nodes[0];
    assert_eq!(init.role, Role::System);
    assert_eq!(init.content, "[claude init]");
    assert_eq!(init.metadata["subtype"], "init");
    let compact = &session.nodes[2];
    assert_eq!(compact.role, Role::System);
    assert_eq!(compact.content, "Conversation compacted");
    assert_json_eq(
        &compact.metadata["compactMetadata"],
        &json!({ "trigger": "auto", "preTokens": 1000 }),
        "compactMetadata",
    );
}

#[test]
fn summary_free_files_title_from_the_first_user_message() {
    let session = from_jsonl(
        &line(user_entry(
            "u1",
            Value::Null,
            json!(format!("  {}  ", "word ".repeat(30))),
            json!({}),
        )),
        &source("untitled-id"),
    );
    assert_eq!(session.title, "word ".repeat(30).trim()[..80].to_string());
    assert!(session.title.len() <= 80);

    let empty = from_jsonl("", &source("bare"));
    assert_eq!(empty.title, "bare");
    assert!(empty.nodes.is_empty());
    assert_eq!(empty.working_directory, "/");
}

#[test]
fn fallback_cwd_covers_entries_without_cwd_and_last_git_branch_wins() {
    let session = from_jsonl(
        &[
            line(json!({ "type": "user", "uuid": "u1", "parentUuid": null, "message": { "content": "hi" } })),
            line(json!({ "type": "user", "uuid": "u2", "parentUuid": "u1", "gitBranch": "feature/x", "message": { "content": "again" } })),
        ]
        .join("\n"),
        &ClaudeSourceInfo {
            id: "s".into(),
            fallback_cwd: Some(decode_project_dir("-home-luis-proj")),
            ..Default::default()
        },
    );
    assert_eq!(session.working_directory, "/home/luis/proj");
    assert_eq!(session.metadata["gitBranch"], "feature/x");
}

#[test]
fn subagent_files_map_is_sidechain_and_session_id_to_parent_and_agent() {
    let session = from_jsonl(
        &[
            line(json!({
                "type": "user", "uuid": "a1", "parentUuid": null,
                "sessionId": "parent-uuid", "agentId": "agent-42", "isSidechain": true,
                "cwd": "/work/proj", "timestamp": "2026-01-01T00:00:00.000Z",
                "message": { "role": "user", "content": "subtask prompt" },
            })),
            line(json!({
                "type": "assistant", "uuid": "a2", "parentUuid": "a1",
                "sessionId": "parent-uuid", "agentId": "agent-42", "isSidechain": true,
                "timestamp": "2026-01-01T00:00:01.000Z",
                "message": { "role": "assistant", "model": "claude-haiku-4-5", "content": [{ "type": "text", "text": "working" }] },
            })),
        ]
        .join("\n"),
        &source("agent-42"),
    );
    assert_eq!(session.parent_session_id.as_deref(), Some("parent-uuid"));
    assert_eq!(session.agent_id.as_deref(), Some("agent-42"));
    assert_eq!(session.nodes[1].metadata["isSidechain"], true);
    assert_eq!(session.model, "claude-haiku-4-5");
}

#[test]
fn agent_files_derive_agent_id_from_the_filename_when_entries_omit_it() {
    let session = from_jsonl(
        &line(json!({
            "type": "user", "uuid": "a1", "parentUuid": null,
            "sessionId": "parent-uuid", "isSidechain": true,
            "message": { "role": "user", "content": "task" },
        })),
        &ClaudeSourceInfo {
            id: "agent-77".into(),
            parent_session_id: Some("parent-uuid".into()),
            ..Default::default()
        },
    );
    assert_eq!(session.agent_id.as_deref(), Some("77"));
    assert_eq!(session.parent_session_id.as_deref(), Some("parent-uuid"));
}

#[test]
fn inline_sidechain_roots_form_a_separate_tree_in_the_same_session() {
    let session = from_jsonl(
        &[
            line(user_entry(
                "u1",
                Value::Null,
                json!("main question"),
                json!({}),
            )),
            line(user_entry("u2", json!("u1"), json!("more"), json!({}))),
            line(user_entry(
                "s1",
                Value::Null,
                json!("sidechain root"),
                json!({ "isSidechain": true }),
            )),
        ]
        .join("\n"),
        &source("sess-1"),
    );
    assert!(session.nodes[2].parent_node_id.is_none());
    // sessionId matches the file id, so no parentSessionId is inferred
    assert!(session.parent_session_id.is_none());
}

#[test]
fn parent_links_resolve_through_non_message_entries_and_dangling_ids_chain_linearly() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("first"), json!({}))),
            line(json!({
                "type": "file-history-snapshot", "messageId": "u1", "snapshot": {},
                "uuid": "snap-1", "parentUuid": "u1",
            })),
            line(user_entry(
                "u2",
                json!("snap-1"),
                json!("after snapshot"),
                json!({}),
            )),
            line(user_entry(
                "u3",
                json!("uuid-that-does-not-exist"),
                json!("dangling"),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let roles: Vec<Role> = session.nodes.iter().map(|n| n.role).collect();
    assert_eq!(roles, [Role::User, Role::User, Role::User]);
    assert_eq!(session.nodes[1].parent_node_id, Some(0));
    assert_eq!(session.nodes[2].parent_node_id, Some(1));
}

#[test]
fn edit_family_tool_calls_carry_locations_and_revertable_diffs() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("change things"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([
                    { "type": "tool_use", "id": "toolu_e", "name": "Edit", "input": { "file_path": "/work/proj/a.ts", "old_string": "old", "new_string": "new" } },
                    { "type": "tool_use", "id": "toolu_m", "name": "MultiEdit", "input": { "file_path": "/work/proj/b.ts", "edits": [ { "old_string": "o1", "new_string": "n1" }, { "old_string": "o2" }, { "not": "a-hunk" } ] } },
                    { "type": "tool_use", "id": "toolu_w", "name": "Write", "input": { "file_path": "/work/proj/c.ts", "content": "whole file" } },
                    { "type": "tool_use", "id": "toolu_r", "name": "Read", "input": { "file_path": "/work/proj/a.ts" } },
                    { "type": "tool_use", "id": "toolu_n", "name": "NotebookEdit", "input": { "notebook_path": "/work/proj/nb.ipynb", "new_source": "cell" } },
                    { "type": "tool_use", "id": "toolu_b", "name": "Bash", "input": { "command": "ls" } },
                    { "type": "tool_use", "id": "toolu_g", "name": "Glob", "input": { "path": "/work/proj" } },
                    // degenerate inputs stay honest: nothing recorded is
                    // better than guessing a diff.
                    { "type": "tool_use", "id": "toolu_raw", "name": "Edit", "input": "not-an-object" },
                    { "type": "tool_use", "id": "toolu_one", "name": "Edit", "input": { "file_path": "/work/proj/d.ts", "new_string": "only-new" } },
                    { "type": "tool_use", "id": "toolu_bad_edits", "name": "MultiEdit", "input": { "file_path": "/work/proj/e.ts", "edits": "not-a-list" } },
                    { "type": "tool_use", "id": "toolu_junk_edit", "name": "MultiEdit", "input": { "file_path": "/work/proj/e.ts", "edits": ["junk", { "new_string": "x" }] } },
                    { "type": "tool_use", "id": "toolu_no_content", "name": "Write", "input": { "file_path": "/work/proj/f.ts" } },
                    { "type": "redacted_thinking" },
                ]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let calls = &session.nodes[1].tool_calls;
    let [
        edit,
        multi,
        write,
        read,
        notebook,
        bash,
        glob,
        raw,
        one_sided,
        bad_edits,
        junk_edits,
        no_content,
    ] = &calls[..]
    else {
        panic!("expected 12 tool calls");
    };
    assert_json_eq(
        &serde_json::to_value(&edit.locations).unwrap(),
        &json!([{ "path": "/work/proj/a.ts" }]),
        "edit.locations",
    );
    assert_json_eq(
        &serde_json::to_value(&edit.diffs).unwrap(),
        &json!([{ "path": "/work/proj/a.ts", "oldText": "old", "newText": "new" }]),
        "edit.diffs",
    );
    assert_json_eq(
        &serde_json::to_value(&multi.diffs).unwrap(),
        &json!([
            { "path": "/work/proj/b.ts", "oldText": "o1", "newText": "n1" },
            { "path": "/work/proj/b.ts", "oldText": "o2" },
        ]),
        "multi.diffs",
    );
    assert_json_eq(
        &serde_json::to_value(&write.diffs).unwrap(),
        &json!([{ "path": "/work/proj/c.ts", "newText": "whole file" }]),
        "write.diffs",
    );
    assert_json_eq(
        &serde_json::to_value(&read.locations).unwrap(),
        &json!([{ "path": "/work/proj/a.ts" }]),
        "read.locations",
    );
    assert!(read.diffs.is_empty());
    // cell-level new_source is not a file diff — location only
    assert_json_eq(
        &serde_json::to_value(&notebook.locations).unwrap(),
        &json!([{ "path": "/work/proj/nb.ipynb" }]),
        "notebook.locations",
    );
    assert!(notebook.diffs.is_empty());
    assert!(bash.locations.is_empty());
    assert!(bash.diffs.is_empty());
    assert_json_eq(
        &serde_json::to_value(&glob.locations).unwrap(),
        &json!([{ "path": "/work/proj" }]),
        "glob.locations",
    );
    assert!(raw.locations.is_empty());
    assert!(raw.diffs.is_empty());
    assert_json_eq(
        &serde_json::to_value(&one_sided.diffs).unwrap(),
        &json!([{ "path": "/work/proj/d.ts", "newText": "only-new" }]),
        "one_sided.diffs",
    );
    assert!(bad_edits.diffs.is_empty());
    assert_json_eq(
        &serde_json::to_value(&junk_edits.diffs).unwrap(),
        &json!([{ "path": "/work/proj/e.ts", "newText": "x" }]),
        "junk_edits.diffs",
    );
    assert!(no_content.diffs.is_empty());
    assert_json_eq(
        &serde_json::to_value(&no_content.locations).unwrap(),
        &json!([{ "path": "/work/proj/f.ts" }]),
        "no_content.locations",
    );
}

#[test]
fn file_history_snapshot_entries_become_checkpoints_with_a_path_backup_map() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("go"), json!({}))),
            line(json!({
                "type": "file-history-snapshot", "messageId": "msg-1", "uuid": "snap-1",
                "parentUuid": "u1",
                "snapshot": {
                    "messageId": "msg-1", "timestamp": "2026-01-01T00:00:00.500Z",
                    "trackedFileBackups": {
                        "/work/proj/a.ts": { "backupFileName": "hash1@v1", "version": 1, "backupTime": "2026-01-01T00:00:00.000Z" },
                        "/work/proj/deleted.ts": { "backupFileName": null, "version": 2 },
                        "/work/proj/junk.ts": "not-an-object",
                    },
                },
            })),
            // an isSnapshotUpdate entry tops up the same ref — files merge
            line(json!({
                "type": "file-history-snapshot", "messageId": "msg-1",
                "isSnapshotUpdate": true, "uuid": "snap-2", "parentUuid": "snap-1",
                "snapshot": {
                    "messageId": "msg-1",
                    "trackedFileBackups": { "/work/proj/b.ts": { "backupFileName": "hash2@v1", "version": 1 } },
                },
            })),
            line(user_entry("u2", json!("snap-2"), json!("next"), json!({}))),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_json_eq(
        &serde_json::to_value(&session.checkpoints).unwrap(),
        &json!([{ "ref": "msg-1", "createdAt": 1767225600000.0, "kind": "file-history-snapshot" }]),
        "checkpoints",
    );
    let history = &session.metadata["fileHistory"];
    assert_eq!(history["sessionId"], "sess-1");
    assert_json_eq(
        &history["snapshots"]["msg-1"]["files"],
        &json!({
            "/work/proj/a.ts": { "backup": "hash1@v1", "version": 1 },
            "/work/proj/deleted.ts": { "backup": null, "version": 2 },
            "/work/proj/b.ts": { "backup": "hash2@v1", "version": 1 },
        }),
        "fileHistory.files",
    );
}

#[test]
fn file_history_falls_back_to_the_file_id_and_tolerates_missing_pieces() {
    let session = from_jsonl(
        &[
            // no sessionId/timestamp anywhere — uuid is the ref, epoch the time
            line(json!({
                "type": "file-history-snapshot", "uuid": "snap-x",
                "snapshot": { "trackedFileBackups": { "/w/a.ts": { "backupFileName": "h@v1" } } },
            })),
            // no ids at all — nothing to key the snapshot under
            line(json!({ "type": "file-history-snapshot", "parentUuid": "snap-x" })),
            // ref inside the snapshot object; a non-map trackedFileBackups is empty
            line(json!({
                "type": "file-history-snapshot", "uuid": "snap-y",
                "snapshot": { "messageId": "inner-y", "trackedFileBackups": "junk" },
            })),
        ]
        .join("\n"),
        &source("sid-fallback"),
    );
    assert_json_eq(
        &serde_json::to_value(&session.checkpoints).unwrap(),
        &json!([
            { "ref": "snap-x", "createdAt": 0, "kind": "file-history-snapshot" },
            { "ref": "inner-y", "createdAt": 0, "kind": "file-history-snapshot" },
        ]),
        "checkpoints",
    );
    let history = &session.metadata["fileHistory"];
    assert_eq!(history["sessionId"], "sid-fallback");
    assert_json_eq(
        &history["snapshots"]["snap-x"]["files"],
        &json!({ "/w/a.ts": { "backup": "h@v1" } }),
        "snap-x.files",
    );
    assert_json_eq(
        &history["snapshots"]["inner-y"]["files"],
        &json!({}),
        "inner-y.files",
    );
}

#[test]
fn sessions_without_file_history_expose_no_checkpoint_metadata() {
    let session = from_jsonl(
        &line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
        &source("s"),
    );
    assert!(session.checkpoints.is_empty());
    assert_eq!(session.metadata["fileHistory"], Value::Null);
}

#[test]
fn entries_without_parent_uuid_chain_linearly_and_self_parent_loops_are_safe() {
    let session = from_jsonl(
        &[
            line(json!({ "type": "user", "uuid": "u1", "message": { "content": "one" } })),
            line(json!({ "type": "user", "uuid": "u2", "parentUuid": "u2", "message": { "content": "two" } })),
        ]
        .join("\n"),
        &source("s"),
    );
    assert!(session.nodes[0].parent_node_id.is_none());
    assert_eq!(session.nodes[1].parent_node_id, Some(0));
}

#[test]
fn malformed_and_non_object_lines_are_skipped() {
    let session = from_jsonl(
        &[
            "{\"type\":\"user\"".to_string(),
            "\"just a string\"".to_string(),
            String::new(),
            "   ".to_string(),
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_eq!(session.nodes.len(), 1);
    assert_eq!(session.nodes[0].content, "hi");
}

#[test]
fn is_meta_user_entries_stay_out_of_prompt_history_but_remain_nodes() {
    let session = from_jsonl(
        &[
            line(user_entry(
                "u1",
                Value::Null,
                json!("<local-command-caveat>ran /clear</local-command-caveat>"),
                json!({ "isMeta": true }),
            )),
            line(user_entry(
                "u2",
                json!("u1"),
                json!("real prompt"),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_eq!(session.nodes.len(), 2);
    assert_eq!(session.nodes[0].metadata["isMeta"], true);
    let history: Vec<&str> = session
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(history, ["real prompt"]);
}

#[test]
fn user_entries_without_content_emit_no_node_and_assistant_blocks_fold() {
    let session = from_jsonl(
        &[
            line(json!({ "type": "user", "uuid": "u0", "parentUuid": null, "message": { "role": "user" } })),
            line(user_entry("u1", json!("u0"), json!("go"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([
                    { "type": "redacted_thinking", "data": "opaque" },
                    { "type": "thinking", "thinking": "plan" },
                    { "type": "text", "text": "answer" },
                    42,
                    { "type": "tool_use", "name": "NoId" },
                ]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    // the content-less user entry emitted nothing but stays in the uuid chain
    assert_eq!(session.nodes.len(), 2);
    let assistant = &session.nodes[1];
    assert_eq!(assistant.content, "answer");
    // the redacted block folds to the marker and its opaque blob rides as the
    // thinking signature — the unsealed `thinking` block adds only its text.
    assert_eq!(assistant.thinking.as_deref(), Some("[redacted]\nplan"));
    assert_eq!(assistant.thinking_signature.as_deref(), Some("opaque"));
    assert!(assistant.tool_calls[0].id.starts_with("claude-tool-"));
    assert_eq!(assistant.tool_calls[0].name, "NoId");
    assert_json_eq(&assistant.tool_calls[0].arguments, &json!({}), "arguments");
}

#[test]
fn assistant_without_usage_or_model_leaves_the_options_empty() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(json!({
                "type": "assistant", "uuid": "u2", "parentUuid": "u1",
                "timestamp": "not-a-date",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "ok" }] },
            })),
        ]
        .join("\n"),
        &source("s"),
    );
    let assistant = &session.nodes[1];
    assert!(assistant.usage.is_none());
    assert!(assistant.model.is_none());
    assert!(assistant.finish_reason.is_none());
    assert!(assistant.request_id.is_none());
    // bad timestamp falls back to the session's first timestamp
    assert_eq!(assistant.created_at, session.created_at);
}

#[test]
fn summarize_jsonl_gives_session_meta_without_nodes() {
    let summary = summarize_jsonl(
        &[
            line(json!({ "type": "summary", "summary": "Listed", "leafUuid": "u1" })),
            line(user_entry("u1", Value::Null, json!("do things"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([{ "type": "text", "text": "done" }]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("sess-1"),
    );
    assert_eq!(summary.id, "sess-1");
    assert_eq!(summary.title, "Listed");
    assert_eq!(summary.working_directory, "/work/proj");
    assert_eq!(summary.model, "claude-opus-4-5");
    assert!(summary.nodes.is_empty());
    assert_eq!(summary.main_chain_id, 0);
}

#[test]
fn decode_project_dir_maps_slugs_back_to_paths() {
    assert_eq!(decode_project_dir("-home-luis-proj"), "/home/luis/proj");
    assert_eq!(decode_project_dir("bare"), "/bare");
}

#[test]
fn user_entries_with_no_usable_text_emit_no_node_and_no_history_entry() {
    let session = from_jsonl(
        &[
            line(json!({ "type": "user", "uuid": "u0", "parentUuid": null, "message": { "content": "" } })),
            line(json!({ "type": "user", "uuid": "u1", "parentUuid": "u0", "message": { "content": [] } })),
            line(json!({ "type": "user", "uuid": "u2", "parentUuid": "u1", "message": { "content": 7 } })),
            line(json!({ "type": "user", "uuid": "u3", "parentUuid": "u2" })),
            line(user_entry("u4", json!("u3"), json!("real"), json!({}))),
            line(json!({ "type": "user", "parentUuid": "u4", "message": { "content": "uuid-less" } })),
        ]
        .join("\n"),
        &source("s"),
    );
    let contents: Vec<&str> = session.nodes.iter().map(|n| n.content.as_str()).collect();
    assert_eq!(contents, ["real", "uuid-less"]);
    let history: Vec<&str> = session
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(history, ["real", "uuid-less"]);
    // the uuid-less node chains to the last emitted node
    assert_eq!(session.nodes[1].parent_node_id, Some(0));
}

#[test]
fn assistant_entries_without_a_message_object_emit_an_empty_node() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(json!({ "type": "assistant", "uuid": "u2", "parentUuid": "u1" })),
        ]
        .join("\n"),
        &source("s"),
    );
    let assistant = &session.nodes[1];
    assert_eq!(assistant.role, Role::Assistant);
    assert_eq!(assistant.content, "");
    assert!(assistant.usage.is_none());
}

#[test]
fn assistant_usage_object_without_token_counts_yields_none() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([{ "type": "text", "text": "ok" }]),
                json!({ "usage": { "service_tier": "standard" } }),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    assert!(session.nodes[1].usage.is_none());
}

#[test]
fn sanitization_strips_control_characters_from_content() {
    let session = from_jsonl(
        &line(user_entry(
            "u1",
            Value::Null,
            json!("clean\u{1}me"),
            json!({}),
        )),
        &source("s"),
    );
    assert_eq!(session.nodes[0].content, "cleanme");
}

#[test]
fn numeric_parent_uuid_and_non_string_fields_degrade_to_linear_chaining() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("one"), json!({}))),
            line(json!({ "type": "user", "uuid": "u2", "parentUuid": 3, "message": { "content": "two" } })),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_eq!(session.nodes[1].parent_node_id, Some(0));
}

#[test]
fn system_entries_without_content_or_subtype_get_a_generic_label() {
    let session = from_jsonl(
        &line(json!({ "type": "system", "uuid": "s0", "parentUuid": null })),
        &source("s"),
    );
    assert_eq!(session.nodes[0].role, Role::System);
    assert_eq!(session.nodes[0].content, "[claude system]");
    assert_eq!(session.nodes[0].metadata["subtype"], Value::Null);
}

#[test]
fn tool_use_blocks_without_name_or_id_still_produce_a_call() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("go"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([{ "type": "tool_use", "input": { "x": 1 } }]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let call = &session.nodes[1].tool_calls[0];
    assert_eq!(call.name, "unknown");
    assert!(call.id.starts_with("claude-tool-"));
}

#[test]
fn permission_mode_becomes_agent_mode_and_compact_flags_are_kept() {
    let session = from_jsonl(
        &line(user_entry(
            "u1",
            Value::Null,
            json!("hi"),
            json!({
                "permissionMode": "plan",
                "isCompactSummary": true,
                "isVisibleInTranscriptOnly": true,
            }),
        )),
        &source("s"),
    );
    assert_eq!(session.agent_mode, "plan");
    let meta = &session.nodes[0].metadata;
    assert_eq!(meta["isCompactSummary"], true);
    assert_eq!(meta["isVisibleInTranscriptOnly"], true);
}

#[test]
fn tool_use_result_sidecar_rides_on_the_tool_node_metadata() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("go"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([{ "type": "tool_use", "id": "toolu_t", "name": "Task", "input": { "prompt": "x" } }]),
                json!({}),
            )),
            line(user_entry(
                "u3",
                json!("u2"),
                json!([{ "type": "tool_result", "tool_use_id": "toolu_t", "content": "spawned" }]),
                json!({ "toolUseResult": { "status": "completed", "agentId": "agent-1" } }),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_json_eq(
        &session.nodes[2].metadata["toolUseResult"],
        &json!({ "status": "completed", "agentId": "agent-1" }),
        "toolUseResult",
    );
}

#[test]
fn assistant_content_items_of_an_unknown_type_are_skipped() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(json!({
                "type": "assistant", "uuid": "u2", "parentUuid": "u1",
                "timestamp": "2026-01-01T00:00:01.000Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "image", "source": { "type": "url", "url": "https://x/i.png" } },
                        { "type": "text", "text": "answer" },
                    ],
                },
            })),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_eq!(session.nodes[1].content, "answer");
}

#[test]
fn usage_with_one_sided_counts_and_partial_cache_tiers_defaults_the_rest() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([{ "type": "text", "text": "a" }]),
                json!({ "usage": { "input_tokens": 4 } }),
            )),
            line(assistant_entry(
                "u3",
                "u2",
                json!([{ "type": "text", "text": "b" }]),
                json!({ "usage": { "output_tokens": 9, "cache_creation": { "ephemeral_5m_input_tokens": 3 } } }),
            )),
            line(assistant_entry(
                "u4",
                "u3",
                json!([{ "type": "text", "text": "c" }]),
                json!({ "usage": { "output_tokens": 2, "cache_creation": { "ephemeral_1h_input_tokens": 6 } } }),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    assert_json_eq(
        &serde_json::to_value(&session.nodes[1].usage).unwrap(),
        &json!({ "input": 4, "output": 0 }),
        "usage[1]",
    );
    assert_json_eq(
        &serde_json::to_value(&session.nodes[2].usage).unwrap(),
        &json!({ "input": 0, "output": 9, "cacheWrite": 3 }),
        "usage[2]",
    );
    assert_json_eq(
        &serde_json::to_value(&session.nodes[3].usage).unwrap(),
        &json!({ "input": 0, "output": 2, "cacheWrite": 6 }),
        "usage[3]",
    );
}

#[test]
fn tool_result_without_content_yields_empty_text_and_text_blocks_without_text_drop() {
    let session = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("hi"), json!({}))),
            line(user_entry(
                "u2",
                json!("u1"),
                json!([
                    { "type": "tool_result", "tool_use_id": "t1" },
                    { "type": "text" },
                    { "type": "text", "text": "kept" },
                ]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let tool = session.nodes.iter().find(|n| n.role == Role::Tool);
    assert!(tool.is_some());
    assert_eq!(tool.unwrap().content, "");
    // the text block with no text field folded away; only "kept" survives on
    // the user node the same entry produced
    let user_contents: Vec<&str> = session
        .nodes
        .iter()
        .filter(|n| n.role == Role::User)
        .map(|n| n.content.as_str())
        .collect();
    assert!(user_contents.contains(&"kept"));
}

/* ---- writer ---- */

fn node(role: Role, content: &str, node_id: i64) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id: None,
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
        created_at: 1_700_000_000.0,
        metadata: Value::Null,
    }
}

fn writer_session(nodes: Vec<MessageNode>, metadata: Value) -> Session {
    let main_chain_id = nodes.len() as i64 - 1;
    Session {
        id: "sess-1".into(),
        title: "Title".into(),
        working_directory: "/work/proj".into(),
        backend_type: "claude".into(),
        agent_mode: "accept-edits".into(),
        model: "session-model".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_010.0,
        main_chain_id,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: Vec::new(),
        metadata,
        nodes,
        prompt_history: Vec::new(),
    }
}

fn entries_of(jsonl: &str) -> Vec<Value> {
    jsonl
        .trim()
        .split('\n')
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn to_jsonl_writes_every_attachment_block_form_and_drops_unmappable_ones() {
    let mut writer = writer_session(vec![node(Role::User, "see attachments", 0)], json!({}));
    writer.nodes[0].blocks = vec![
        Block::Text {
            text: "see attachments".into(),
        },
        Block::Image {
            data: Some("AAA=".into()),
            mime_type: Some("image/jpeg".into()),
            uri: None,
        },
        Block::Image {
            data: Some("BBB=".into()),
            mime_type: None,
            uri: None,
        },
        Block::Image {
            data: None,
            mime_type: None,
            uri: Some("https://x/img.png".into()),
        },
        Block::Image {
            data: None,
            mime_type: None,
            uri: None,
        },
        Block::File {
            text: Some("doc body".into()),
            name: Some("notes.txt".into()),
            uri: None,
            mime_type: None,
            size: None,
            data: None,
        },
        Block::File {
            text: Some("md".into()),
            mime_type: Some("text/markdown".into()),
            uri: None,
            name: None,
            size: None,
            data: None,
        },
        Block::File {
            data: Some("RA==".into()),
            name: Some("blob.bin".into()),
            uri: None,
            mime_type: None,
            size: None,
            text: None,
        },
        Block::File {
            data: Some("Ug==".into()),
            uri: None,
            name: None,
            mime_type: None,
            size: None,
            text: None,
        },
        Block::File {
            uri: Some("https://x/doc.pdf".into()),
            name: Some("doc.pdf".into()),
            mime_type: None,
            size: None,
            text: None,
            data: None,
        },
        Block::File {
            uri: None,
            name: None,
            mime_type: None,
            size: None,
            text: None,
            data: None,
        },
        Block::Audio {
            data: Some("QQ==".into()),
            mime_type: None,
        },
    ];

    let entries = entries_of(&to_jsonl(&writer));
    let entry = entries.iter().find(|e| e["type"] == "user").unwrap();
    assert_json_eq(
        &entry["message"]["content"],
        &json!([
            { "type": "text", "text": "see attachments" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "AAA=" } },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "BBB=" } },
            { "type": "image", "source": { "type": "url", "url": "https://x/img.png" } },
            { "type": "document", "source": { "type": "text", "media_type": "text/plain", "text": "doc body" }, "title": "notes.txt" },
            { "type": "document", "source": { "type": "text", "media_type": "text/markdown", "text": "md" } },
            { "type": "document", "source": { "type": "base64", "media_type": "application/octet-stream", "data": "RA==" }, "title": "blob.bin" },
            { "type": "document", "source": { "type": "base64", "media_type": "application/octet-stream", "data": "Ug==" } },
            { "type": "document", "source": { "type": "url", "url": "https://x/doc.pdf" }, "title": "doc.pdf" },
        ]),
        "content",
    );
}

#[test]
fn to_jsonl_falls_back_to_node_content_and_keeps_flag_metadata() {
    let mut n = node(Role::User, "meta entry", 0);
    n.metadata = json!({
        "uuid": "u-flagged",
        "isMeta": true,
        "isCompactSummary": true,
        "isVisibleInTranscriptOnly": true,
    });
    let writer = writer_session(vec![n], json!({}));
    let entries = entries_of(&to_jsonl(&writer));
    let entry = entries.iter().find(|e| e["type"] == "user").unwrap();
    assert_eq!(entry["isMeta"], true);
    assert_eq!(entry["isCompactSummary"], true);
    assert_eq!(entry["isVisibleInTranscriptOnly"], true);
    assert_eq!(entry["uuid"], "u-flagged");
    assert_eq!(entry["message"]["content"], "meta entry");
}

#[test]
fn to_jsonl_mints_uuids_for_nodes_without_one_and_emits_parent_links() {
    let mut second = node(Role::User, "second", 1);
    second.parent_node_id = Some(0);
    second.metadata = json!(42);
    let mut orphan = node(Role::User, "orphan", 2);
    orphan.parent_node_id = Some(99);
    let writer = writer_session(
        vec![node(Role::User, "first", 0), second, orphan],
        json!({}),
    );
    let all_entries = entries_of(&to_jsonl(&writer));
    let entries: Vec<&Value> = all_entries.iter().filter(|e| e["type"] == "user").collect();
    assert!(entries[0]["uuid"].is_string());
    assert!(entries[1]["uuid"].is_string());
    assert_ne!(entries[0]["uuid"], entries[1]["uuid"]);
    assert_eq!(entries[1]["parentUuid"], entries[0]["uuid"]);
    assert_eq!(entries[2]["parentUuid"], Value::Null);
}

#[test]
fn to_jsonl_writes_tool_nodes_with_recorded_sidecar_and_empty_id_fallback() {
    let mut tool = node(Role::Tool, "output", 1);
    tool.parent_node_id = Some(0);
    tool.tool_call_id = Some("toolu_1".into());
    tool.tool_result = Some(sepia_core::domain::ToolResultInfo {
        status: ToolCallStatus::Error,
        exit_code: None,
        duration_ms: None,
    });
    tool.metadata = json!({ "toolUseResult": { "status": "completed" } });
    let bare = node(Role::Tool, "no id", 2);
    let writer = writer_session(vec![node(Role::User, "run", 0), tool, bare], json!({}));
    let all_entries = entries_of(&to_jsonl(&writer));
    let entries: Vec<&Value> = all_entries.iter().filter(|e| e["type"] == "user").collect();
    assert_json_eq(
        &entries[1]["toolUseResult"],
        &json!({ "status": "completed" }),
        "toolUseResult",
    );
    let content = &entries[1]["message"]["content"][0];
    assert_eq!(content["type"], "tool_result");
    assert_eq!(content["tool_use_id"], "toolu_1");
    assert_eq!(content["content"], "output");
    assert_eq!(content["is_error"], true);
    let bare_content = &entries[2]["message"]["content"][0];
    assert_eq!(bare_content["tool_use_id"], "");
    assert_eq!(bare_content["is_error"], false);
}

#[test]
fn to_jsonl_assistant_entries_carry_model_request_usage_fallbacks() {
    let mut first = node(Role::Assistant, "", 1);
    first.parent_node_id = Some(0);
    first.thinking = Some(REDACTED_THINKING.into());
    first.thinking_signature = Some("blob".into());
    first.tool_calls = vec![sepia_core::domain::ToolCall {
        id: "c1".into(),
        name: "Bash".into(),
        arguments: json!({}),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: Vec::new(),
        diffs: Vec::new(),
    }];
    first.request_id = Some("req_9".into());
    first.metadata = json!({ "messageId": "msg_recorded" });
    first.usage = Some(sepia_core::domain::TokenUsage {
        input: 3.0,
        output: 1.0,
        ..Default::default()
    });
    let mut second = node(Role::Assistant, "plain", 2);
    second.model = Some("node-model".into());
    second.finish_reason = Some("max_tokens".into());
    second.usage = Some(sepia_core::domain::TokenUsage {
        input: 1.0,
        output: 2.0,
        cache_read: Some(5.0),
        cache_write: Some(7.0),
        thinking: None,
        cost: None,
    });
    let writer = writer_session(
        vec![node(Role::User, "go", 0), first, second],
        json!({ "gitBranch": "dev", "claudeVersion": "2.2.0", "slug": "sluggy" }),
    );
    let entries = entries_of(&to_jsonl(&writer));
    let assistants: Vec<&Value> = entries
        .iter()
        .filter(|e| e["type"] == "assistant")
        .collect();
    let first = assistants[0];
    assert_eq!(first["message"]["id"], "msg_recorded");
    assert_eq!(first["message"]["model"], "session-model");
    assert_eq!(first["message"]["stop_reason"], "tool_use");
    assert_json_eq(
        &first["message"]["usage"],
        &json!({ "input_tokens": 3, "output_tokens": 1 }),
        "usage",
    );
    assert_json_eq(
        &first["message"]["content"],
        &json!([
            { "type": "redacted_thinking", "data": "blob" },
            { "type": "tool_use", "id": "c1", "name": "Bash", "input": {} },
        ]),
        "content",
    );
    assert_eq!(first["requestId"], "req_9");
    assert_eq!(first["gitBranch"], "dev");
    assert_eq!(first["version"], "2.2.0");
    assert_eq!(first["slug"], "sluggy");

    let second = assistants[1];
    assert_eq!(second["message"]["model"], "node-model");
    assert_eq!(second["message"]["stop_reason"], "max_tokens");
    assert_json_eq(
        &second["message"]["usage"],
        &json!({
            "input_tokens": 1,
            "output_tokens": 2,
            "cache_read_input_tokens": 5,
            "cache_creation_input_tokens": 7,
        }),
        "usage",
    );
    // no recorded messageId — one is minted from the entry uuid
    assert!(
        second["message"]["id"]
            .as_str()
            .unwrap()
            .starts_with("msg_")
    );
    assert_eq!(second["requestId"], Value::Null);
}

#[test]
fn to_jsonl_writes_system_nodes_with_subtype_level_compact_metadata() {
    let mut init = node(Role::System, "init banner", 0);
    init.metadata =
        json!({ "subtype": "init", "level": "info", "compactMetadata": { "preTokens": 10 } });
    let bare = node(Role::System, "bare", 1);
    let writer = writer_session(vec![init, bare], json!({}));
    let entries = entries_of(&to_jsonl(&writer));
    let systems: Vec<&Value> = entries.iter().filter(|e| e["type"] == "system").collect();
    assert_eq!(systems[0]["subtype"], "init");
    assert_eq!(systems[0]["content"], "init banner");
    assert_eq!(systems[0]["level"], "info");
    assert_json_eq(
        &systems[0]["compactMetadata"],
        &json!({ "preTokens": 10 }),
        "compactMetadata",
    );
    // no recorded subtype — the entry defaults to "init" with no extras
    assert_eq!(systems[1]["subtype"], "init");
    assert_eq!(systems[1]["content"], "bare");
    assert_eq!(systems[1]["level"], Value::Null);
    assert_eq!(systems[1]["compactMetadata"], Value::Null);
}

#[test]
fn to_jsonl_round_trips_through_from_jsonl() {
    let original = from_jsonl(
        &[
            line(json!({ "type": "summary", "summary": "Fix the login bug", "leafUuid": "u4" })),
            line(user_entry("u1", Value::Null, json!("fix the login bug please"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([
                    { "type": "thinking", "thinking": "look at auth", "signature": "sig-1" },
                    { "type": "text", "text": "I'll check the auth module" },
                    { "type": "tool_use", "id": "toolu_1", "name": "Read", "input": { "file_path": "/src/a.ts" } },
                ]),
                json!({}),
            )),
            line(user_entry(
                "u3",
                json!("u2"),
                json!([{ "type": "tool_result", "tool_use_id": "toolu_1", "content": "file contents" }]),
                json!({}),
            )),
            line(assistant_entry(
                "u4",
                "u3",
                json!([{ "type": "text", "text": "done" }]),
                json!({ "usage": { "input_tokens": 5, "output_tokens": 2 } }),
            )),
        ]
        .join("\n"),
        &source("sess-1"),
    );

    let written = to_jsonl(&original);
    let entries = entries_of(&written);
    assert_eq!(entries[0]["type"], "summary");
    assert_eq!(entries[0]["summary"], "Fix the login bug");
    let uuids: Vec<&str> = entries
        .iter()
        .filter(|e| e["type"] != "summary")
        .map(|e| e["uuid"].as_str().unwrap())
        .collect();
    let unique: std::collections::BTreeSet<_> = uuids.iter().collect();
    assert_eq!(unique.len(), uuids.len());

    let reread = from_jsonl(&written, &source("sess-1"));
    assert_eq!(reread.title, "Fix the login bug");
    assert_eq!(reread.working_directory, "/work/proj");
    assert_eq!(reread.model, "claude-opus-4-5");
    let roles: Vec<Role> = reread.nodes.iter().map(|n| n.role).collect();
    assert_eq!(
        roles,
        [Role::User, Role::Assistant, Role::Tool, Role::Assistant]
    );

    let [user, assistant, tool, last] = &reread.nodes[..] else {
        panic!("expected 4 nodes");
    };
    assert_eq!(user.content, "fix the login bug please");
    assert!(user.parent_node_id.is_none());
    // recorded uuids survive, so the parent links are identical
    assert_eq!(user.metadata["uuid"], "u1");
    assert_eq!(assistant.metadata["uuid"], "u2");
    assert_eq!(assistant.parent_node_id, Some(user.node_id));
    assert_eq!(assistant.thinking.as_deref(), Some("look at auth"));
    assert_eq!(assistant.thinking_signature.as_deref(), Some("sig-1"));
    assert_eq!(assistant.tool_calls[0].id, "toolu_1");
    assert_eq!(assistant.tool_calls[0].name, "Read");
    assert_json_eq(
        &assistant.tool_calls[0].arguments,
        &json!({ "file_path": "/src/a.ts" }),
        "arguments",
    );
    assert_json_eq(
        &serde_json::to_value(&assistant.usage).unwrap(),
        &json!({ "input": 100, "output": 40, "cacheRead": 12, "cacheWrite": 8 }),
        "usage",
    );
    assert_eq!(tool.role, Role::Tool);
    assert_eq!(tool.tool_call_id.as_deref(), Some("toolu_1"));
    assert_eq!(tool.content, "file contents");
    assert_json_eq(
        &serde_json::to_value(&last.usage).unwrap(),
        &json!({ "input": 5, "output": 2 }),
        "usage",
    );
    let history: Vec<&str> = reread
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(history, ["fix the login bug please"]);
}

#[test]
fn to_jsonl_marks_subagent_sessions_is_sidechain_with_the_parent_session_id() {
    let mut subagent = writer_session(vec![node(Role::User, "sub task", 0)], json!({}));
    subagent.id = "agent-9".into();
    subagent.parent_session_id = Some("sess-1".into());
    subagent.agent_id = Some("agent-9".into());
    for entry in entries_of(&to_jsonl(&subagent)) {
        if entry["type"] == "summary" {
            continue;
        }
        assert_eq!(entry["isSidechain"], true);
        assert_eq!(entry["sessionId"], "sess-1");
        assert_eq!(entry["agentId"], "agent-9");
    }
}

#[test]
fn to_jsonl_drops_unsigned_thinking_and_a_redacted_marker_echoes_its_blob() {
    let unsigned = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("go"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([
                    { "type": "thinking", "thinking": "unsigned plan" },
                    { "type": "text", "text": "answer" },
                ]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let entries = entries_of(&to_jsonl(&unsigned));
    let assistant = entries.iter().find(|e| e["type"] == "assistant").unwrap();
    // no signature on the node — the thinking block can't replay, so it drops
    assert_json_eq(
        &assistant["message"]["content"],
        &json!([{ "type": "text", "text": "answer" }]),
        "unsigned content",
    );

    let redacted = from_jsonl(
        &[
            line(user_entry("u1", Value::Null, json!("go"), json!({}))),
            line(assistant_entry(
                "u2",
                "u1",
                json!([
                    { "type": "redacted_thinking", "data": "opaque-blob" },
                    { "type": "text", "text": "answer" },
                ]),
                json!({}),
            )),
        ]
        .join("\n"),
        &source("s"),
    );
    let entries = entries_of(&to_jsonl(&redacted));
    let sealed = entries.iter().find(|e| e["type"] == "assistant").unwrap();
    assert_json_eq(
        &sealed["message"]["content"],
        &json!([
            { "type": "redacted_thinking", "data": "opaque-blob" },
            { "type": "text", "text": "answer" },
        ]),
        "sealed content",
    );
}
