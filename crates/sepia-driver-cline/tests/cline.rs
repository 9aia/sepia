#![allow(clippy::unwrap_used, clippy::pedantic, clippy::too_many_arguments)]

//! `Cline.test.ts` + `ClineExtra.test.ts` ports — import parsing, the
//! tool-call/result alignment, manifest + transcript writers, and the
//! transcript-validity checker.

use std::path::{Path, PathBuf};

use sepia_core::domain::{
    Block, CheckpointRef, MessageNode, REDACTED_THINKING, Role, Session, TokenUsage, ToolCall,
    ToolCallDiff, ToolCallLocation, ToolCallStatus, ToolResultInfo,
};
use sepia_driver_cline::{cline, cline_index};
use serde_json::{Value, json};

fn make_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    role: Role,
    content: &str,
    tool_calls: Vec<ToolCall>,
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
    tool_result: Option<ToolResultInfo>,
    thinking: Option<&str>,
    thinking_signature: Option<&str>,
    metadata: Value,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
        role,
        content: content.into(),
        blocks: Vec::new(),
        tool_calls,
        tool_call_id: tool_call_id.map(str::to_string),
        tool_name: tool_name.map(str::to_string),
        thinking: thinking.map(str::to_string),
        thinking_signature: thinking_signature.map(str::to_string),
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result,
        created_at: 1_700_000_000.0 + node_id as f64,
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

fn make_session(nodes: Vec<MessageNode>) -> Session {
    Session {
        id: "imported-session".into(),
        title: "Imported session".into(),
        working_directory: "/work".into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: "swe-2-high".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_300.0,
        main_chain_id: nodes.len() as i64,
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

fn rendered() -> Value {
    json!({ "summarized_from": null, "num_tokens_preceding": 12, "is_system_prefix": null })
}

fn sample_nodes() -> Vec<MessageNode> {
    vec![
        make_node(
            0,
            None,
            Role::System,
            "system prompt",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            1,
            None,
            Role::User,
            "do the thing",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            2,
            Some(1),
            Role::Assistant,
            "working on it",
            vec![tool_call(
                "call_1",
                "read",
                json!({ "file_path": "/work/a.ts" }),
            )],
            None,
            None,
            None,
            None,
            None,
            rendered(),
        ),
        make_node(
            3,
            Some(2),
            Role::Tool,
            "file body",
            vec![],
            Some("call_1"),
            Some("read"),
            None,
            None,
            None,
            json!({ "toolArguments": { "file_path": "/work/a.ts" } }),
        ),
        make_node(
            4,
            Some(3),
            Role::Assistant,
            "done",
            vec![],
            None,
            None,
            None,
            None,
            None,
            rendered(),
        ),
        make_node(
            5,
            Some(4),
            Role::Assistant,
            "next step",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            6,
            Some(4),
            Role::Assistant,
            "next step",
            vec![],
            None,
            None,
            None,
            None,
            None,
            rendered(),
        ),
    ]
}

fn write_cline_dir(
    root: &Path,
    id: &str,
    messages: &Value,
    manifest: &Value,
    messages_content: Option<&str>,
) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut base = json!({
        "version": 1,
        "session_id": id,
        "cwd": "/work",
        "started_at": "2026-09-01T00:00:00.000Z",
        "ended_at": "2026-09-01T00:01:00.000Z",
        "status": "completed",
        "model": "cline-pass/swe-2-high",
        "metadata": { "title": "imported" },
    });
    if let (Some(b), Some(extra)) = (base.as_object_mut(), manifest.as_object()) {
        for (k, v) in extra {
            b.insert(k.clone(), v.clone());
        }
    }
    std::fs::write(
        dir.join(format!("{id}.messages.json")),
        messages_content.map_or_else(
            || json!({ "version": 1, "sessionId": id, "messages": messages }).to_string(),
            str::to_string,
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&base).unwrap(),
    )
    .unwrap();
    dir
}

fn load(root: &Path, id: &str) -> Session {
    cline::from_directory(&root.join(id), None).unwrap()
}

fn tool_result_contents(session: &Session) -> Vec<String> {
    cline::visible_nodes(session)
        .iter()
        .filter(|n| n.role == Role::Tool)
        .map(|n| n.content.clone())
        .collect()
}

fn messages_of(payload: &Value) -> &Vec<Value> {
    payload["messages"].as_array().unwrap()
}

#[test]
fn visible_nodes_keeps_assistants_and_collapses_rendered_twins() {
    let visible = cline::visible_nodes(&make_session(sample_nodes()));
    // system node dropped, rendered twin of node 5 (node 6) dropped.
    let ids: Vec<i64> = visible.iter().map(|n| n.node_id).collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);
}

#[test]
fn to_directory_exports_a_resumable_pair() {
    let out = tempfile::tempdir().unwrap();
    let session = make_session(sample_nodes());
    let actions = cline::to_directory(&session, out.path(), false, false).unwrap();
    assert_eq!(
        actions,
        [cline::ExportAction::Created, cline::ExportAction::Created]
    );

    let meta: Value = serde_json::from_str(
        &std::fs::read_to_string(out.path().join("imported-session.json")).unwrap(),
    )
    .unwrap();
    let data: Value = serde_json::from_str(
        &std::fs::read_to_string(out.path().join("imported-session.messages.json")).unwrap(),
    )
    .unwrap();
    let messages = messages_of(&data);

    // manifest carries every field the Cline CLI requires to resume by id.
    assert_eq!(meta["session_id"], "imported-session");
    assert_eq!(meta["cwd"], "/work");
    assert_eq!(meta["workspace_root"], "/work");
    assert_eq!(meta["status"], "completed");
    assert_eq!(meta["pid"], 0);
    assert_eq!(meta["enable_tools"], true);
    assert_eq!(meta["enable_spawn"], true);
    assert_eq!(meta["enable_teams"], true);
    assert_eq!(meta["prompt"], "do the thing");
    assert_eq!(meta["metadata"]["title"], "Imported session");
    assert_eq!(
        meta["messages_path"],
        out.path()
            .join("imported-session.messages.json")
            .to_string_lossy()
            .to_string()
    );

    assert_eq!(data["sessionId"], "imported-session");
    assert_eq!(messages.len(), 5);
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        ["user", "assistant", "user", "assistant", "assistant"]
    );

    // assistant text survives …
    assert_eq!(
        messages[3]["content"],
        json!([{ "type": "text", "text": "done" }])
    );
    assert_eq!(
        messages[3]["modelInfo"],
        json!({ "id": "swe-2-high", "provider": "cline-pass" })
    );

    // … and tool calls stay paired with their results.
    assert_eq!(
        messages[1]["content"][1],
        json!({
            "type": "tool_use",
            "id": "call_1",
            "name": "read_files",
            "input": { "files": [{ "path": "/work/a.ts" }] },
        })
    );
    assert_eq!(
        messages[2]["content"],
        json!([{
            "type": "tool_result",
            "tool_use_id": "call_1",
            "name": "read_files",
            "content": [{ "query": "/work/a.ts", "result": "file body", "success": true }],
        }])
    );

    // no turn may land between a tool call and its result.
    assert_eq!(cline::transcript_violations(messages), vec![]);
}

#[test]
fn session_messages_pairs_uncaptured_call_with_placeholder() {
    let session = make_session(vec![
        make_node(
            0,
            None,
            Role::User,
            "do the thing",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            1,
            Some(0),
            Role::Assistant,
            "on it",
            vec![
                tool_call("call_1", "read", json!({ "file_path": "/work/a.ts" })),
                tool_call("call_2", "exec", json!({ "command": "pwd" })),
            ],
            None,
            None,
            None,
            None,
            None,
            rendered(),
        ),
        make_node(
            2,
            Some(1),
            Role::Tool,
            "file body",
            vec![],
            Some("call_1"),
            Some("read"),
            None,
            None,
            None,
            json!({ "toolArguments": { "file_path": "/work/a.ts" } }),
        ),
        make_node(
            3,
            Some(2),
            Role::Assistant,
            "done",
            vec![],
            None,
            None,
            None,
            None,
            None,
            rendered(),
        ),
    ]);

    let out = cline::session_messages(&session, "imported-session");
    let messages = messages_of(&out);

    assert_eq!(cline::transcript_violations(messages), vec![]);
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
    assert_eq!(
        messages[2]["content"],
        json!([
            {
                "type": "tool_result",
                "tool_use_id": "call_1",
                "name": "read_files",
                "content": [{ "query": "/work/a.ts", "result": "file body", "success": true }],
            },
            {
                "type": "tool_result",
                "tool_use_id": "call_2",
                "name": "run_commands",
                "content": cline::UNCAPTURED_TOOL_RESULT,
            },
        ])
    );
}

#[test]
fn session_messages_hoists_a_result_recorded_behind_a_user_turn() {
    let session = make_session(vec![
        make_node(
            0,
            None,
            Role::User,
            "do the thing",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            1,
            Some(0),
            Role::Assistant,
            "",
            vec![tool_call(
                "call_1",
                "read",
                json!({ "file_path": "/work/a.ts" }),
            )],
            None,
            None,
            None,
            None,
            None,
            rendered(),
        ),
        make_node(
            2,
            Some(1),
            Role::User,
            "continue",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            3,
            Some(1),
            Role::Tool,
            "file body",
            vec![],
            Some("call_1"),
            Some("read"),
            None,
            None,
            None,
            json!({ "toolArguments": { "file_path": "/work/a.ts" } }),
        ),
    ]);

    let out = cline::session_messages(&session, "imported-session");
    let messages = messages_of(&out);

    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user", "user"]);
    assert_eq!(
        messages[2]["content"],
        json!([{
            "type": "tool_result",
            "tool_use_id": "call_1",
            "name": "read_files",
            "content": [{ "query": "/work/a.ts", "result": "file body", "success": true }],
        }])
    );
    assert_eq!(
        messages[3]["content"],
        json!([{ "type": "text", "text": "continue" }])
    );
    assert_eq!(cline::transcript_violations(messages), vec![]);
}

#[test]
fn session_messages_writes_thinking_seals_back_verbatim() {
    let session = make_session(vec![
        make_node(
            0,
            None,
            Role::User,
            "go",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            1,
            Some(0),
            Role::Assistant,
            "done",
            vec![],
            None,
            None,
            None,
            Some("ponder"),
            Some("sealed.v1.sig"),
            rendered(),
        ),
        make_node(
            2,
            Some(1),
            Role::Assistant,
            "ok",
            vec![],
            None,
            None,
            None,
            Some(REDACTED_THINKING),
            Some("opaque-blob"),
            rendered(),
        ),
    ]);

    let out = cline::session_messages(&session, "imported-session");
    let messages = messages_of(&out);
    let assistants: Vec<&Value> = messages
        .iter()
        .filter(|m| m["role"] == "assistant")
        .collect();

    let blocks0 = assistants[0]["content"].as_array().unwrap();
    assert!(blocks0.contains(&json!({
        "type": "thinking",
        "thinking": "ponder",
        "signature": "sealed.v1.sig",
    })));
    // a marker-only thinking + opaque blob was a redacted block — write it as one
    let blocks1 = assistants[1]["content"].as_array().unwrap();
    assert!(blocks1.contains(&json!({
        "type": "redacted_thinking",
        "data": "opaque-blob",
    })));
}

#[test]
fn import_records_editor_diffs_locations_and_manifest_checkpoints() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "checkpointed",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_e", "name": "editor",
                      "input": { "path": "/work/a.ts", "old_text": "before", "new_text": "after" } },
                    { "type": "tool_use", "id": "call_w", "name": "editor",
                      "input": { "path": "/work/b.ts", "new_text": "body" } },
                    { "type": "tool_use", "id": "call_r", "name": "read_files",
                      "input": { "files": [{ "path": "/work/c.ts" }] } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_e", "name": "editor", "content": "ok" },
                    { "type": "tool_result", "tool_use_id": "call_w", "name": "editor", "content": "ok" },
                    { "type": "tool_result", "tool_use_id": "call_r", "name": "read_files",
                      "content": [{ "query": "/work/c.ts", "result": "body", "success": true }] },
                ],
                "ts": 3,
            },
        ]),
        &json!({
            "metadata": {
                "title": "imported",
                "checkpoint": {
                    "latest": { "ref": "bbb", "createdAt": 200, "runCount": 2, "kind": "commit" },
                    "history": [{ "ref": "aaa", "createdAt": 100, "runCount": 1, "kind": "stash" }, "junk"],
                },
            },
        }),
        None,
    );
    let session = load(root.path(), "checkpointed");

    // `latest` isn't in this manifest's history — it is appended, not lost.
    assert_eq!(
        session.checkpoints,
        vec![
            CheckpointRef {
                r#ref: "aaa".into(),
                created_at: 100.0,
                run_count: Some(1),
                kind: Some("stash".into()),
            },
            CheckpointRef {
                r#ref: "bbb".into(),
                created_at: 200.0,
                run_count: Some(2),
                kind: Some("commit".into()),
            },
        ]
    );

    let visible = cline::visible_nodes(&session);
    let calls: Vec<&ToolCall> = visible.iter().flat_map(|n| n.tool_calls.iter()).collect();
    let edit = calls.iter().find(|c| c.name == "edit").unwrap();
    assert_eq!(
        edit.locations,
        vec![ToolCallLocation {
            path: "/work/a.ts".into(),
            line: None
        }]
    );
    assert_eq!(
        edit.diffs,
        vec![ToolCallDiff {
            path: "/work/a.ts".into(),
            old_text: Some("before".into()),
            new_text: Some("after".into()),
        }]
    );

    // A create (`old_text` absent) records a newText-only diff.
    let write = calls.iter().find(|c| c.name == "write").unwrap();
    assert_eq!(
        write.diffs,
        vec![ToolCallDiff {
            path: "/work/b.ts".into(),
            old_text: None,
            new_text: Some("body".into()),
        }]
    );
    assert_eq!(
        write.locations,
        vec![ToolCallLocation {
            path: "/work/b.ts".into(),
            line: None
        }]
    );

    let read = calls.iter().find(|c| c.name == "read").unwrap();
    assert_eq!(
        read.locations,
        vec![ToolCallLocation {
            path: "/work/c.ts".into(),
            line: None
        }]
    );
    assert_eq!(read.diffs, vec![]);

    // The manifest write-back restores the Cline-native {latest, history} blob.
    let manifest = cline::session_manifest(&session, "checkpointed", "/m");
    let history = manifest["metadata"]["checkpoint"]["history"]
        .as_array()
        .unwrap();
    let refs: Vec<&str> = history.iter().map(|e| e["ref"].as_str().unwrap()).collect();
    assert_eq!(refs, ["aaa", "bbb"]);
    assert_eq!(manifest["metadata"]["checkpoint"]["latest"]["ref"], "bbb");

    // Sessions that never checkpointed write no checkpoint metadata at all.
    let plain = cline::session_manifest(&make_session(sample_nodes()), "plain", "/m");
    assert!(plain["metadata"].get("checkpoint").is_none());
}

#[test]
fn import_keeps_thinking_signatures_and_redacted_blobs_verbatim() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "sealed",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "ponder", "signature": "sealed.v1.sig" },
                    { "type": "text", "text": "done" },
                ],
                "ts": 2,
            },
            {
                "id": "a1",
                "role": "assistant",
                "content": [
                    { "type": "redacted_thinking", "data": "opaque-blob" },
                    { "type": "text", "text": "ok" },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "sealed");

    let visible = cline::visible_nodes(&session);
    let assistants: Vec<&MessageNode> = visible
        .iter()
        .filter(|n| n.role == Role::Assistant)
        .collect();
    let (sealed, redacted) = (assistants[0], assistants[1]);
    assert_eq!(sealed.thinking.as_deref(), Some("ponder"));
    assert_eq!(sealed.thinking_signature.as_deref(), Some("sealed.v1.sig"));
    // the redacted blob has no text — the marker stands in, `data` is the seal
    assert_eq!(redacted.thinking.as_deref(), Some(REDACTED_THINKING));
    assert_eq!(redacted.thinking_signature.as_deref(), Some("opaque-blob"));

    // … and an export writes both back in provider shape.
    let out = cline::session_messages(&session, "sealed");
    let messages = messages_of(&out);
    let blocks: Vec<&Value> = messages
        .iter()
        .filter(|m| m["role"] == "assistant")
        .flat_map(|m| m["content"].as_array().unwrap().iter())
        .collect();
    assert!(blocks.contains(&&json!({
        "type": "thinking",
        "thinking": "ponder",
        "signature": "sealed.v1.sig",
    })));
    assert!(blocks.contains(&&json!({ "type": "redacted_thinking", "data": "opaque-blob" })));
}

#[test]
fn import_keeps_calls_whose_list_fields_arrived_as_strings() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "stringy",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "run_commands",
                      "input": { "commands": "[\"pwd\", \"ls\"]" } },
                    { "type": "tool_use", "id": "call_2", "name": "search_codebase",
                      "input": { "queries": ":=" } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "run_commands",
                      "content": [
                          { "query": "pwd", "result": "/work", "success": true },
                          { "query": "ls", "result": "a.ts", "success": true },
                      ] },
                    { "type": "tool_result", "tool_use_id": "call_2", "name": "search_codebase",
                      "content": [{ "query": ":=", "result": "x := 1", "success": true }] },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "stringy");

    // a JSON-encoded list and a bare string both still describe the calls to make
    assert_eq!(
        tool_result_contents(&session),
        vec!["/work", "a.ts", "x := 1"]
    );

    let out = cline::session_messages(&session, "stringy");
    let messages = messages_of(&out);
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user"]);
    let uses: Vec<(&str, &Value)> = messages[1]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "tool_use")
        .map(|b| (b["name"].as_str().unwrap(), &b["input"]))
        .collect();
    assert_eq!(
        uses,
        [
            ("run_commands", &json!({ "commands": ["pwd"] })),
            ("run_commands", &json!({ "commands": ["ls"] })),
            ("search_codebase", &json!({ "queries": [":="] })),
        ]
    );
    let result_bodies: Vec<&Value> = messages[2]["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| &b["content"])
        .collect();
    assert_eq!(
        result_bodies,
        [
            &json!([{ "query": "pwd", "result": "/work", "success": true }]),
            &json!([{ "query": "ls", "result": "a.ts", "success": true }]),
            &json!([{ "query": ":=", "result": "x := 1", "success": true }]),
        ]
    );
}

#[test]
fn import_pairs_each_result_entry_with_its_call_when_the_key_differs() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "ranges",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "read both" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "read_files",
                      "input": { "files": [{ "path": "/work/a.ts" }, { "path": "/work/b.ts" }] } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "read_files",
                      "content": [
                          { "query": "/work/a.ts:1-40", "result": "a body", "success": true },
                          { "query": "/work/b.ts:5-9", "result": "b body", "success": true },
                      ] },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "ranges");

    // the entries answer a line range the call never mentioned, yet each call
    // still keeps its own output instead of an empty result
    assert_eq!(tool_result_contents(&session), vec!["a body", "b body"]);
}

#[test]
fn import_fills_calls_from_result_entries_that_carry_no_key_at_all() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "unkeyed",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "run both" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "run_commands",
                      "input": { "commands": ["pwd", "ls"] } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "run_commands",
                      "content": [{ "result": "first" }, { "result": "second" }] },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "unkeyed");

    assert_eq!(tool_result_contents(&session), vec!["first", "second"]);
}

#[test]
fn import_reads_the_shape_variants_a_call_can_arrive_in() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "variants",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "run_commands",
                      "input": { "command": "pwd" } },
                    { "type": "tool_use", "id": "call_2", "name": "read_files",
                      "input": { "path": "/work/a.ts", "start_line": 10, "end_line": 20 } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "run_commands",
                      "content": [{ "query": "pwd", "result": "/work", "success": true }] },
                    { "type": "tool_result", "tool_use_id": "call_2", "name": "read_files",
                      "content": [{ "query": "/work/a.ts:10-20", "result": "a body", "success": true }] },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "variants");

    // a single `command` instead of `commands`, and a `path` with a line range
    assert_eq!(tool_result_contents(&session), vec!["/work", "a body"]);
}

#[test]
fn import_keeps_a_call_whose_input_carries_nothing_readable() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "unreadable",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "run_commands",
                      "input": { "description": "check the build" } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "run_commands",
                      "content": "done" },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "unreadable");

    // the call is kept as it arrived, so its output still has a caller
    assert_eq!(tool_result_contents(&session), vec!["done"]);

    let out = cline::session_messages(&session, "unreadable");
    let messages = messages_of(&out);
    let uses: Vec<&Value> = messages[1]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "tool_use")
        .collect();
    assert_eq!(uses.len(), 1);
    assert!(uses[0]["id"].as_str().is_some());
    assert_eq!(uses[0]["name"], "run_commands");
    assert_eq!(
        uses[0]["input"],
        json!({ "description": "check the build" })
    );
    assert_eq!(cline::transcript_violations(messages), vec![]);
}

#[test]
fn import_pairs_a_result_that_arrived_after_a_later_assistant_turn() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "late",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "run_commands",
                      "input": { "commands": ["pwd"] } },
                ],
                "ts": 2,
            },
            { "id": "u1", "role": "user", "content": [{ "type": "text", "text": "continue" }], "ts": 3 },
            { "id": "a1", "role": "assistant", "content": [{ "type": "text", "text": "still working" }], "ts": 4 },
            {
                "id": "u2",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "run_commands",
                      "content": [{ "query": "pwd", "result": "/work", "success": true }] },
                ],
                "ts": 5,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "late");

    // the call belongs to the first turn, so its output does too — it must not
    // decay into plain "[tool output]" text
    assert_eq!(tool_result_contents(&session), vec!["/work"]);

    let out = cline::session_messages(&session, "late");
    let messages = messages_of(&out);
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user", "user", "assistant"]);
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["name"], "run_commands");
    assert_eq!(
        messages[2]["content"][0]["content"],
        json!([{ "query": "pwd", "result": "/work", "success": true }])
    );
    assert_eq!(cline::transcript_violations(messages), vec![]);
}

#[test]
fn import_keeps_answer_parts_that_no_call_can_hold() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "excess",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "update the todos" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "todo_write",
                      "input": { "todos": [{ "content": "a" }, { "content": "b" }] } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "todo_write",
                      "content": [
                          { "query": "todo 1", "result": "updated 1", "success": true },
                          { "query": "todo 2", "result": "updated 2", "success": true },
                      ] },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "excess");

    // the tool has no sub-calls to split into, so its whole answer must survive
    assert_eq!(tool_result_contents(&session), vec!["updated 1\nupdated 2"]);
}

#[test]
fn session_messages_drops_an_assistant_turn_that_carries_nothing() {
    let session = make_session(vec![
        make_node(
            0,
            None,
            Role::User,
            "do the thing",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        make_node(
            1,
            Some(0),
            Role::Assistant,
            "",
            vec![],
            None,
            None,
            None,
            None,
            None,
            rendered(),
        ),
        make_node(
            2,
            Some(1),
            Role::User,
            "still there?",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
    ]);

    let out = cline::session_messages(&session, "imported-session");
    let messages = messages_of(&out);

    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "user"]);
    assert_eq!(cline::transcript_violations(messages), vec![]);
}

#[test]
fn transcript_violations_reports_what_the_cli_rejects() {
    let call = json!({
        "role": "assistant",
        "content": [{ "type": "tool_use", "id": "call_1", "name": "read_files", "input": {} }],
    });
    let text = json!({ "role": "user", "content": [{ "type": "text", "text": "continue" }] });
    let result = json!({
        "role": "user",
        "content": [{ "type": "tool_result", "tool_use_id": "call_1", "name": "read_files", "content": "body" }],
    });

    // a turn sandwiched between a call and its result is the reported failure …
    let violations = cline::transcript_violations(&[call.clone(), text.clone(), result.clone()]);
    let indices: Vec<cline::ViolationIndex> = violations.iter().map(|v| v.index.clone()).collect();
    assert_eq!(indices, [cline::ViolationIndex::Index(1)]);
    // … a result in front of it is fine, and a call left open at the end is not.
    assert_eq!(
        cline::transcript_violations(&[call.clone(), result, text]),
        vec![]
    );
    let violations = cline::transcript_violations(&[call]);
    let indices: Vec<cline::ViolationIndex> = violations.iter().map(|v| v.index.clone()).collect();
    assert_eq!(indices, [cline::ViolationIndex::Eof]);
}

#[test]
fn cline_session_id_follows_the_cli_shape() {
    let id = cline::cline_session_id(1_789_882_145_000.0);
    assert!(id.starts_with("1789882145000_"));
    let suffix = &id["1789882145000_".len()..];
    assert_eq!(suffix.len(), 5);
    assert!(
        suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    );
}

#[test]
fn session_row_carries_the_whole_index_shape() {
    let row = cline_index::session_row(
        &make_session(sample_nodes()),
        "1789882145000_sepia",
        "/home/luis/.cline/data/sessions/1789882145000_sepia/1789882145000_sepia.messages.json",
        "2026-09-20T05:29:05.000Z",
    );

    let keys: Vec<&str> = row.keys().map(String::as_str).collect();
    assert_eq!(keys, cline_index::SESSION_COLUMNS);
    assert_eq!(row["session_id"], "1789882145000_sepia");
    assert_eq!(row["status"], "completed");
    assert_eq!(row["pid"], 0);
    assert_eq!(row["is_subagent"], 0);
    assert_eq!(row["prompt"], "do the thing");
    assert_eq!(row["cwd"], "/work");
    assert_eq!(row["workspace_root"], "/work");
    assert_eq!(row["transcript_path"], "");
    assert_eq!(row["hook_path"], "");

    let metadata: Value = serde_json::from_str(row["metadata_json"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["title"], "Imported session");
    assert_eq!(
        metadata["importedFrom"],
        json!({ "store": "devin", "sessionId": "imported-session" })
    );
    // placeholders and columns cannot drift apart.
    assert_eq!(
        cline_index::insert_session_sql().matches('?').count(),
        cline_index::SESSION_COLUMNS.len()
    );
}

#[test]
fn session_row_records_subagent_lineage() {
    let mut session = make_session(vec![]);
    session.id = "1788501677312_sh9yh__teamtask__astdata__hXujax".into();
    session.parent_session_id = Some("1788501677312_sh9yh".into());
    session.agent_id = Some("astdata".into());
    let row =
        cline_index::session_row(&session, &session.id.clone(), "/tmp/m.messages.json", "now");

    assert_eq!(row["parent_session_id"], "1788501677312_sh9yh");
    assert_eq!(row["agent_id"], "astdata");
    assert_eq!(row["is_subagent"], 1);
}

#[test]
fn is_pid_alive_and_active_row_match_owner_state() {
    assert!(!cline_index::is_pid_alive(0));
    assert!(!cline_index::is_pid_alive(-1));
    assert!(!cline_index::is_pid_alive(4_000_000_000));
    assert!(cline_index::is_pid_alive(i64::from(std::process::id())));

    // a running session whose owner is gone is adoptable.
    assert!(!cline_index::is_active_row("running", false));
    // a dead run is replaceable even when the row keeps its stale pid.
    assert!(!cline_index::is_active_row("completed", true));
    // an in-flight session with a signaling pid must not be replaced.
    assert!(cline_index::is_active_row("running", true));
    assert!(cline_index::is_active_row("idle", true));
    assert!(cline_index::is_active_row("pending", true));
}

#[test]
fn import_keeps_token_metrics_and_the_per_message_model() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "metrics",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [{ "type": "text", "text": "done" }],
                "ts": 2,
                "modelInfo": { "id": "deepseek/deepseek-v4-flash", "provider": "cline-pass" },
                "metrics": {
                    "inputTokens": 5720,
                    "outputTokens": 279,
                    "cacheReadTokens": 100,
                    "cacheWriteTokens": 4,
                    "cost": 0.02,
                },
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "metrics");

    let assistant = session
        .nodes
        .iter()
        .find(|n| n.role == Role::Assistant && n.content == "done")
        .unwrap();
    assert_eq!(
        assistant.usage,
        Some(TokenUsage {
            input: 5720.0,
            output: 279.0,
            cache_read: Some(100.0),
            cache_write: Some(4.0),
            thinking: None,
            cost: Some(0.02),
        })
    );
    assert_eq!(
        assistant.model.as_deref(),
        Some("deepseek/deepseek-v4-flash")
    );
}

#[test]
fn import_leaves_usage_and_model_empty_for_malformed_fields() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "sparse",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            { "id": "a0", "role": "assistant", "content": [{ "type": "text", "text": "a" }], "ts": 2, "metrics": "nope" },
            {
                "id": "a1",
                "role": "assistant",
                "content": [{ "type": "text", "text": "b" }],
                "ts": 3,
                "metrics": { "cacheReadTokens": 9 },
                "modelInfo": { "id": "", "provider": "cline-pass" },
            },
            { "id": "a2", "role": "assistant", "content": [{ "type": "text", "text": "c" }], "ts": 4, "metrics": { "inputTokens": 1 } },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "sparse");

    let text = |s: &str| {
        session
            .nodes
            .iter()
            .find(|n| n.role == Role::Assistant && n.content == s)
            .unwrap()
    };
    assert!(text("a").usage.is_none());
    assert!(text("b").usage.is_none());
    assert!(text("b").model.is_none());
    assert_eq!(
        text("c").usage,
        Some(TokenUsage {
            input: 1.0,
            output: 0.0,
            cache_read: None,
            cache_write: None,
            thinking: None,
            cost: None,
        })
    );
}

#[test]
fn import_folds_a_failed_result_back_onto_call_and_node() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "failed",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            {
                "id": "a0",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "call_1", "name": "run_commands",
                      "input": { "command": "exit 2" } },
                ],
                "ts": 2,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "call_1", "name": "run_commands",
                      "content": [{ "query": "exit 2", "result": "", "success": false }] },
                ],
                "ts": 3,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "failed");

    let tool = session.nodes.iter().find(|n| n.role == Role::Tool).unwrap();
    assert_eq!(
        tool.tool_result,
        Some(ToolResultInfo {
            status: ToolCallStatus::Error,
            exit_code: None,
            duration_ms: None
        })
    );

    // both assistant twins carry the outcome on their ToolCall
    let assistants: Vec<&MessageNode> = session
        .nodes
        .iter()
        .filter(|n| n.role == Role::Assistant)
        .collect();
    assert!(!assistants.is_empty());
    for a in assistants {
        let statuses: Vec<Option<ToolCallStatus>> =
            a.tool_calls.iter().map(|tc| tc.status).collect();
        assert_eq!(statuses, [Some(ToolCallStatus::Error)]);
    }
}

#[test]
fn cline_subagent_info_reads_lineage_out_of_the_session_id() {
    assert_eq!(cline::cline_subagent_info("1788501677312_sh9yh"), None);
    assert_eq!(
        cline::cline_subagent_info("1788501677312_sh9yh__teamtask__astdata__hXujax"),
        Some(cline::SubagentInfo {
            parent_session_id: "1788501677312_sh9yh".into(),
            agent_id: "astdata".into(),
        })
    );
    assert_eq!(
        cline::cline_subagent_info("1788512223880_qo9bf__agent_1788512292452_l58kf4"),
        Some(cline::SubagentInfo {
            parent_session_id: "1788512223880_qo9bf".into(),
            agent_id: "agent_1788512292452_l58kf4".into(),
        })
    );
    // nested team tasks split at the last marker — the parent is one level up
    assert_eq!(
        cline::cline_subagent_info(
            "1789101927554_p6vq1__teamtask__subagent-env-removal__RBdWwq__teamtask__cranelift-env-removal__pUEZ9k"
        ),
        Some(cline::SubagentInfo {
            parent_session_id: "1789101927554_p6vq1__teamtask__subagent-env-removal__RBdWwq".into(),
            agent_id: "cranelift-env-removal".into(),
        })
    );
    // a truncated marker yields no agent to name
    assert_eq!(cline::cline_subagent_info("x__teamtask__"), None);
}

#[test]
fn import_marks_a_subagent_sessions_lineage_from_its_id() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "1788501677312_sh9yh__teamtask__astdata__hXujax",
        &json!([{ "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 }]),
        &json!({}),
        None,
    );
    let session = load(
        root.path(),
        "1788501677312_sh9yh__teamtask__astdata__hXujax",
    );

    assert_eq!(
        session.parent_session_id.as_deref(),
        Some("1788501677312_sh9yh")
    );
    assert_eq!(session.agent_id.as_deref(), Some("astdata"));
}

#[test]
fn export_writes_usage_per_message_model_and_result_success_back() {
    let mut assistant = make_node(
        1,
        Some(0),
        Role::Assistant,
        "on it",
        vec![tool_call("call_1", "read", json!({ "file_path": "/a" }))],
        None,
        None,
        None,
        None,
        None,
        rendered(),
    );
    assistant.usage = Some(TokenUsage {
        input: 10.0,
        output: 5.0,
        cache_read: Some(7.0),
        cache_write: None,
        thinking: None,
        cost: None,
    });
    assistant.model = Some("deepseek/deepseek-v4-flash".into());
    let session = make_session(vec![
        make_node(
            0,
            None,
            Role::User,
            "do the thing",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        assistant,
        make_node(
            2,
            Some(1),
            Role::Tool,
            "denied",
            vec![],
            Some("call_1"),
            Some("read"),
            Some(ToolResultInfo {
                status: ToolCallStatus::Error,
                exit_code: None,
                duration_ms: None,
            }),
            None,
            None,
            json!({ "toolArguments": { "file_path": "/a" } }),
        ),
    ]);

    let out = cline::session_messages(&session, "imported-session");
    let messages = messages_of(&out);

    assert_eq!(
        messages[1]["modelInfo"],
        json!({ "id": "deepseek/deepseek-v4-flash", "provider": "cline-pass" })
    );
    assert_eq!(
        messages[1]["metrics"],
        json!({
            "inputTokens": 10.0,
            "outputTokens": 5.0,
            "cacheReadTokens": 7.0,
            "cacheWriteTokens": 0.0,
            "cost": 0.0,
        })
    );
    assert_eq!(
        messages[2]["content"][0]["content"],
        json!([{ "query": "/a", "result": "denied", "success": false }])
    );
}

#[test]
fn import_lifts_image_and_document_blocks_off_user_messages() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "attachments",
        &json!([
            {
                "id": "u0",
                "role": "user",
                "content": [
                    { "type": "text", "text": "<user_input mode=\"act\">look at these</user_input>" },
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGk=" } },
                    { "type": "document", "title": "spec.md",
                      "source": { "type": "url", "url": "file:///work/spec.md", "media_type": "text/markdown" } },
                ],
                "ts": 1,
            },
            {
                "id": "u1",
                "role": "user",
                "content": [
                    { "type": "text", "text": "and this" },
                    { "type": "image", "source": { "type": "url", "url": "https://x/y.png" } },
                    // Entries that carry nothing renderable are skipped, not fatal.
                    { "type": "image", "source": { "type": "base64" } },
                    { "type": "document", "source": null },
                    { "type": "mystery", "payload": 1 },
                    "junk",
                ],
                "ts": 2,
            },
            {
                // Flat field spellings — a block may carry data/url/media_type on
                // itself rather than under `source`.
                "id": "u2",
                "role": "user",
                "content": [
                    { "type": "text", "text": "flat fields" },
                    { "type": "image", "data": "AAE=", "media_type": "image/jpeg" },
                    { "type": "image", "data": "AAF=", "mimeType": "image/gif" },
                    { "type": "image", "url": "https://flat/i.png" },
                    { "type": "document", "url": "file:///b.md", "text": "doc body" },
                    { "type": "document", "source": { "type": "base64", "data": "AAI=" } },
                    { "type": "document", "source": { "type": "text", "text": "src body" } },
                    { "type": "document", "data": "AAQ=" },
                ],
                "ts": 3,
            },
            {
                // A tool result rides a user-role message in this format — its content
                // stays a tool concern and never becomes a block.
                "id": "u3",
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": "orphan", "name": "exec", "content": "x" }],
                "ts": 4,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "attachments");

    let visible = cline::visible_nodes(&session);
    let users: Vec<&MessageNode> = visible.iter().filter(|n| n.role == Role::User).collect();

    // The first user message seeds the tree — its attachments land there.
    assert_eq!(
        users[0].blocks,
        vec![
            Block::Text {
                text: "<user_input mode=\"act\">look at these</user_input>".into()
            },
            Block::Image {
                data: Some("aGk=".into()),
                mime_type: Some("image/png".into()),
                uri: None,
            },
            Block::File {
                uri: Some("file:///work/spec.md".into()),
                name: Some("spec.md".into()),
                mime_type: Some("text/markdown".into()),
                size: None,
                text: None,
                data: None,
            },
        ]
    );

    // Later user messages keep their own blocks.
    assert_eq!(
        users[1].blocks,
        vec![
            Block::Text {
                text: "and this".into()
            },
            Block::Image {
                data: None,
                mime_type: None,
                uri: Some("https://x/y.png".into())
            },
        ]
    );

    // Flat spellings map the same way; a document's own text becomes the
    // file block's embedded text.
    assert_eq!(
        users[2].blocks,
        vec![
            Block::Text {
                text: "flat fields".into()
            },
            Block::Image {
                data: Some("AAE=".into()),
                mime_type: Some("image/jpeg".into()),
                uri: None,
            },
            Block::Image {
                data: Some("AAF=".into()),
                mime_type: Some("image/gif".into()),
                uri: None,
            },
            Block::Image {
                data: None,
                mime_type: None,
                uri: Some("https://flat/i.png".into())
            },
            Block::File {
                uri: Some("file:///b.md".into()),
                name: None,
                mime_type: None,
                size: None,
                text: Some("doc body".into()),
                data: None,
            },
            Block::File {
                uri: None,
                name: None,
                mime_type: None,
                size: None,
                text: None,
                data: Some("AAI=".into()),
            },
            Block::File {
                uri: None,
                name: None,
                mime_type: None,
                size: None,
                text: Some("src body".into()),
                data: None,
            },
            Block::File {
                uri: None,
                name: None,
                mime_type: None,
                size: None,
                text: None,
                data: Some("AAQ=".into()),
            },
        ]
    );

    // The orphaned tool result decays to "[tool output]" text with no blocks.
    assert!(users[3].content.contains("[tool output]"));
    assert_eq!(users[3].blocks, vec![]);
}

#[test]
fn import_survives_a_log_whose_first_turn_isnt_text_bearing() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "nouser",
        &json!([
            {
                "id": "a0",
                "role": "assistant",
                "content": [{ "type": "text", "text": "booted mid-flight" }],
                "ts": 1,
            },
            {
                "id": "u0",
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "url", "url": "https://x/i.png" } },
                    { "type": "text" },
                    { "type": "text", "text": "" },
                    { "type": "text", "text": "now with text" },
                ],
                "ts": 2,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "nouser");

    // The first text-bearing user message seeds the tree — non-text items
    // ahead of its text, an empty text block, and the image all land on the
    // seed node's blocks; the message is not emitted twice.
    let visible = cline::visible_nodes(&session);
    let users: Vec<&MessageNode> = visible.iter().filter(|n| n.role == Role::User).collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].content, "now with text");
    assert_eq!(
        users[0].blocks,
        vec![
            Block::Image {
                data: None,
                mime_type: None,
                uri: Some("https://x/i.png".into())
            },
            Block::Text { text: "".into() },
            Block::Text {
                text: "now with text".into()
            },
        ]
    );
}

#[test]
fn import_tolerates_empty_missing_and_mixed_user_content() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "mixed",
        &json!([
            // No content field at all.
            { "id": "u_pre", "role": "user", "ts": 0 },
            // A text entry with no text, mixed with a tool result: the text half is
            // dropped (a tool-result message is not a prompt) and the orphan decays.
            {
                "id": "u0",
                "role": "user",
                "content": [
                    { "type": "text" },
                    { "type": "tool_result", "tool_use_id": "or", "name": "exec", "content": "y" },
                    // A result entry may even lack the call id entirely.
                    { "type": "tool_result", "name": "exec", "content": "z" },
                ],
                "ts": 1,
            },
            { "id": "u1", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 2 },
            // A prompt turn whose only text entry carries no text emits nothing.
            { "id": "u2", "role": "user", "content": [{ "type": "text" }], "ts": 3 },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "mixed");

    // The "go" turn seeds the tree; the orphaned results decay to tool-output
    // text; the empty/missing contents produce nothing.
    let users: Vec<&MessageNode> = session
        .nodes
        .iter()
        .filter(|n| n.role == Role::User)
        .collect();
    let contents: Vec<&str> = users.iter().map(|n| n.content.as_str()).collect();
    assert_eq!(contents, ["go", "[tool output]\ny", "[tool output]\nz"]);
    assert!(users.iter().all(|n| n.blocks.is_empty()));
}

#[test]
fn import_builds_a_session_from_a_log_with_no_user_text_at_all() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "assistantonly",
        &json!([
            {
                "id": "a0",
                "role": "assistant",
                "content": [{ "type": "text", "text": "spoke unprompted" }],
                "ts": 1,
            },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "assistantonly");

    // The seed user node stays empty — nothing to lift text or blocks from.
    let users: Vec<&MessageNode> = session
        .nodes
        .iter()
        .filter(|n| n.role == Role::User)
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].content, "");
    assert_eq!(users[0].blocks, vec![]);
}

#[test]
fn import_keeps_a_text_only_content_array_off_node_blocks() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "textonly",
        &json!([
            { "id": "u0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1 },
            { "id": "u1", "role": "user", "content": [{ "type": "text", "text": "more" }], "ts": 2 },
            { "id": "u2", "role": "user", "content": "bare string", "ts": 3 },
        ]),
        &json!({}),
        None,
    );
    let session = load(root.path(), "textonly");

    for node in &session.nodes {
        assert_eq!(node.blocks, vec![]);
    }
}

#[test]
fn export_writes_user_blocks_back_as_provider_content_entries() {
    let mut user = make_node(
        1,
        None,
        Role::User,
        "with attachments",
        vec![],
        None,
        None,
        None,
        None,
        None,
        Value::Null,
    );
    user.blocks = vec![
        Block::Text {
            text: "with attachments".into(),
        },
        Block::Image {
            data: Some("aGk=".into()),
            mime_type: Some("image/png".into()),
            uri: None,
        },
        Block::Image {
            data: Some("aGk=".into()),
            mime_type: None,
            uri: None,
        },
        Block::Image {
            data: None,
            mime_type: None,
            uri: Some("https://x/y.png".into()),
        },
        Block::Image {
            data: None,
            mime_type: None,
            uri: None,
        },
        Block::Audio {
            data: Some("AAE=".into()),
            mime_type: Some("audio/wav".into()),
        },
        Block::Audio {
            data: None,
            mime_type: None,
        },
        Block::File {
            uri: Some("file:///work/spec.md".into()),
            name: Some("spec.md".into()),
            mime_type: None,
            size: None,
            text: None,
            data: None,
        },
        Block::File {
            uri: Some("file:///a.ts".into()),
            name: None,
            mime_type: None,
            size: None,
            text: Some("const a=1".into()),
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
    ];
    let session = make_session(vec![
        make_node(
            0,
            None,
            Role::System,
            "sys",
            vec![],
            None,
            None,
            None,
            None,
            None,
            Value::Null,
        ),
        user,
    ]);

    let out = cline::session_messages(&session, "imported-session");
    let messages = messages_of(&out);

    // Text stays a single block from `content`; each attachment maps to the
    // provider form (embedded image) or degrades to a mention.
    assert_eq!(
        messages[0]["content"],
        json!([
            { "type": "text", "text": "with attachments" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGk=" } },
            { "type": "image", "source": { "type": "base64", "data": "aGk=" } },
            { "type": "image", "source": { "type": "url", "url": "https://x/y.png" } },
            { "type": "text", "text": "[image]" },
            { "type": "text", "text": "[audio: audio/wav]" },
            { "type": "text", "text": "[audio: attachment]" },
            { "type": "text", "text": "[file: spec.md]" },
            { "type": "text", "text": "[file: file:///a.ts]\nconst a=1" },
            { "type": "text", "text": "[file: attachment]" },
        ])
    );
}

/* ---- mapToolUse raw fallbacks (ClineExtra) ---------------------------- */

#[test]
fn import_keeps_tool_calls_whose_list_fields_are_empty_or_absent() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "s1",
        &json!([
            { "id": "m0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1000 },
            {
                "id": "m1",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "t1", "name": "read_files", "input": {} },
                    { "type": "tool_use", "id": "t2", "name": "search_codebase", "input": { "queries": [] } },
                    { "type": "tool_use", "id": "t3", "name": "fetch_web_content", "input": {} },
                    { "type": "tool_use", "id": "t4", "name": "editor", "input": { "path": "" } },
                    { "type": "tool_use", "id": "t5", "name": "run_commands" },
                ],
                "ts": 2000,
            },
        ]),
        &json!({ "model": "glm-5-2" }),
        None,
    );
    let session = load(root.path(), "s1");
    let assistant = session
        .nodes
        .iter()
        .find(|n| n.role == Role::Assistant && !n.tool_calls.is_empty())
        .unwrap();
    let names: Vec<&str> = assistant
        .tool_calls
        .iter()
        .map(|tc| tc.name.as_str())
        .collect();
    // none of these split into calls — each survives verbatim, name and all
    assert_eq!(
        names,
        [
            "read_files",
            "search_codebase",
            "fetch_web_content",
            "editor",
            "run_commands"
        ]
    );
    assert_eq!(assistant.tool_calls[4].arguments, json!({}));
}

#[test]
fn import_maps_fetch_requests_and_nested_query_shapes() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "s1",
        &json!([
            { "id": "m0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1000 },
            {
                "id": "m1",
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use", "id": "t1", "name": "fetch_web_content",
                        "input": { "requests": ["https://a.io", { "url": "https://b.io" }] },
                    },
                    // a search whose queries arrive as a nested array
                    {
                        "type": "tool_use", "id": "t2", "name": "search_codebase",
                        "input": { "queries": [["inner", { "pattern": "obj" }]] },
                    },
                    // a read whose file entry carries no path-like key stringifies whole
                    { "type": "tool_use", "id": "t3", "name": "read_files", "input": { "files": [{ "odd": 1 }] } },
                    // a null entry resolves to "" — a call without a location
                    { "type": "tool_use", "id": "t4", "name": "read_files", "input": { "files": [null] } },
                    // a bare number stringifies
                    { "type": "tool_use", "id": "t5", "name": "read_files", "input": { "files": [42] } },
                ],
                "ts": 2000,
            },
        ]),
        &json!({ "model": "glm-5-2" }),
        None,
    );
    let session = load(root.path(), "s1");
    let assistant = session
        .nodes
        .iter()
        .find(|n| n.role == Role::Assistant && !n.tool_calls.is_empty())
        .unwrap();
    let calls = &assistant.tool_calls;
    let (fetch1, fetch2, grep1, grep2, read1, read2, read3) = (
        &calls[0], &calls[1], &calls[2], &calls[3], &calls[4], &calls[5], &calls[6],
    );
    assert_eq!(fetch1.name, "webfetch");
    assert_eq!(fetch1.arguments, json!({ "url": "https://a.io" }));
    assert_eq!(fetch2.arguments, json!({ "url": "https://b.io" }));
    assert_eq!(grep1.name, "grep");
    assert_eq!(grep1.arguments, json!({ "pattern": "inner" }));
    assert_eq!(grep2.arguments, json!({ "pattern": "obj" }));
    assert_eq!(read1.arguments, json!({ "file_path": "{\"odd\":1}" }));
    // no readable path → the call stays but records no location
    assert_eq!(read2.name, "read");
    assert_eq!(read2.arguments, json!({ "file_path": "" }));
    assert_eq!(read2.locations, vec![]);
    assert_eq!(read3.arguments, json!({ "file_path": "42" }));
}

#[test]
fn import_folds_editor_inputs_into_write_and_edit_calls_with_diffs() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "s1",
        &json!([
            { "id": "m0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1000 },
            {
                "id": "m1",
                "role": "assistant",
                "content": [
                    // create: old_text "null" is the CLI's marker for a new file
                    { "type": "tool_use", "id": "t1", "name": "editor",
                      "input": { "path": "/work/a.ts", "old_text": "null", "new_text": "body" } },
                    // create with a non-string payload keeps the path marker only
                    { "type": "tool_use", "id": "t2", "name": "editor",
                      "input": { "path": "/work/b.ts", "old_text": null, "new_text": { "x": 1 } } },
                    // edit with non-string hunks serializes them
                    { "type": "tool_use", "id": "t3", "name": "editor",
                      "input": { "path": "/work/c.ts", "old_text": { "o": 1 }, "new_text": 7 } },
                ],
                "ts": 2000,
            },
        ]),
        &json!({ "model": "glm-5-2" }),
        None,
    );
    let session = load(root.path(), "s1");
    let assistant = session
        .nodes
        .iter()
        .find(|n| n.role == Role::Assistant && !n.tool_calls.is_empty())
        .unwrap();
    let (create, odd_create, edit) = (
        &assistant.tool_calls[0],
        &assistant.tool_calls[1],
        &assistant.tool_calls[2],
    );
    assert_eq!(create.name, "write");
    assert_eq!(
        create.arguments,
        json!({ "file_path": "/work/a.ts", "content": "body" })
    );
    assert_eq!(
        create.diffs,
        vec![ToolCallDiff {
            path: "/work/a.ts".into(),
            old_text: None,
            new_text: Some("body".into()),
        }]
    );
    assert_eq!(odd_create.name, "write");
    assert_eq!(
        odd_create.diffs,
        vec![ToolCallDiff {
            path: "/work/b.ts".into(),
            old_text: None,
            new_text: None
        }]
    );
    assert_eq!(edit.name, "edit");
    assert_eq!(
        edit.diffs,
        vec![ToolCallDiff {
            path: "/work/c.ts".into(),
            old_text: Some("{\"o\":1}".into()),
            new_text: None,
        }]
    );
}

/* ---- toolResultShares ------------------------------------------------- */

#[test]
fn import_distributes_a_non_text_non_list_result_body_across_calls() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "s1",
        &json!([
            { "id": "m0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1000 },
            {
                "id": "m1",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "t1", "name": "run_commands",
                      "input": { "commands": ["ls"] } },
                ],
                "ts": 2000,
            },
            // the result body is a bare number — every call's share serializes it
            {
                "id": "m2",
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": 42 }],
                "ts": 3000,
            },
            {
                "id": "m3",
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "id": "t9", "name": "run_commands",
                      "input": { "commands": ["pwd"] } },
                ],
                "ts": 4000,
            },
            {
                "id": "m4",
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": "t9" }],
                "ts": 5000,
            },
        ]),
        &json!({ "model": "glm-5-2" }),
        None,
    );
    let session = load(root.path(), "s1");
    let tools: Vec<&MessageNode> = session
        .nodes
        .iter()
        .filter(|n| n.role == Role::Tool)
        .collect();
    assert_eq!(tools[0].content, "42");
    // no content at all → empty output, still a success
    assert_eq!(tools[1].content, "");
    assert_eq!(
        tools[1].tool_result.as_ref().unwrap().status,
        ToolCallStatus::Success
    );
}

#[test]
fn import_keeps_answer_parts_matched_by_url_or_arriving_as_bare_strings() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "s1",
        &json!([
            { "id": "m0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": 1000 },
            {
                "id": "m1",
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use", "id": "t1", "name": "fetch_web_content",
                        "input": { "requests": ["https://a.io", "https://b.io"] },
                    },
                ],
                "ts": 2000,
            },
            {
                "id": "m2",
                "role": "user",
                "content": [
                    {
                        "type": "tool_result", "tool_use_id": "t1",
                        "content": [
                            // keyed by url rather than query
                            { "url": "https://b.io", "result": "page b", "success": false },
                            // a bare string entry claims nothing — it lands in the tail
                            "loose text",
                            // an object with neither query/url/result stringifies whole
                            { "meta": true },
                            // an array query — keyOf joins it
                            { "query": ["q1", "q2"], "result": "joined" },
                        ],
                    },
                ],
                "ts": 3000,
            },
        ]),
        &json!({ "model": "glm-5-2" }),
        None,
    );
    let session = load(root.path(), "s1");
    let tools: Vec<&MessageNode> = session
        .nodes
        .iter()
        .filter(|n| n.role == Role::Tool)
        .collect();
    assert_eq!(tools.len(), 2);
    // the bare string entry fills the call no key matched (a.io)
    assert_eq!(tools[0].content, "loose text");
    // https://b.io matched the second call by url key; the leftovers
    // (keyless object, unmatched array query) merge onto the last call's
    // share, and the recorded failure marks it.
    let last = tools[1];
    assert!(last.content.contains("page b"));
    assert!(last.content.contains("{\"meta\":true}"));
    assert!(last.content.contains("joined"));
    assert_eq!(
        last.tool_result.as_ref().unwrap().status,
        ToolCallStatus::Error
    );
}

/* ---- manifest/messages degenerate inputs ------------------------------ */

#[test]
fn from_directory_reports_missing_dirs_manifests_and_bad_builds() {
    let root = tempfile::tempdir().unwrap();

    let missing = cline::from_directory(&root.path().join("ghost"), None).unwrap_err();
    assert!(missing.message.contains("not found"));

    // a dir with only a .messages.json — no manifest
    let bare = root.path().join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::write(bare.join("bare.messages.json"), "{}").unwrap();
    let no_manifest = cline::from_directory(&bare, None).unwrap_err();
    assert!(no_manifest.message.contains("metadata json"));

    // buildSession blows up on a non-string model
    write_cline_dir(
        root.path(),
        "badbuild",
        &json!([]),
        &json!({ "model": 5 }),
        None,
    );
    let bad_build = cline::from_directory(&root.path().join("badbuild"), None).unwrap_err();
    assert!(bad_build.message.contains("Failed to build session"));

    // manifest parses but the transcript is missing → raw fs error wraps
    let dir = root.path().join("nomsg");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("nomsg.json"),
        json!({ "session_id": "nomsg" }).to_string(),
    )
    .unwrap();
    let no_msg = cline::from_directory(&dir, None).unwrap_err();
    assert!(no_msg.message.contains("Cline conversion failed"));

    // the messages file is valid JSON but not an object → no messages
    write_cline_dir(root.path(), "num", &json!([]), &json!({}), Some("5"));
    let num = load(root.path(), "num");
    // two system nodes + an empty user node is all a message-less log yields
    assert!(num.nodes.iter().all(|n| n.role != Role::Assistant));
}

#[test]
fn from_directory_tolerates_missing_meta_fields_and_non_numeric_timestamps() {
    let root = tempfile::tempdir().unwrap();
    // no session_id → dir name; no cwd → dir; no title → prompt → fallback
    let dir = write_cline_dir(
        root.path(),
        "sparse",
        &json!([
            // non-numeric ts falls back to the session start
            { "id": "m0", "role": "user", "content": [{ "type": "text", "text": "go" }], "ts": "not-a-ts" },
            { "id": "m1", "role": "assistant", "content": [{ "type": "text", "text": "ok" }] },
        ]),
        &json!({}),
        None,
    );
    // manifest without session_id: rewrite it bare
    std::fs::write(
        dir.join("sparse.json"),
        json!({ "version": 1, "status": "done" }).to_string(),
    )
    .unwrap();
    let session = load(root.path(), "sparse");
    assert_eq!(session.id, "sparse");
    assert_eq!(session.title, "Imported session");
    assert_eq!(session.working_directory, dir.to_string_lossy());
    assert_eq!(session.created_at, session.nodes[0].created_at);
    assert_eq!(
        session.prompt_history[0].timestamp,
        session.created_at * 1000.0
    );
}

#[test]
fn checkpoints_from_manifest_drops_malformed_entries_and_keeps_a_lone_latest() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(
        root.path(),
        "s1",
        &json!([]),
        &json!({
            "metadata": {
                "checkpoint": {
                    "history": [
                        { "ref": "aaa", "createdAt": 1000, "runCount": 1, "kind": "stash" },
                        { "ref": "bbb", "createdAt": "not a number" },
                        { "ref": null, "createdAt": 5 },
                        "junk",
                        { "ref": "ccc", "createdAt": 3000, "runCount": "x" },
                    ],
                    "latest": { "ref": "newest", "createdAt": 4000 },
                },
            },
        }),
        None,
    );
    let session = load(root.path(), "s1");
    assert_eq!(
        session.checkpoints,
        vec![
            CheckpointRef {
                r#ref: "aaa".into(),
                created_at: 1000.0,
                run_count: Some(1),
                kind: Some("stash".into()),
            },
            CheckpointRef {
                r#ref: "ccc".into(),
                created_at: 3000.0,
                run_count: None,
                kind: None
            },
            CheckpointRef {
                r#ref: "newest".into(),
                created_at: 4000.0,
                run_count: None,
                kind: None
            },
        ]
    );

    // latest already in history → not duplicated; non-list history → []
    write_cline_dir(
        root.path(),
        "s2",
        &json!([]),
        &json!({
            "metadata": {
                "checkpoint": { "history": "none", "latest": { "ref": "l", "createdAt": 1 } },
            },
        }),
        None,
    );
    assert_eq!(
        load(root.path(), "s2").checkpoints,
        vec![CheckpointRef {
            r#ref: "l".into(),
            created_at: 1.0,
            run_count: None,
            kind: None
        }]
    );

    write_cline_dir(
        root.path(),
        "s3",
        &json!([]),
        &json!({
            "metadata": {
                "checkpoint": {
                    "history": [{ "ref": "only", "createdAt": 1 }],
                    "latest": { "ref": "only", "createdAt": 1 },
                },
            },
        }),
        None,
    );
    assert_eq!(
        load(root.path(), "s3").checkpoints,
        vec![CheckpointRef {
            r#ref: "only".into(),
            created_at: 1.0,
            run_count: None,
            kind: None
        }]
    );
}

/* ---- sessionMessages writer paths (ClineExtra) ------------------------ */

fn w_node(
    node_id: i64,
    role: Role,
    content: &str,
    tool_calls: Vec<ToolCall>,
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
    thinking_signature: Option<&str>,
    metadata: Value,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id: if node_id == 0 {
            None
        } else {
            Some(node_id - 1)
        },
        role,
        content: content.into(),
        blocks: Vec::new(),
        tool_calls,
        tool_call_id: tool_call_id.map(str::to_string),
        tool_name: tool_name.map(str::to_string),
        thinking: None,
        thinking_signature: thinking_signature.map(str::to_string),
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: 1_700_000_000.0 + node_id as f64,
        metadata,
    }
}

fn w_session(nodes: Vec<MessageNode>) -> Session {
    Session {
        id: "s".into(),
        title: "t".into(),
        working_directory: "/work".into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: "m".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_010.0,
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

#[test]
fn session_messages_maps_webfetch_edit_write_calls_back_to_cline_inputs() {
    let assistant = w_node(
        1,
        Role::Assistant,
        "working",
        vec![
            tool_call("w1", "webfetch", json!({ "url": "https://a.io" })),
            tool_call(
                "e1",
                "edit",
                json!({ "file_path": "/a.ts", "old_string": "o", "new_string": "n" }),
            ),
            tool_call(
                "wr1",
                "write",
                json!({ "file_path": "/b.ts", "content": "c" }),
            ),
        ],
        None,
        None,
        None,
        rendered(),
    );
    let out = cline::session_messages(
        &w_session(vec![
            w_node(0, Role::User, "go", vec![], None, None, None, Value::Null),
            assistant,
        ]),
        "s",
    );
    let messages = messages_of(&out);
    let assistant_msg = messages.iter().find(|m| m["role"] == "assistant").unwrap();
    let uses: Vec<&Value> = assistant_msg["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "tool_use")
        .collect();
    assert_eq!(
        uses,
        [
            &json!({
                "type": "tool_use",
                "id": "w1",
                "name": "fetch_web_content",
                "input": { "requests": [{ "url": "https://a.io" }] },
            }),
            &json!({
                "type": "tool_use",
                "id": "e1",
                "name": "editor",
                "input": { "path": "/a.ts", "old_text": "o", "new_text": "n" },
            }),
            &json!({
                "type": "tool_use",
                "id": "wr1",
                "name": "editor",
                "input": { "path": "/b.ts", "old_text": null, "new_text": "c" },
            }),
        ]
    );
    // the calls have no recorded results — placeholders keep the log valid
    assert_eq!(cline::transcript_violations(messages), vec![]);
}

#[test]
fn session_messages_writes_orphaned_results_and_signature_only_thinking() {
    let out = cline::session_messages(
        &w_session(vec![
            w_node(0, Role::User, "go", vec![], None, None, None, Value::Null),
            // a tool node no assistant turn claims — lands as its own user entry
            w_node(
                1,
                Role::Tool,
                "late output",
                vec![],
                None,
                None,
                None,
                Value::Null,
            ),
            // an orphan naming a tool still carries it
            w_node(
                2,
                Role::Tool,
                "grep out",
                vec![],
                Some("gone"),
                Some("grep"),
                None,
                json!({ "toolArguments": { "pattern": "x" } }),
            ),
            w_node(
                3,
                Role::Assistant,
                "",
                vec![],
                None,
                None,
                // a seal without text is a fully redacted block
                Some("opaque-blob"),
                Value::Null,
            ),
        ]),
        "s",
    );
    let messages = messages_of(&out);
    let orphan = &messages[1];
    assert_eq!(orphan["role"], "user");
    assert_eq!(
        orphan["content"][0],
        json!({
            "type": "tool_result",
            "tool_use_id": "",
            "name": "unknown",
            "content": "late output",
        })
    );
    let named = &messages[2];
    // a named call whose tool maps back gets the keyed result envelope
    assert_eq!(
        named["content"][0],
        json!({
            "type": "tool_result",
            "tool_use_id": "gone",
            "name": "search_codebase",
            "content": [{ "query": "x", "result": "grep out", "success": true }],
        })
    );
    let assistant = messages.iter().find(|m| m["role"] == "assistant").unwrap();
    assert_eq!(
        assistant["content"],
        json!([{ "type": "redacted_thinking", "data": "opaque-blob" }])
    );
}

#[test]
fn session_messages_keeps_results_for_unmapped_tool_names_as_plain_content() {
    let mut tool = w_node(
        2,
        Role::Tool,
        "done",
        vec![],
        Some("b1"),
        None,
        None,
        Value::Null,
    );
    tool.tool_name = None;
    let out = cline::session_messages(
        &w_session(vec![
            w_node(0, Role::User, "go", vec![], None, None, None, Value::Null),
            w_node(
                1,
                Role::Assistant,
                "ran it",
                vec![tool_call("b1", "Bash", json!({ "command": "ls" }))],
                None,
                None,
                None,
                Value::Null,
            ),
            tool,
        ]),
        "s",
    );
    let messages = messages_of(&out);
    let results = messages
        .iter()
        .find(|m| m["role"] == "user" && m["id"] != "msg_0")
        .unwrap();
    // toolName absent → the call's own name stands in; "Bash" is not a keyed
    // result tool so the body is the raw content
    assert_eq!(
        results["content"][0],
        json!({
            "type": "tool_result",
            "tool_use_id": "b1",
            "name": "Bash",
            "content": "done",
        })
    );
}

#[test]
fn to_directory_reports_kept_planned_replaced_and_created() {
    let session = w_session(vec![w_node(
        0,
        Role::User,
        "hi",
        vec![],
        None,
        None,
        None,
        Value::Null,
    )]);

    let kept_dir = tempfile::tempdir().unwrap();
    std::fs::write(kept_dir.path().join("s.json"), "{}").unwrap();
    let kept = cline::to_directory(&session, kept_dir.path(), false, false).unwrap();
    assert_eq!(kept, [cline::ExportAction::Kept, cline::ExportAction::Kept]);

    let dry_dir = tempfile::tempdir().unwrap();
    let planned = cline::to_directory(&session, &dry_dir.path().join("dry"), false, true).unwrap();
    assert_eq!(
        planned,
        [cline::ExportAction::Planned, cline::ExportAction::Planned]
    );
    assert_eq!(std::fs::read_dir(dry_dir.path()).unwrap().count(), 0);

    // replaced: force over an existing manifest
    let replaced = cline::to_directory(&session, kept_dir.path(), true, false).unwrap();
    assert_eq!(
        replaced,
        [cline::ExportAction::Replaced, cline::ExportAction::Replaced]
    );

    // write failure surfaces as a ConversionError
    let fail_dir = tempfile::tempdir().unwrap();
    let file_out = fail_dir.path().join("not-a-dir");
    std::fs::write(&file_out, "blocking file").unwrap();
    let err = cline::to_directory(&session, &file_out.join("sub"), false, false).unwrap_err();
    assert!(err.message.contains("Cline export failed"));
}

#[test]
fn transcript_violations_flags_unresolved_calls_and_stray_messages() {
    let violations = cline::transcript_violations(&[
        json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "c1" }] }),
        // a text user message while c1 is pending — the CLI rejects this
        json!({ "role": "user", "content": [{ "type": "text", "text": "interrupt" }] }),
        // a result-only user message resolves c1, leaves c2 open
        json!({
            "role": "user",
            "content": [
                { "type": "tool_result", "tool_use_id": "c1", "content": "done" },
                { "type": "tool_result", "tool_use_id": "c1b", "content": "x" },
            ],
        }),
        json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "c2" }] }),
        // non-message junk contributes nothing
        json!("junk"),
        json!({ "role": "assistant", "content": "not-a-list" }),
    ]);
    assert_eq!(
        violations,
        vec![
            cline::TranscriptViolation {
                index: cline::ViolationIndex::Index(1),
                tool_call_ids: vec!["c1".into()],
            },
            // a non-message line is not a result-only message — it trips the
            // still-pending c2 as well
            cline::TranscriptViolation {
                index: cline::ViolationIndex::Index(4),
                tool_call_ids: vec!["c2".into()],
            },
            cline::TranscriptViolation {
                index: cline::ViolationIndex::Eof,
                tool_call_ids: vec!["c2".into()],
            },
        ]
    );
}

#[test]
fn import_reads_cline_pass_prefixed_model_and_strips_it() {
    let root = tempfile::tempdir().unwrap();
    write_cline_dir(root.path(), "s1", &json!([]), &json!({}), None);
    let session = load(root.path(), "s1");
    assert_eq!(session.model, "swe-2-high");
    assert_eq!(session.backend_type, "windsurf");
}
