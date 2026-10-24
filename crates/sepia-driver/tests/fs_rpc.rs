#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! Unit coverage for the driver's fs helpers, rpc framing edges, and
//! manifest validation — the helpers every driver binary shares.

use sepia_driver::fs;
use sepia_driver::manifest::{Capability, DriverManifest};
use sepia_driver::rpc::{self, Inbound};
use serde_json::{Value, json};

// ---------- fs ---------------------------------------------------------------

#[test]
fn read_file_if_exists_distinguishes_missing_from_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        fs::read_file_if_exists(&dir.path().join("nope")).unwrap(),
        None
    );
    // NotADirectory also reads as absent.
    let file = dir.path().join("file");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(fs::read_file_if_exists(&file.join("child")).unwrap(), None);
    assert_eq!(fs::read_file_if_exists(&file).unwrap(), Some("x".into()));
}

#[test]
fn read_json_if_exists_is_tolerant() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.json");
    assert!(fs::read_json_if_exists(&p).unwrap().is_none());
    std::fs::write(&p, "not json").unwrap();
    assert!(fs::read_json_if_exists(&p).unwrap().is_none());
    std::fs::write(&p, "[1,2]").unwrap();
    assert!(fs::read_json_if_exists(&p).unwrap().is_none()); // not an object
    std::fs::write(&p, "{\"a\":1}").unwrap();
    assert_eq!(fs::read_json_if_exists(&p).unwrap().unwrap()["a"], 1);
}

#[test]
fn list_dir_if_exists_covers_missing_and_populated() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        fs::list_dir_if_exists(&dir.path().join("nope"))
            .unwrap()
            .is_empty()
    );
    // A file is NotADirectory → empty.
    let file = dir.path().join("f");
    std::fs::write(&file, "x").unwrap();
    assert!(fs::list_dir_if_exists(&file).unwrap().is_empty());
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("a"), "").unwrap();
    assert_eq!(fs::list_dir_if_exists(&sub).unwrap(), vec!["a"]);
}

#[test]
fn write_file_action_reports_wrote_updated_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("deep/nested/f.md");
    let a = fs::write_file_action(&p, "v1").unwrap();
    assert_eq!(a.action, sepia_core::agent_config::WriteActionKind::Wrote);
    let a = fs::write_file_action(&p, "v1").unwrap();
    assert_eq!(
        a.action,
        sepia_core::agent_config::WriteActionKind::Unchanged
    );
    let a = fs::write_file_action(&p, "v2").unwrap();
    assert_eq!(a.action, sepia_core::agent_config::WriteActionKind::Updated);
    // A write under a path that is a file fails.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    assert!(fs::write_file_action(&blocker.join("child"), "y").is_err());
}

#[test]
fn read_markdown_dir_parses_sorted_docs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("b.md"),
        "---\ndescription: bee\n---\nbody b",
    )
    .unwrap();
    std::fs::write(dir.path().join("a.mdc"), "plain a").unwrap();
    std::fs::write(dir.path().join("skip.txt"), "ignored").unwrap();
    let docs = fs::read_markdown_dir(dir.path(), &[".md", ".mdc"]).unwrap();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].stem, "a");
    assert_eq!(docs[0].body, "plain a");
    assert_eq!(docs[1].stem, "b");
    assert_eq!(docs[1].attributes["description"], "bee");
    assert!(docs[1].file_name.ends_with(".md"));
    // Missing dir → empty.
    assert!(
        fs::read_markdown_dir(&dir.path().join("none"), &[".md"])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn skills_dir_round_trip_includes_sibling_files() {
    let dir = tempfile::tempdir().unwrap();
    let skills_root = dir.path().join("skills");
    let skills = vec![sepia_core::agent_config::ConfigSkill {
        name: "tidy".into(),
        description: Some("clean".into()),
        body: "do it".into(),
        files: vec![
            sepia_core::agent_config::ConfigFile {
                path: "extra.txt".into(),
                content: "x".into(),
            },
            sepia_core::agent_config::ConfigFile {
                path: "sub/deep.txt".into(),
                content: "deep".into(),
            },
            // An escaping path is skipped, not written.
            sepia_core::agent_config::ConfigFile {
                path: "../evil".into(),
                content: "no".into(),
            },
        ],
        metadata: Value::Null,
    }];
    let actions = fs::write_skills_dir(&skills_root, &skills).unwrap();
    assert!(
        actions
            .iter()
            .any(|a| a.action == sepia_core::agent_config::WriteActionKind::Skipped)
    );
    assert!(!skills_root.join("evil").exists());

    let read = fs::read_skills_dir(&skills_root).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].name, "tidy");
    assert_eq!(read[0].description.as_deref(), Some("clean"));
    let files: Vec<&str> = read[0].files.iter().map(|f| f.path.as_str()).collect();
    assert!(files.contains(&"extra.txt"));
    assert!(files.contains(&"sub/deep.txt"));

    // A skill dir without SKILL.md is skipped on read; a frontmatter
    // `name` overrides the dir name.
    std::fs::create_dir_all(skills_root.join("empty")).unwrap();
    std::fs::create_dir_all(skills_root.join("named")).unwrap();
    std::fs::write(
        skills_root.join("named/SKILL.md"),
        "---\nname: custom\n---\nbody",
    )
    .unwrap();
    let read = fs::read_skills_dir(&skills_root).unwrap();
    assert!(read.iter().any(|s| s.name == "custom"));
    assert!(!read.iter().any(|s| s.name == "empty"));

    // An unsafe skill name is skipped on write: ".." survives
    // `safe_file_stem`'s sanitizers as a bare traversal segment.
    let bad = vec![sepia_core::agent_config::ConfigSkill {
        name: "..".into(),
        description: None,
        body: String::new(),
        files: vec![],
        metadata: Value::Null,
    }];
    let actions = fs::write_skills_dir(&skills_root.join("other"), &bad).unwrap();
    assert_eq!(
        actions[0].action,
        sepia_core::agent_config::WriteActionKind::Skipped
    );
}

// ---------- rpc ----------------------------------------------------------------

#[test]
fn decode_line_classifies_every_inbound_shape() {
    assert!(matches!(
        rpc::decode_line("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"m\"}").unwrap(),
        Inbound::Request(_)
    ));
    assert!(matches!(
        rpc::decode_line("{\"jsonrpc\":\"2.0\",\"method\":\"m\"}").unwrap(),
        Inbound::Notification(_)
    ));
    assert!(matches!(
        rpc::decode_line("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}").unwrap(),
        Inbound::Response(_)
    ));
    // Errors carry back an id only when one was recovered.
    assert_eq!(
        rpc::decode_line("nope").unwrap_err().1.code,
        rpc::PARSE_ERROR
    );
    assert_eq!(
        rpc::decode_line("[1]").unwrap_err().1.code,
        rpc::INVALID_REQUEST
    );
    assert_eq!(
        rpc::decode_line("{}").unwrap_err().1.code,
        rpc::INVALID_REQUEST
    );
    // A response with a malformed error object → parse error tagged with id.
    let (id, err) = rpc::decode_line("{\"id\": 5, \"error\": \"not-an-object\"}").unwrap_err();
    assert_eq!(id, Some(5));
    assert_eq!(err.code, rpc::PARSE_ERROR);
    // A well-formed error response decodes.
    assert!(matches!(
        rpc::decode_line("{\"id\": 5, \"error\": {\"code\": -1, \"message\": \"m\"}}").unwrap(),
        Inbound::Response(_)
    ));
}

#[test]
fn rpc_types_construct_and_display() {
    let req = rpc::Request::new(3, "m", json!({"a": 1}));
    assert_eq!(req.id, 3);
    let note = rpc::Notification::new("n", json!(null));
    assert_eq!(note.method, "n");
    let res = rpc::Response::ok(1, json!("r"));
    assert_eq!(res.result, Some(json!("r")));
    let res = rpc::Response::err(1, rpc::RpcError::internal("boom"));
    assert_eq!(res.error.as_ref().unwrap().code, rpc::INTERNAL_ERROR);
    assert_eq!(
        rpc::RpcError::store("s").code,
        rpc::STORE_ERROR,
        "store error code"
    );
    assert_eq!(rpc::RpcError::invalid_params("p").code, rpc::INVALID_PARAMS);
    assert_eq!(rpc::RpcError::new(-1, "msg").to_string(), "msg (code -1)");
}

// ---------- manifest ------------------------------------------------------------

fn manifest() -> DriverManifest {
    DriverManifest {
        id: "m".into(),
        label: "M".into(),
        version: "0".into(),
        protocol: sepia_driver::DRIVER_PROTOCOL,
        capabilities: [Capability::SessionStore].into_iter().collect(),
        agent_command: None,
        config_schema: json!({}),
        backend_type: None,
    }
}

#[test]
fn manifest_validate_rejects_bad_id_and_protocol() {
    assert!(manifest().validate().is_ok());
    let mut bad = manifest();
    bad.id = String::new();
    assert!(bad.validate().unwrap_err().contains("id"));
    let mut bad = manifest();
    bad.protocol = 99;
    assert!(bad.validate().unwrap_err().contains("protocol"));
}

#[test]
fn capability_convert_variant_round_trips() {
    let mut m = manifest();
    m.capabilities.insert(Capability::Convert {
        from: "a".into(),
        to: "b".into(),
    });
    let json = serde_json::to_value(&m).unwrap();
    let back: DriverManifest = serde_json::from_value(json).unwrap();
    assert_eq!(back, m);
}
