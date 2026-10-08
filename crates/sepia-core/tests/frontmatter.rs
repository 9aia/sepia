#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_core::frontmatter::{self, FrontmatterDoc};
use serde_json::{Map, Value, json};

fn attrs(doc: &FrontmatterDoc) -> &Map<String, Value> {
    &doc.attributes
}

#[test]
fn parse_returns_whole_text_as_body_without_fence() {
    let doc = frontmatter::parse("# Title\n\nbody text\n");
    assert!(doc.attributes.is_empty());
    assert_eq!(doc.body, "# Title\n\nbody text\n");
}

#[test]
fn parse_reads_scalar_attributes_of_every_shape() {
    let doc = frontmatter::parse(
        "---\nname: my-skill\ndescription: a folded thing\nalwaysApply: true\ncount: 3\nratio: 1.5\nempty:\nquoted: \"has: colon\"\nsingle: 'it''s'\n---\n\nbody here",
    );
    let a = attrs(&doc);
    assert_eq!(a["name"], json!("my-skill"));
    assert_eq!(a["description"], json!("a folded thing"));
    assert_eq!(a["alwaysApply"], json!(true));
    assert_eq!(a["count"], json!(3));
    assert_eq!(a["ratio"], json!(1.5));
    assert_eq!(a["empty"], Value::Null);
    assert_eq!(a["quoted"], json!("has: colon"));
    assert_eq!(a["single"], json!("it's"));
    assert_eq!(doc.body, "body here");
}

#[test]
fn parse_handles_folded_and_literal_block_scalars() {
    let doc = frontmatter::parse(
        "---\ndescription: >-\n  Create Cursor rules for\n  persistent AI guidance.\nnote: |\n  line one\n  line two\n---\nbody",
    );
    assert_eq!(
        attrs(&doc)["description"],
        json!("Create Cursor rules for persistent AI guidance.")
    );
    assert_eq!(attrs(&doc)["note"], json!("line one\nline two"));
}

#[test]
fn parse_handles_dash_lists_nested_maps_and_flow_lists() {
    let doc = frontmatter::parse(
        "---\nallowed-tools:\n  - read\n  - grep\npermissions:\n  allow:\n    - Exec(git)\n    - Read(**)\nglobs: [*.ts, *.tsx]\n---\nx",
    );
    assert_eq!(attrs(&doc)["allowed-tools"], json!(["read", "grep"]));
    assert_eq!(
        attrs(&doc)["permissions"],
        json!({ "allow": ["Exec(git)", "Read(**)"] })
    );
    assert_eq!(attrs(&doc)["globs"], json!(["*.ts", "*.tsx"]));
}

#[test]
fn parse_bails_when_fence_never_closes_or_non_key_line_interrupts() {
    let no_close = frontmatter::parse("---\nname: x\nbody without close");
    assert!(no_close.attributes.is_empty());
    let hr = frontmatter::parse("---\n\na thematic break\n---\nrest");
    assert!(hr.attributes.is_empty());
}

#[test]
fn has_detects_opening_fence() {
    assert!(frontmatter::has("---\nname: x\n---\n"));
    assert!(!frontmatter::has("# no fence"));
    assert!(!frontmatter::has("---"));
}

#[test]
fn render_emits_bare_body_for_empty_attributes() {
    assert_eq!(
        frontmatter::render(&Map::new(), "plain body"),
        "plain body\n"
    );
    assert_eq!(
        frontmatter::render(&Map::new(), "plain body\n"),
        "plain body\n"
    );
}

#[test]
fn render_round_trips_through_parse() {
    let attrs = json!({
        "name": "skill",
        "description": "uses a: colon",
        "alwaysApply": true,
        "globs": ["*.ts", "*.tsx"],
        "permissions": { "allow": ["Exec(git)"] },
        "note": "line one\nline two"
    })
    .as_object()
    .unwrap()
    .clone();
    let text = frontmatter::render(&attrs, "the body\n");
    let doc = frontmatter::parse(&text);
    assert_eq!(doc.attributes, attrs);
    assert_eq!(doc.body, "the body\n");
}

#[test]
fn render_quotes_ambiguous_scalars_and_keeps_plain() {
    let attrs = json!({ "plain": "hello-world", "booly": "true", "colon": "a: b", "list": "a, b" })
        .as_object()
        .unwrap()
        .clone();
    let text = frontmatter::render(&attrs, "x");
    assert!(text.contains("plain: hello-world"));
    assert!(text.contains("booly: \"true\""));
    assert!(text.contains("colon: \"a: b\""));
    let doc = frontmatter::parse(&text);
    assert_eq!(doc.attributes["plain"], json!("hello-world"));
    assert_eq!(doc.attributes["booly"], json!("true"));
    assert_eq!(doc.attributes["colon"], json!("a: b"));
    assert_eq!(doc.attributes["list"], json!("a, b"));
}

#[test]
fn render_emits_null_and_object_values_losslessly() {
    let attrs = json!({ "nothing": null, "nested": { "a": 1 } })
        .as_object()
        .unwrap()
        .clone();
    let text = frontmatter::render(&attrs, "x");
    let doc = frontmatter::parse(&text);
    assert_eq!(doc.attributes["nothing"], Value::Null);
    assert_eq!(doc.attributes["nested"], json!({ "a": 1 }));
}

#[test]
fn parse_handles_bare_dash_items_and_scalar_edge_cases() {
    let doc = frontmatter::parse(
        "---\nitems:\n  -\n  - one\nneg: -5\nempty-list: []\nbadquote: \"unterminated\nnumstr: '007'\n---\nx",
    );
    assert_eq!(attrs(&doc)["items"], json!([null, "one"]));
    assert_eq!(attrs(&doc)["neg"], json!(-5));
    assert_eq!(attrs(&doc)["empty-list"], json!([]));
    assert_eq!(attrs(&doc)["badquote"], json!("\"unterminated"));
    assert_eq!(attrs(&doc)["numstr"], json!("007"));
}

#[test]
fn parse_bails_on_bare_list_fence() {
    let text = "---\n- a\n- b\n---\nbody";
    let doc = frontmatter::parse(text);
    assert!(doc.attributes.is_empty());
    assert_eq!(doc.body, text);
}

#[test]
fn parse_handles_top_level_map_child_with_trailing_blanks() {
    let doc = frontmatter::parse("---\nkey:\n  nested:\n    - x\nnext: v\n---\nb");
    assert_eq!(doc.attributes["key"], json!({ "nested": ["x"] }));
    assert_eq!(doc.attributes["next"], json!("v"));
}

#[test]
fn render_emits_empty_arrays_and_multiline_literal_scalars() {
    let attrs = json!({ "empty": [], "multi": "one\ntwo\nthree" })
        .as_object()
        .unwrap()
        .clone();
    let text = frontmatter::render(&attrs, "body");
    assert!(text.contains("empty: []"));
    assert!(text.contains("multi: |"));
    let doc = frontmatter::parse(&text);
    assert_eq!(doc.attributes["multi"], json!("one\ntwo\nthree"));
    assert_eq!(doc.attributes["empty"], json!([]));
}

#[test]
fn render_keeps_object_scalars_inside_lists_as_json() {
    let attrs = json!({ "list": ["a", { "k": 1 }] })
        .as_object()
        .unwrap()
        .clone();
    let text = frontmatter::render(&attrs, "x");
    let doc = frontmatter::parse(&text);
    assert_eq!(doc.attributes["list"], json!(["a", "{\"k\":1}"]));
}

#[test]
fn parse_unquotes_broken_json_string_by_slicing_quotes() {
    let doc = frontmatter::parse("---\nkey: \"a\"b\"\n---\nx");
    assert_eq!(doc.attributes["key"], json!("a\"b"));
}

#[test]
fn parse_skips_blank_lines_inside_maps_and_lists() {
    let doc = frontmatter::parse("---\na: 1\n\nb: 2\n\nitems:\n  - x\n\n  - y\n---\nz");
    assert_eq!(doc.attributes["a"], json!(1));
    assert_eq!(doc.attributes["b"], json!(2));
    assert_eq!(doc.attributes["items"], json!(["x", "y"]));
}

#[test]
fn parse_reads_bare_dash_item_with_nested_block() {
    let doc = frontmatter::parse("---\nitems:\n  -\n    - x\n    - y\n  - z\n---\nb");
    assert_eq!(doc.attributes["items"], json!([["x", "y"], "z"]));
}

#[test]
fn parse_handles_chomping_and_indented_non_key_line() {
    let doc = frontmatter::parse("---\nnote: |+\n  kept\n  \nkey: v\n---\nb");
    assert_eq!(doc.attributes["note"], json!("kept"));
    let stray = frontmatter::parse("---\n  orphan\nkey: v\n---\nb");
    assert!(stray.attributes.is_empty());
}
