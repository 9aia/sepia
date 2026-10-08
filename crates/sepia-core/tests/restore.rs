#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_core::domain::{MessageNode, Role, Session, ToolCall, ToolCallDiff};
use sepia_core::restore::{
    FileRestorePlan, diffs_for_path, file_history_snapshot, plan_path_restore,
    resolve_workspace_path,
};
use serde_json::{Value, json};
use std::path::PathBuf;

const CWD: &str = "/repo/workspace";

fn session_with(calls: Vec<(&str, Vec<ToolCallDiff>)>, cwd: &str) -> Session {
    Session {
        id: "s1".into(),
        title: "s1".into(),
        working_directory: cwd.into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: "m".into(),
        created_at: 0.0,
        last_activity_at: 0.0,
        main_chain_id: 0,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: vec![],
        metadata: Value::Null,
        nodes: calls
            .into_iter()
            .enumerate()
            .map(|(index, (id, diffs))| MessageNode {
                node_id: index as i64,
                parent_node_id: None,
                role: Role::Assistant,
                content: String::new(),
                blocks: vec![],
                tool_calls: vec![ToolCall {
                    id: id.into(),
                    name: "edit".into(),
                    arguments: json!({}),
                    index: 0,
                    kind: "function".into(),
                    status: None,
                    exit_code: None,
                    duration_ms: None,
                    locations: vec![],
                    diffs,
                }],
                tool_call_id: None,
                tool_name: None,
                thinking: None,
                thinking_signature: None,
                usage: None,
                model: None,
                request_id: None,
                finish_reason: None,
                tool_result: None,
                created_at: index as f64,
                metadata: Value::Null,
            })
            .collect(),
        prompt_history: vec![],
    }
}

fn diff(path: &str, old_text: Option<&str>, new_text: Option<&str>) -> ToolCallDiff {
    ToolCallDiff {
        path: path.into(),
        old_text: old_text.map(str::to_string),
        new_text: new_text.map(str::to_string),
    }
}

fn s(calls: Vec<(&str, Vec<ToolCallDiff>)>) -> Session {
    session_with(calls, CWD)
}

#[test]
fn resolve_relative_paths_under_cwd() {
    assert_eq!(
        resolve_workspace_path(CWD, "src/a.ts"),
        Some(PathBuf::from("/repo/workspace/src/a.ts"))
    );
}

#[test]
fn resolve_keeps_absolute_paths_inside() {
    assert_eq!(
        resolve_workspace_path(CWD, "/repo/workspace/src/a.ts"),
        Some(PathBuf::from("/repo/workspace/src/a.ts"))
    );
}

#[test]
fn resolve_accepts_the_directory_itself() {
    assert_eq!(
        resolve_workspace_path(CWD, "."),
        resolve_workspace_path(CWD, CWD)
    );
}

#[test]
fn resolve_rejects_escapes() {
    assert_eq!(resolve_workspace_path(CWD, "../outside"), None);
    assert_eq!(resolve_workspace_path(CWD, "/etc/passwd"), None);
    assert_eq!(resolve_workspace_path(CWD, "a/../../outside"), None);
    assert_eq!(
        resolve_workspace_path("/repo/other", "/repo/workspace/src/a.ts"),
        None
    );
}

#[test]
fn resolve_rejects_empty_and_blank() {
    assert_eq!(resolve_workspace_path(CWD, ""), None);
    assert_eq!(resolve_workspace_path(CWD, "   "), None);
}

#[test]
fn diffs_for_path_collects_in_session_order() {
    let session = s(vec![
        ("c1", vec![diff("a.ts", Some("0"), Some("1"))]),
        (
            "c2",
            vec![
                diff(&format!("{CWD}/a.ts"), Some("1"), Some("2")),
                diff("b.ts", Some("x"), Some("y")),
            ],
        ),
        ("c3", vec![]),
    ]);
    let diffs = diffs_for_path(&session, "a.ts");
    assert_eq!(
        diffs.iter().map(|d| d.tool_call_id).collect::<Vec<_>>(),
        ["c1", "c2"]
    );
    assert_eq!(
        diffs
            .iter()
            .map(|d| d.diff.new_text.as_deref())
            .collect::<Vec<_>>(),
        [Some("1"), Some("2")]
    );
    assert_eq!(diffs_for_path(&session, &format!("{CWD}/a.ts")).len(), 2);
    assert_eq!(diffs_for_path(&session, &format!("{CWD}/b.ts")).len(), 1);
    assert!(diffs_for_path(&session, "c.ts").is_empty());
}

#[test]
fn restores_whole_file_diffs_to_pre_session_state() {
    let session = s(vec![
        ("c1", vec![diff("a.ts", Some("v0"), Some("v1"))]),
        ("c2", vec![diff("a.ts", Some("v1"), Some("v2"))]),
    ]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("v2"), None),
        FileRestorePlan::Write {
            path: "a.ts".into(),
            content: "v0".into()
        }
    );
}

#[test]
fn reverts_only_the_named_call() {
    let session = s(vec![
        ("c1", vec![diff("a.ts", Some("v0"), Some("v1"))]),
        ("c2", vec![diff("a.ts", Some("v1"), Some("v2"))]),
    ]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("v2"), Some("c2")),
        FileRestorePlan::Write {
            path: "a.ts".into(),
            content: "v1".into()
        }
    );
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("v2"), Some("c1")),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "file no longer contains the recorded after-state".into()
        }
    );
}

#[test]
fn reports_unknown_calls_and_paths() {
    let session = s(vec![("c1", vec![diff("a.ts", Some("v0"), Some("v1"))])]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("v1"), Some("nope")),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "no recorded change for this path in call nope".into()
        }
    );
    let empty = s(vec![("c1", vec![])]);
    assert_eq!(
        plan_path_restore(&empty, "a.ts", Some("x"), None),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "no recorded change for this path".into()
        }
    );
}

#[test]
fn deletes_a_created_file_untouched_since() {
    let session = s(vec![(
        "c1",
        vec![diff("new.ts", None, Some("fresh content"))],
    )]);
    assert_eq!(
        plan_path_restore(&session, "new.ts", Some("fresh content"), None),
        FileRestorePlan::Delete {
            path: "new.ts".into()
        }
    );
}

#[test]
fn hunk_reverts_a_snippet_inside_a_drifted_file() {
    let session = s(vec![(
        "c1",
        vec![diff("a.ts", Some("fn old()"), Some("fn new()"))],
    )]);
    let current = "header\nfn new()\nfooter\n";
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some(current), None),
        FileRestorePlan::Write {
            path: "a.ts".into(),
            content: "header\nfn old()\nfooter\n".into()
        }
    );
}

#[test]
fn skips_when_after_state_matches_multiple_positions() {
    let session = s(vec![("c1", vec![diff("a.ts", Some("x"), Some("dup"))])]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("dup and dup"), None),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "the recorded after-state matches multiple positions".into()
        }
    );
}

#[test]
fn skips_mid_fold_when_earlier_after_state_gone() {
    let session = s(vec![
        ("c1", vec![diff("a.ts", Some("v0"), Some("v1"))]),
        ("c2", vec![diff("a.ts", Some("v1"), Some("v2"))]),
    ]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("drifted"), None),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "file no longer contains the recorded after-state".into()
        }
    );
}

#[test]
fn resurrects_before_state_when_file_deleted() {
    let session = s(vec![("c1", vec![diff("a.ts", Some("v0"), Some("v1"))])]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", None, None),
        FileRestorePlan::Write {
            path: "a.ts".into(),
            content: "v0".into()
        }
    );
}

#[test]
fn unchanged_when_created_file_already_absent() {
    let session = s(vec![("c1", vec![diff("new.ts", None, Some("content"))])]);
    assert_eq!(
        plan_path_restore(&session, "new.ts", None, None),
        FileRestorePlan::Unchanged {
            path: "new.ts".into()
        }
    );
}

#[test]
fn restores_a_delete_diff_when_file_absent() {
    let session = s(vec![(
        "c1",
        vec![diff("gone.ts", Some("whole file"), None)],
    )]);
    assert_eq!(
        plan_path_restore(&session, "gone.ts", None, None),
        FileRestorePlan::Write {
            path: "gone.ts".into(),
            content: "whole file".into()
        }
    );
}

#[test]
fn skips_a_removal_revert_while_file_exists() {
    let session = s(vec![(
        "c1",
        vec![diff("a.ts", Some("removed block"), None)],
    )]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("current content"), None),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "cannot re-locate where the recorded removal happened".into()
        }
    );
}

#[test]
fn skips_a_diff_with_neither_side() {
    let session = s(vec![("c1", vec![diff("a.ts", None, None)])]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("x"), None),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "the recorded diff carries no content".into()
        }
    );
}

#[test]
fn round_trips_create_edit_delete_chains() {
    let session = s(vec![
        ("c1", vec![diff("a.ts", None, Some("v1"))]),
        ("c2", vec![diff("a.ts", Some("v1"), Some("v2"))]),
        ("c3", vec![diff("a.ts", Some("v2"), None)]),
    ]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", None, None),
        FileRestorePlan::Unchanged {
            path: "a.ts".into()
        }
    );
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("v2"), None),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "cannot re-locate where the recorded removal happened".into()
        }
    );
}

#[test]
fn handles_empty_after_state() {
    let session = s(vec![("c1", vec![diff("a.ts", Some("stuff"), Some(""))])]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some(""), None),
        FileRestorePlan::Write {
            path: "a.ts".into(),
            content: "stuff".into()
        }
    );
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("other"), None),
        FileRestorePlan::Skip {
            path: "a.ts".into(),
            reason: "the recorded after-state is empty and the file has since changed".into()
        }
    );
}

#[test]
fn removes_a_created_block_inside_a_file_that_grew() {
    let session = s(vec![(
        "c1",
        vec![diff("a.ts", None, Some("created block"))],
    )]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("pre created block post"), None),
        FileRestorePlan::Write {
            path: "a.ts".into(),
            content: "pre  post".into()
        }
    );
}

#[test]
fn unchanged_when_fold_reproduces_current() {
    let session = s(vec![("c1", vec![diff("a.ts", Some("same"), Some("same"))])]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("same"), None),
        FileRestorePlan::Unchanged {
            path: "a.ts".into()
        }
    );
}

#[test]
fn normalizes_request_path_like_diff_matching() {
    let session = s(vec![(
        "c1",
        vec![diff(&format!("{CWD}/a.ts"), Some("0"), Some("1"))],
    )]);
    assert_eq!(
        plan_path_restore(&session, "a.ts", Some("1"), None),
        FileRestorePlan::Write {
            path: "a.ts".into(),
            content: "0".into()
        }
    );
}

#[test]
fn file_history_snapshot_reads_the_backup_map() {
    let mut session = s(vec![]);
    session.metadata = json!({
        "fileHistory": {
            "sessionId": "claude-1",
            "snapshots": {
                "msg-1": {
                    "at": 1,
                    "files": {
                        "/repo/workspace/a.ts": { "backup": "hash@v1", "version": 1 },
                        "/repo/workspace/gone.ts": { "backup": null },
                        "/repo/workspace/junk.ts": "not-an-object",
                        "/repo/workspace/bad.ts": { "backup": 42 }
                    }
                }
            }
        }
    });
    let snap = file_history_snapshot(&session, "msg-1").unwrap();
    assert_eq!(snap.session_id, "claude-1");
    assert_eq!(snap.files.len(), 2);
    assert_eq!(
        snap.files["/repo/workspace/a.ts"].backup.as_deref(),
        Some("hash@v1")
    );
    assert_eq!(snap.files["/repo/workspace/a.ts"].version, Some(1.0));
    assert_eq!(snap.files["/repo/workspace/gone.ts"].backup, None);
}

#[test]
fn file_history_snapshot_none_without_a_map() {
    let mut session = s(vec![]);
    assert!(file_history_snapshot(&session, "msg-1").is_none());
    session.metadata = json!({ "fileHistory": null });
    assert!(file_history_snapshot(&session, "msg-1").is_none());
    session.metadata = json!({ "fileHistory": { "sessionId": "s", "snapshots": {} } });
    assert!(file_history_snapshot(&session, "msg-1").is_none());
    session.metadata = json!({
        "fileHistory": { "sessionId": 42, "snapshots": { "msg-1": { "files": {} } } }
    });
    assert!(file_history_snapshot(&session, "msg-1").is_none());
    session.metadata = json!({
        "fileHistory": { "sessionId": "s", "snapshots": { "msg-1": { "files": null } } }
    });
    assert!(file_history_snapshot(&session, "msg-1").is_none());
    session.metadata = json!({
        "fileHistory": { "sessionId": "s", "snapshots": { "msg-1": "not-an-object" } }
    });
    assert!(file_history_snapshot(&session, "msg-1").is_none());
}
